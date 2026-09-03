use std::{
    collections::BTreeMap,
    fmt, fs,
    fs::OpenOptions,
    io::{self, Write},
    path::{Path, PathBuf},
};

use webtest_app_bridge::{AppManifest, FieldSchema, FunctionSchema, PROTOCOL_VERSION, TypeSchema};

use crate::{error::AppError, report::ExitClass};

pub const PROJECT_CONFIG: &str = r#"[project]
test_roots = ["tests"]

[server.app]
adapter = "bridge"
transport = "auto"
schema = ".webtest/app-schema.json"

# Configure these sections before running application and browser tests.
# WebTest launches [app]; [browser] points browser steps at that launched app.
#
# [app]
# command = "your-application-command"
# args = []
# working_directory = "."
#
# [browser]
# base_url = "http://127.0.0.1:3000"
#
# [server]
# base_url = "http://127.0.0.1:3000"
"#;

pub const EXAMPLE_TEST: &str = r#"test "application bridge responds" {
    server {
        let echoed = app.echo(message: "hello from WebTest")
        expect echoed == "hello from WebTest"
    }
}
"#;

#[cfg(test)]
const FULL_FLOW_EXAMPLE: &str = r#"test "created user can sign in" {
    server {
        let user = app.create_user(email: "alice@example.com", admin: false)
        let response = http.post("/api/login", json: { email: user.email })
        expect response.status == 200
    }

    browser {
        open "/login"
        fill label("Email") with user.email
        click role("button", name: "Sign in")
        expect text("Welcome").visible
    }
}
"#;

pub const WEBTEST_SKILL: &str = include_str!("../../../.agents/skills/webtest/SKILL.md");

const CLAUDE_SKILL_LINK: &str = ".claude/skills/webtest";
const CLAUDE_SKILL_TARGET: &str = "../../.agents/skills/webtest";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorClass {
    Usage,
    Infrastructure,
    Internal,
}

#[derive(Debug)]
pub struct InitError {
    class: ErrorClass,
    message: String,
}

impl InitError {
    pub fn class(&self) -> ErrorClass {
        self.class
    }

    fn usage(message: impl Into<String>) -> Self {
        Self {
            class: ErrorClass::Usage,
            message: message.into(),
        }
    }

    fn infrastructure(message: impl Into<String>) -> Self {
        Self {
            class: ErrorClass::Infrastructure,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            class: ErrorClass::Internal,
            message: message.into(),
        }
    }
}

impl fmt::Display for InitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for InitError {}

#[derive(Debug)]
pub struct InitOutcome {
    pub root: PathBuf,
    pub created: Vec<String>,
    pub unchanged: Vec<String>,
    pub warnings: Vec<String>,
}

struct ScaffoldFile {
    relative: &'static str,
    contents: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExistingState {
    Missing,
    Unchanged,
}

#[derive(Debug)]
enum CreatedArtifact {
    File(PathBuf),
    DirectoryLink(PathBuf),
}

pub fn initialize(requested_root: &Path) -> Result<InitOutcome, InitError> {
    let files = scaffold_files()?;
    let (root, root_created) = prepare_root(requested_root)?;

    let mut states = Vec::with_capacity(files.len());
    let mut conflicts = Vec::new();
    for file in &files {
        let path = root.join(file.relative);
        if has_conflicting_parent(&path, &root)? {
            states.push(ExistingState::Missing);
            conflicts.push(file.relative.to_string());
            continue;
        }
        match existing_file_state(&path, file.contents.as_bytes())? {
            Ok(state) => states.push(state),
            Err(()) => conflicts.push(file.relative.to_string()),
        }
    }
    let alias_path = root.join(CLAUDE_SKILL_LINK);
    let alias_state = if has_conflicting_parent(&alias_path, &root)? {
        conflicts.push(CLAUDE_SKILL_LINK.into());
        ExistingState::Missing
    } else {
        match existing_alias_state(&alias_path)? {
            Ok(state) => state,
            Err(()) => {
                conflicts.push(CLAUDE_SKILL_LINK.into());
                ExistingState::Missing
            }
        }
    };

    if !conflicts.is_empty() {
        if root_created {
            let _ = fs::remove_dir(&root);
        }
        return Err(InitError::usage(format!(
            "refusing to overwrite conflicting WebTest scaffold path(s): {}",
            conflicts.join(", ")
        )));
    }

    let mut created = Vec::new();
    let mut unchanged = Vec::new();
    let mut warnings = Vec::new();
    let mut created_directories = Vec::new();
    let mut created_artifacts = Vec::new();

    let write_result = (|| {
        for (file, state) in files.iter().zip(states) {
            match state {
                ExistingState::Missing => {
                    let path = root.join(file.relative);
                    ensure_directory(
                        path.parent().ok_or_else(|| {
                            InitError::internal(format!(
                                "scaffold path `{}` has no parent",
                                path.display()
                            ))
                        })?,
                        &root,
                        &mut created_directories,
                    )?;
                    create_file(&path, file.contents.as_bytes())?;
                    created_artifacts.push(CreatedArtifact::File(path));
                    created.push(file.relative.to_string());
                }
                ExistingState::Unchanged => unchanged.push(file.relative.to_string()),
            }
        }

        match alias_state {
            ExistingState::Missing => {
                let copied =
                    create_claude_alias(&root, &mut created_directories, &mut created_artifacts)?;
                created.push(CLAUDE_SKILL_LINK.into());
                if copied {
                    warnings.push(
                        "the platform could not create the Claude skill symlink; installed an exact copy instead"
                            .into(),
                    );
                }
            }
            ExistingState::Unchanged => unchanged.push(CLAUDE_SKILL_LINK.into()),
        }
        Ok(())
    })();

    if let Err(error) = write_result {
        rollback(&created_artifacts, &created_directories);
        if root_created {
            let _ = fs::remove_dir(&root);
        }
        return Err(error);
    }

    Ok(InitOutcome {
        root,
        created,
        unchanged,
        warnings,
    })
}

pub(crate) fn run(path: &Path) -> Result<ExitClass, AppError> {
    let outcome = initialize(path).map_err(|error| match error.class() {
        ErrorClass::Usage => AppError::usage(error),
        ErrorClass::Infrastructure => AppError::infrastructure(error),
        ErrorClass::Internal => AppError::internal(error),
    })?;
    let action = if outcome.created.is_empty() {
        "WebTest project is already initialized at"
    } else {
        "initialized WebTest project at"
    };
    println!("{action} {}", outcome.root.display());
    for path in &outcome.created {
        println!("  created {path}");
    }
    for path in &outcome.unchanged {
        println!("  unchanged {path}");
    }
    for warning in &outcome.warnings {
        eprintln!("warning[init.skill_link]: {warning}");
    }
    println!("next:");
    println!("  configure [app] in webtest.toml");
    println!("  implement the app.echo bridge using .agents/skills/webtest/SKILL.md");
    println!("  use `webtest describe app.protocol` for the complete protocol reference");
    println!("  webtest check");
    println!("  webtest test");
    Ok(ExitClass::Success)
}

fn scaffold_files() -> Result<Vec<ScaffoldFile>, InitError> {
    Ok(vec![
        ScaffoldFile {
            relative: "webtest.toml",
            contents: PROJECT_CONFIG.into(),
        },
        ScaffoldFile {
            relative: ".webtest/app-schema.json",
            contents: manifest_json()?,
        },
        ScaffoldFile {
            relative: "tests/example.webtest",
            contents: EXAMPLE_TEST.into(),
        },
        ScaffoldFile {
            relative: ".agents/skills/webtest/SKILL.md",
            contents: WEBTEST_SKILL.into(),
        },
    ])
}

fn manifest_json() -> Result<String, InitError> {
    let parameters = BTreeMap::from([(
        "message".into(),
        FieldSchema {
            ty: TypeSchema::String,
            documentation: "Text returned unchanged by the bootstrap bridge operation.".into(),
            optional: false,
            secret: false,
            default: None,
        },
    )]);
    let manifest = AppManifest {
        manifest_version: 1,
        protocol: PROTOCOL_VERSION,
        provider: "app".into(),
        sdk: "webtest-init".into(),
        sdk_version: env!("CARGO_PKG_VERSION").into(),
        schema_hash: String::new(),
        functions: BTreeMap::from([(
            "echo".into(),
            FunctionSchema {
                documentation:
                    "Return the supplied message to prove the application bridge is connected."
                        .into(),
                retry_safe: true,
                params: TypeSchema::Object { fields: parameters },
                returns: TypeSchema::String,
            },
        )]),
    }
    .with_computed_hash()
    .map_err(|error| InitError::internal(format!("could not hash bootstrap schema: {error}")))?;
    manifest
        .validate()
        .map_err(|error| InitError::internal(format!("bootstrap schema is invalid: {error}")))?;
    let mut output = serde_json::to_string_pretty(&manifest).map_err(|error| {
        InitError::internal(format!("could not serialize bootstrap schema: {error}"))
    })?;
    output.push('\n');
    Ok(output)
}

fn prepare_root(requested: &Path) -> Result<(PathBuf, bool), InitError> {
    let absolute = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                InitError::infrastructure(format!(
                    "could not determine the current directory: {error}"
                ))
            })?
            .join(requested)
    };
    match fs::metadata(&absolute) {
        Ok(metadata) if !metadata.is_dir() => Err(InitError::usage(format!(
            "initialization target `{}` is not a directory",
            absolute.display()
        ))),
        Ok(_) => fs::canonicalize(&absolute)
            .map(|path| (path, false))
            .map_err(|error| {
                InitError::infrastructure(format!(
                    "could not resolve initialization target `{}`: {error}",
                    absolute.display()
                ))
            }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(&absolute).map_err(|error| {
                InitError::infrastructure(format!(
                    "could not create initialization target `{}`: {error}",
                    absolute.display()
                ))
            })?;
            fs::canonicalize(&absolute)
                .map(|path| (path, true))
                .map_err(|error| {
                    InitError::infrastructure(format!(
                        "could not resolve initialization target `{}`: {error}",
                        absolute.display()
                    ))
                })
        }
        Err(error) => Err(InitError::infrastructure(format!(
            "could not inspect initialization target `{}`: {error}",
            absolute.display()
        ))),
    }
}

fn existing_file_state(
    path: &Path,
    expected: &[u8],
) -> Result<Result<ExistingState, ()>, InitError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            let contents = fs::read(path).map_err(|error| {
                InitError::infrastructure(format!(
                    "could not read existing scaffold path `{}`: {error}",
                    path.display()
                ))
            })?;
            Ok(if contents == expected {
                Ok(ExistingState::Unchanged)
            } else {
                Err(())
            })
        }
        Ok(_) => Ok(Err(())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Ok(ExistingState::Missing)),
        Err(error) => Err(InitError::infrastructure(format!(
            "could not inspect scaffold path `{}`: {error}",
            path.display()
        ))),
    }
}

fn has_conflicting_parent(path: &Path, root: &Path) -> Result<bool, InitError> {
    let parent = path.parent().ok_or_else(|| {
        InitError::internal(format!("scaffold path `{}` has no parent", path.display()))
    })?;
    let relative = parent.strip_prefix(root).map_err(|_| {
        InitError::internal(format!(
            "scaffold path `{}` escaped initialization root `{}`",
            path.display(),
            root.display()
        ))
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(InitError::infrastructure(format!(
                    "could not inspect scaffold parent `{}`: {error}",
                    current.display()
                )));
            }
        }
    }
    Ok(false)
}

fn existing_alias_state(path: &Path) -> Result<Result<ExistingState, ()>, InitError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = fs::read_link(path).map_err(|error| {
                InitError::infrastructure(format!(
                    "could not read skill link `{}`: {error}",
                    path.display()
                ))
            })?;
            Ok(if target == Path::new(CLAUDE_SKILL_TARGET) {
                Ok(ExistingState::Unchanged)
            } else {
                Err(())
            })
        }
        Ok(metadata) if metadata.is_dir() => {
            let entries = fs::read_dir(path)
                .map_err(|error| {
                    InitError::infrastructure(format!(
                        "could not inspect copied skill `{}`: {error}",
                        path.display()
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    InitError::infrastructure(format!(
                        "could not inspect copied skill `{}`: {error}",
                        path.display()
                    ))
                })?;
            if entries.len() != 1 || entries[0].file_name() != "SKILL.md" {
                return Ok(Err(()));
            }
            existing_file_state(&path.join("SKILL.md"), WEBTEST_SKILL.as_bytes())
        }
        Ok(_) => Ok(Err(())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Ok(ExistingState::Missing)),
        Err(error) => Err(InitError::infrastructure(format!(
            "could not inspect skill link `{}`: {error}",
            path.display()
        ))),
    }
}

fn ensure_directory(
    directory: &Path,
    root: &Path,
    created: &mut Vec<PathBuf>,
) -> Result<(), InitError> {
    let relative = directory.strip_prefix(root).map_err(|_| {
        InitError::internal(format!(
            "scaffold directory `{}` escaped initialization root `{}`",
            directory.display(),
            root.display()
        ))
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(InitError::infrastructure(format!(
                    "scaffold parent `{}` is not a directory",
                    current.display()
                )));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    InitError::infrastructure(format!(
                        "could not create scaffold directory `{}`: {error}",
                        current.display()
                    ))
                })?;
                created.push(current.clone());
            }
            Err(error) => {
                return Err(InitError::infrastructure(format!(
                    "could not inspect scaffold directory `{}`: {error}",
                    current.display()
                )));
            }
        }
    }
    Ok(())
}

fn create_file(path: &Path, contents: &[u8]) -> Result<(), InitError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            InitError::infrastructure(format!(
                "could not create scaffold file `{}`: {error}",
                path.display()
            ))
        })?;
    if let Err(error) = file.write_all(contents) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(InitError::infrastructure(format!(
            "could not write scaffold file `{}`: {error}",
            path.display()
        )));
    }
    Ok(())
}

fn create_claude_alias(
    root: &Path,
    created_directories: &mut Vec<PathBuf>,
    created_artifacts: &mut Vec<CreatedArtifact>,
) -> Result<bool, InitError> {
    let alias = root.join(CLAUDE_SKILL_LINK);
    let parent = alias
        .parent()
        .ok_or_else(|| InitError::internal("Claude skill link has no parent"))?;
    ensure_directory(parent, root, created_directories)?;

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(CLAUDE_SKILL_TARGET, &alias).map_err(|error| {
            InitError::infrastructure(format!(
                "could not create Claude skill link `{}`: {error}",
                alias.display()
            ))
        })?;
        created_artifacts.push(CreatedArtifact::DirectoryLink(alias));
        Ok(false)
    }

    #[cfg(windows)]
    {
        match std::os::windows::fs::symlink_dir(CLAUDE_SKILL_TARGET, &alias) {
            Ok(()) => {
                created_artifacts.push(CreatedArtifact::DirectoryLink(alias));
                Ok(false)
            }
            Err(_) => {
                create_copied_alias(&alias, root, created_directories, created_artifacts)?;
                Ok(true)
            }
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        create_copied_alias(&alias, root, created_directories, created_artifacts)?;
        Ok(true)
    }
}

#[cfg(not(unix))]
fn create_copied_alias(
    alias: &Path,
    root: &Path,
    created_directories: &mut Vec<PathBuf>,
    created_artifacts: &mut Vec<CreatedArtifact>,
) -> Result<(), InitError> {
    ensure_directory(alias, root, created_directories)?;
    let skill = alias.join("SKILL.md");
    create_file(&skill, WEBTEST_SKILL.as_bytes())?;
    created_artifacts.push(CreatedArtifact::File(skill));
    Ok(())
}

fn rollback(artifacts: &[CreatedArtifact], directories: &[PathBuf]) {
    for artifact in artifacts.iter().rev() {
        match artifact {
            CreatedArtifact::File(path) => {
                let _ = fs::remove_file(path);
            }
            CreatedArtifact::DirectoryLink(path) => remove_directory_link(path),
        }
    }
    for directory in directories.iter().rev() {
        let _ = fs::remove_dir(directory);
    }
}

#[cfg(unix)]
fn remove_directory_link(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(windows)]
fn remove_directory_link(path: &Path) {
    let _ = fs::remove_dir(path);
}

#[cfg(not(any(unix, windows)))]
fn remove_directory_link(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_skill_contains_the_exact_bootstrap_templates() {
        assert!(WEBTEST_SKILL.contains(PROJECT_CONFIG));
        assert!(WEBTEST_SKILL.contains(EXAMPLE_TEST));
        assert!(WEBTEST_SKILL.contains(FULL_FLOW_EXAMPLE));
        assert!(WEBTEST_SKILL.contains("webtest init ."));
        assert!(WEBTEST_SKILL.contains("webtest describe app.schema"));
        assert!(WEBTEST_SKILL.contains("webtest describe app.configuration"));
        assert!(WEBTEST_SKILL.contains("webtest describe runtime.configuration"));
        assert!(WEBTEST_SKILL.contains("webtest describe app.bridge"));
        assert!(WEBTEST_SKILL.contains("webtest describe app.protocol"));
        assert!(WEBTEST_SKILL.contains("webtest describe app.bridge.example"));
        assert!(WEBTEST_SKILL.contains("webtest describe app.diagnostics"));
        assert!(WEBTEST_SKILL.contains("webtest inspect [<url>] --reporter json"));
        assert!(WEBTEST_SKILL.contains("webtest describe app.echo"));
        assert!(!WEBTEST_SKILL.contains("target/debug/webtest"));
        assert!(!WEBTEST_SKILL.contains("cargo run"));
    }

    #[test]
    fn bootstrap_manifest_is_valid_and_deterministic() {
        let first = manifest_json().expect("manifest");
        let second = manifest_json().expect("manifest");
        assert_eq!(first, second);
        let manifest = AppManifest::from_json(&first).expect("valid manifest");
        let echo = manifest.functions.get("echo").expect("echo operation");
        assert_eq!(echo.returns, TypeSchema::String);
    }

    #[test]
    fn full_flow_skill_example_is_statically_valid_with_its_declared_schema() {
        let required = |ty| FieldSchema {
            ty,
            documentation: String::new(),
            optional: false,
            secret: false,
            default: None,
        };
        let manifest = AppManifest {
            manifest_version: 1,
            protocol: PROTOCOL_VERSION,
            provider: "app".into(),
            sdk: "skill-test".into(),
            sdk_version: "1".into(),
            schema_hash: String::new(),
            functions: BTreeMap::from([(
                "create_user".into(),
                FunctionSchema {
                    documentation: String::new(),
                    retry_safe: false,
                    params: TypeSchema::Object {
                        fields: BTreeMap::from([
                            ("admin".into(), required(TypeSchema::Boolean)),
                            ("email".into(), required(TypeSchema::String)),
                        ]),
                    },
                    returns: TypeSchema::Object {
                        fields: BTreeMap::from([("email".into(), required(TypeSchema::String))]),
                    },
                },
            )]),
        }
        .with_computed_hash()
        .expect("manifest hash");
        manifest.validate().expect("valid manifest");
        let mut providers = webtest_provider::ProviderRegistry::built_in_schemas();
        providers.register_schema(manifest.provider_schema());
        let mut database = webtest_analysis::AnalysisDatabase::with_provider_registry(providers);
        let file = database.open_file("skill-example.webtest", FULL_FLOW_EXAMPLE);
        let diagnostics = database.diagnostics(file).expect("diagnostics");
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    }
}
