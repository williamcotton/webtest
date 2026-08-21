//! Incremental workspace and semantic query facade.

use std::{collections::HashMap, sync::Arc};

use thiserror::Error;
use webtest_hir::HirFile;
use webtest_plan::TestPlan;
use webtest_syntax::Parse;
use webtest_text::{FileId, SourceRevision, TextRange};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticSource {
    Syntax,
    Semantic,
    Runtime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: TextRange,
    pub severity: DiagnosticSeverity,
    pub code: &'static str,
    pub message: String,
    pub source: DiagnosticSource,
}

#[derive(Debug, Error)]
pub enum AnalysisError {
    #[error("unknown source file {0:?}")]
    UnknownFile(FileId),
}

#[derive(Clone, Debug)]
struct SourceFile {
    path: String,
    text: Arc<str>,
    revision: SourceRevision,
}

#[derive(Clone, Debug)]
struct CachedQueries {
    revision: SourceRevision,
    parse: Parse,
    hir: Arc<HirFile>,
    diagnostics: Arc<Vec<Diagnostic>>,
    plan: Arc<TestPlan>,
}

#[derive(Default)]
pub struct AnalysisDatabase {
    files: HashMap<FileId, SourceFile>,
    paths: HashMap<String, FileId>,
    cache: HashMap<FileId, CachedQueries>,
    next_file: u32,
}

impl AnalysisDatabase {
    pub fn open_file(&mut self, path: impl Into<String>, text: impl Into<String>) -> FileId {
        let path = path.into();
        if let Some(file) = self.paths.get(&path).copied() {
            self.set_file_text(file, text);
            return file;
        }
        let file = FileId::new(self.next_file);
        self.next_file += 1;
        let text: Arc<str> = Arc::from(text.into());
        self.files.insert(
            file,
            SourceFile {
                path: path.clone(),
                revision: SourceRevision::of(&text),
                text,
            },
        );
        self.paths.insert(path, file);
        file
    }

    pub fn set_file_text(&mut self, file: FileId, text: impl Into<String>) {
        let text: Arc<str> = Arc::from(text.into());
        if let Some(source) = self.files.get_mut(&file) {
            let revision = SourceRevision::of(&text);
            if source.revision != revision {
                source.text = text;
                source.revision = revision;
                self.cache.remove(&file);
            }
        }
    }

    pub fn file_for_path(&self, path: &str) -> Option<FileId> {
        self.paths.get(path).copied()
    }

    pub fn close_file(&mut self, file: FileId) {
        if let Some(source) = self.files.remove(&file) {
            self.paths.remove(&source.path);
        }
        self.cache.remove(&file);
    }

    pub fn source(&self, file: FileId) -> Result<Arc<str>, AnalysisError> {
        self.files
            .get(&file)
            .map(|source| Arc::clone(&source.text))
            .ok_or(AnalysisError::UnknownFile(file))
    }

    pub fn path(&self, file: FileId) -> Result<&str, AnalysisError> {
        self.files
            .get(&file)
            .map(|source| source.path.as_str())
            .ok_or(AnalysisError::UnknownFile(file))
    }

    pub fn source_revision(&self, file: FileId) -> Result<SourceRevision, AnalysisError> {
        self.files
            .get(&file)
            .map(|source| source.revision)
            .ok_or(AnalysisError::UnknownFile(file))
    }

    pub fn parse(&mut self, file: FileId) -> Result<Parse, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(self.cache[&file].parse.clone())
    }

    pub fn hir_file(&mut self, file: FileId) -> Result<Arc<HirFile>, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(Arc::clone(&self.cache[&file].hir))
    }

    pub fn diagnostics(&mut self, file: FileId) -> Result<Arc<Vec<Diagnostic>>, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(Arc::clone(&self.cache[&file].diagnostics))
    }

    pub fn test_plan(&mut self, file: FileId) -> Result<Arc<TestPlan>, AnalysisError> {
        self.ensure_queries(file)?;
        Ok(Arc::clone(&self.cache[&file].plan))
    }

    fn ensure_queries(&mut self, file: FileId) -> Result<(), AnalysisError> {
        let source = self
            .files
            .get(&file)
            .ok_or(AnalysisError::UnknownFile(file))?;
        if self
            .cache
            .get(&file)
            .is_some_and(|cached| cached.revision == source.revision)
        {
            return Ok(());
        }

        let parsed = webtest_syntax::parse(&source.text);
        let mut diagnostics = parsed
            .errors()
            .iter()
            .map(|error| Diagnostic {
                range: error.range,
                severity: DiagnosticSeverity::Error,
                code: error.code,
                message: error.message.clone(),
                source: DiagnosticSource::Syntax,
            })
            .collect::<Vec<_>>();
        diagnostics.extend(
            parsed
                .syntax()
                .descendants_with_tokens()
                .filter_map(|element| {
                    let token = element.into_token()?;
                    if token.kind() != webtest_syntax::SyntaxKind::Duration {
                        return None;
                    }
                    let valid = token
                        .text()
                        .strip_suffix("ms")
                        .or_else(|| token.text().strip_suffix('s'))
                        .or_else(|| token.text().strip_suffix('m'))
                        .and_then(|number| number.parse::<u64>().ok())
                        .is_some_and(|number| number > 0);
                    (!valid).then(|| Diagnostic {
                        range: token.text_range(),
                        severity: DiagnosticSeverity::Error,
                        code: "semantic.invalid_duration",
                        message: format!("invalid positive duration `{}`", token.text()),
                        source: DiagnosticSource::Semantic,
                    })
                }),
        );
        let hir = Arc::new(webtest_hir::lower(file, &parsed));
        for test in &hir.tests {
            for statement in &test.body {
                let webtest_hir::HirStmt::Browser(block) = statement;
                for operation in &block.operations {
                    if let webtest_hir::HirBrowserOp::Press(action) = operation
                        && !valid_key_chord(&action.value.value)
                    {
                        diagnostics.push(Diagnostic {
                            range: action.value.origin.range,
                            severity: DiagnosticSeverity::Error,
                            code: "semantic.invalid_key",
                            message: format!("invalid key chord `{}`", action.value.value),
                            source: DiagnosticSource::Semantic,
                        });
                    }
                }
            }
        }
        let plan = Arc::new(webtest_plan::lower(file, source.revision, &hir));
        self.cache.insert(
            file,
            CachedQueries {
                revision: source.revision,
                parse: parsed,
                hir,
                diagnostics: Arc::new(diagnostics),
                plan,
            },
        );
        Ok(())
    }
}

fn valid_key_chord(value: &str) -> bool {
    let mut main = 0;
    for part in value.split('+') {
        match part {
            "Alt" | "Control" | "Ctrl" | "Meta" | "Command" | "Shift" => {}
            "Enter" | "Tab" | "Escape" | "Esc" | "Backspace" | "Delete" | "ArrowUp"
            | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "Home" | "End" | "PageUp" | "PageDown"
            | "Space" => main += 1,
            value if value.chars().count() == 1 => main += 1,
            _ => return false,
        }
    }
    main == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_diagnostics_and_queries_follow_source_updates() {
        let mut database = AnalysisDatabase::default();
        let file = database.open_file(
            "file:///test.webtest",
            "test \"x\" { browser { open click id(\"x\") } }",
        );
        let first_revision = database.source_revision(file).expect("first revision");
        assert!(
            database
                .diagnostics(file)
                .expect("invalid diagnostics")
                .iter()
                .any(|diagnostic| diagnostic.code == "syntax.expected_url")
        );

        database.set_file_text(
            file,
            "test \"x\" { browser { open \"about:blank\" click id(\"x\") } }",
        );
        assert_ne!(
            database.source_revision(file).expect("second revision"),
            first_revision
        );
        assert!(
            database
                .diagnostics(file)
                .expect("valid diagnostics")
                .is_empty()
        );
        assert_eq!(database.test_plan(file).expect("plan").tests.len(), 1);
    }
}
