use std::io::{self, Write};

use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitClass {
    #[default]
    Success,
    TestFailure,
    Usage,
    Infrastructure,
    Internal,
}

impl ExitClass {
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::TestFailure => 1,
            Self::Usage => 2,
            Self::Infrastructure => 3,
            Self::Internal => 4,
        }
    }

    pub const fn combine(self, other: Self) -> Self {
        if other.code() > self.code() {
            other
        } else {
            self
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CommandReport {
    pub schema_version: u32,
    pub command: String,
    pub project_root: String,
    pub warnings: Vec<WarningReport>,
    pub files: Vec<FileReport>,
    pub summary: SummaryReport,
    pub exit_class: ExitClass,
}

#[derive(Clone, Debug, Serialize)]
pub struct WarningReport {
    pub code: String,
    pub key: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileReport {
    pub path: String,
    pub exit_class: ExitClass,
    pub source_revision: String,
    pub duration_nanos: u64,
    pub diagnostics: Vec<DiagnosticReport>,
    pub tests: Vec<TestReport>,
    pub infrastructure_error: Option<FailureReport>,
    #[serde(skip)]
    pub events: Vec<EventReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiagnosticReport {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub span: SourceSpanReport,
}

#[derive(Clone, Debug, Serialize)]
pub struct TestReport {
    pub name: String,
    pub exit_class: ExitClass,
    pub passed: bool,
    pub duration_nanos: u64,
    pub failure: Option<FailureReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FailureReport {
    pub code: String,
    pub message: String,
    pub span: Option<SourceSpanReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SourceSpanReport {
    pub line: usize,
    pub column: usize,
    pub source_line: String,
    pub underline_start: usize,
    pub underline_width: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SummaryReport {
    pub files: usize,
    pub diagnostics: usize,
    pub tests: usize,
    pub passed: usize,
    pub failed: usize,
    pub infrastructure_errors: usize,
    pub duration_nanos: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct EventReport {
    pub schema_version: u32,
    #[serde(rename = "type")]
    pub kind: String,
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_class: Option<ExitClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reporter {
    Human,
    Concise,
    Json,
    Junit,
    Events,
}

impl CommandReport {
    pub fn new(command: impl Into<String>, project_root: impl Into<String>) -> Self {
        Self {
            schema_version: 1,
            command: command.into(),
            project_root: project_root.into(),
            warnings: Vec::new(),
            files: Vec::new(),
            summary: SummaryReport::default(),
            exit_class: ExitClass::Success,
        }
    }

    pub fn finish(&mut self) {
        self.summary.files = self.files.len();
        self.summary.diagnostics = self.files.iter().map(|file| file.diagnostics.len()).sum();
        self.summary.tests = self.files.iter().map(|file| file.tests.len()).sum();
        self.summary.passed = self
            .files
            .iter()
            .flat_map(|file| &file.tests)
            .filter(|test| test.passed)
            .count();
        self.summary.failed = self.summary.tests - self.summary.passed;
        self.summary.infrastructure_errors = self
            .files
            .iter()
            .filter(|file| file.infrastructure_error.is_some())
            .count();
        self.summary.duration_nanos = self.files.iter().map(|file| file.duration_nanos).sum();
    }

    pub fn write(&self, reporter: Reporter, output: &mut dyn Write) -> io::Result<()> {
        match reporter {
            Reporter::Human => self.write_human(output),
            Reporter::Concise => self.write_concise(output),
            Reporter::Json => {
                serde_json::to_writer_pretty(&mut *output, self)?;
                writeln!(output)
            }
            Reporter::Junit => self.write_junit(output),
            Reporter::Events => self.write_events(output),
        }
    }

    fn write_human(&self, output: &mut dyn Write) -> io::Result<()> {
        for warning in &self.warnings {
            writeln!(output, "warning[{}]: {}", warning.code, warning.message)?;
        }
        if self.command == "test" {
            writeln!(
                output,
                "running {} test{} in {} file{}",
                self.summary.tests,
                plural(self.summary.tests),
                self.summary.files,
                plural(self.summary.files)
            )?;
        }
        for file in &self.files {
            for diagnostic in &file.diagnostics {
                write_source_diagnostic(
                    output,
                    &file.path,
                    &diagnostic.severity,
                    &diagnostic.code,
                    &diagnostic.message,
                    &diagnostic.span,
                )?;
            }
            for test in &file.tests {
                writeln!(
                    output,
                    "test {:?} ... {}",
                    test.name,
                    if test.passed { "ok" } else { "FAILED" }
                )?;
                if let Some(failure) = &test.failure {
                    if let Some(span) = &failure.span {
                        write_source_diagnostic(
                            output,
                            &file.path,
                            "error",
                            &failure.code,
                            &failure.message,
                            span,
                        )?;
                    } else {
                        writeln!(output, "error[{}]: {}", failure.code, failure.message)?;
                    }
                    for artifact in &failure.artifacts {
                        writeln!(output, "  evidence: {artifact}")?;
                    }
                }
            }
            if let Some(error) = &file.infrastructure_error {
                writeln!(
                    output,
                    "{}: infrastructure error[{}]: {}",
                    file.path, error.code, error.message
                )?;
            }
        }
        match self.command.as_str() {
            "test" => writeln!(
                output,
                "{} passed; {} failed; {} infrastructure error{}",
                self.summary.passed,
                self.summary.failed,
                self.summary.infrastructure_errors,
                plural(self.summary.infrastructure_errors)
            ),
            "check" => writeln!(
                output,
                "checked {} file{}; {} diagnostic{}",
                self.summary.files,
                plural(self.summary.files),
                self.summary.diagnostics,
                plural(self.summary.diagnostics)
            ),
            _ => Ok(()),
        }
    }

    fn write_concise(&self, output: &mut dyn Write) -> io::Result<()> {
        for warning in &self.warnings {
            writeln!(output, "warning[{}] {}", warning.code, warning.message)?;
        }
        for file in &self.files {
            for diagnostic in &file.diagnostics {
                writeln!(
                    output,
                    "{}:{}:{}: {}[{}]: {}",
                    file.path,
                    diagnostic.span.line,
                    diagnostic.span.column,
                    diagnostic.severity,
                    diagnostic.code,
                    diagnostic.message
                )?;
            }
            for test in &file.tests {
                writeln!(
                    output,
                    "{}: test {:?}: {}",
                    file.path,
                    test.name,
                    if test.passed { "ok" } else { "failed" }
                )?;
            }
            if let Some(error) = &file.infrastructure_error {
                writeln!(
                    output,
                    "{}: infrastructure[{}]: {}",
                    file.path, error.code, error.message
                )?;
            }
        }
        writeln!(
            output,
            "files={} diagnostics={} tests={} passed={} failed={} infrastructure={} exit={}",
            self.summary.files,
            self.summary.diagnostics,
            self.summary.tests,
            self.summary.passed,
            self.summary.failed,
            self.summary.infrastructure_errors,
            self.exit_class.code()
        )
    }

    fn write_events(&self, output: &mut dyn Write) -> io::Result<()> {
        write_json_line(
            output,
            &serde_json::json!({
                "schema_version": 1,
                "type": "command_started",
                "command": self.command,
                "project_root": self.project_root,
            }),
        )?;
        for warning in &self.warnings {
            write_json_line(
                output,
                &serde_json::json!({
                    "schema_version": 1,
                    "type": "warning",
                    "code": warning.code,
                    "key": warning.key,
                    "message": warning.message,
                }),
            )?;
        }
        for file in &self.files {
            for diagnostic in &file.diagnostics {
                write_json_line(
                    output,
                    &serde_json::json!({
                        "schema_version": 1,
                        "type": "diagnostic",
                        "file": file.path,
                        "source_revision": file.source_revision,
                        "diagnostic": diagnostic,
                    }),
                )?;
            }
            for event in &file.events {
                write_json_line(output, event)?;
            }
            if let Some(error) = &file.infrastructure_error {
                write_json_line(
                    output,
                    &serde_json::json!({
                        "schema_version": 1,
                        "type": "infrastructure_error",
                        "file": file.path,
                        "failure": error,
                    }),
                )?;
            }
        }
        write_json_line(
            output,
            &serde_json::json!({
                "schema_version": 1,
                "type": "command_finished",
                "command": self.command,
                "summary": self.summary,
                "exit_class": self.exit_class,
                "exit_code": self.exit_class.code(),
            }),
        )
    }

    fn write_junit(&self, output: &mut dyn Write) -> io::Result<()> {
        let static_cases: usize = self
            .files
            .iter()
            .filter(|file| !file.diagnostics.is_empty() && file.tests.is_empty())
            .count();
        let tests = self.summary.tests + static_cases + self.summary.infrastructure_errors;
        let failures = self.summary.failed + static_cases;
        writeln!(output, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
        writeln!(
            output,
            "<testsuites tests=\"{tests}\" failures=\"{failures}\" errors=\"{}\">",
            self.summary.infrastructure_errors
        )?;
        for file in &self.files {
            writeln!(output, "  <testsuite name=\"{}\">", xml(&file.path))?;
            if !file.diagnostics.is_empty() && file.tests.is_empty() {
                let message = file
                    .diagnostics
                    .iter()
                    .map(|diagnostic| format!("[{}] {}", diagnostic.code, diagnostic.message))
                    .collect::<Vec<_>>()
                    .join("\n");
                writeln!(output, "    <testcase name=\"static check\">")?;
                writeln!(
                    output,
                    "      <failure type=\"static\" message=\"{}\">{}</failure>",
                    xml(&message),
                    xml(&message)
                )?;
                writeln!(output, "    </testcase>")?;
            }
            for test in &file.tests {
                let seconds = test.duration_nanos as f64 / 1_000_000_000.0;
                if test.passed {
                    writeln!(
                        output,
                        "    <testcase name=\"{}\" time=\"{seconds:.9}\" />",
                        xml(&test.name)
                    )?;
                } else {
                    writeln!(
                        output,
                        "    <testcase name=\"{}\" time=\"{seconds:.9}\">",
                        xml(&test.name)
                    )?;
                    let failure = test.failure.as_ref();
                    let code = failure.map_or("test_failure", |failure| failure.code.as_str());
                    let message = failure.map_or("test failed", |failure| failure.message.as_str());
                    writeln!(
                        output,
                        "      <failure type=\"{}\" message=\"{}\">{}</failure>",
                        xml(code),
                        xml(message),
                        xml(message)
                    )?;
                    writeln!(output, "    </testcase>")?;
                }
            }
            if let Some(error) = &file.infrastructure_error {
                writeln!(output, "    <testcase name=\"infrastructure\">")?;
                writeln!(
                    output,
                    "      <error type=\"{}\" message=\"{}\">{}</error>",
                    xml(&error.code),
                    xml(&error.message),
                    xml(&error.message)
                )?;
                writeln!(output, "    </testcase>")?;
            }
            writeln!(output, "  </testsuite>")?;
        }
        writeln!(output, "</testsuites>")
    }
}

fn write_source_diagnostic(
    output: &mut dyn Write,
    path: &str,
    severity: &str,
    code: &str,
    message: &str,
    span: &SourceSpanReport,
) -> io::Result<()> {
    writeln!(
        output,
        "{path}:{}:{}: {severity}[{code}]",
        span.line, span.column
    )?;
    writeln!(output, "    {message}")?;
    writeln!(output, "    |")?;
    writeln!(output, "{:>3} | {}", span.line, span.source_line)?;
    writeln!(
        output,
        "    | {}{}",
        " ".repeat(span.underline_start),
        "^".repeat(span.underline_width.max(1))
    )
}

fn write_json_line(output: &mut dyn Write, value: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *output, value)?;
    writeln!(output)
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CommandReport {
        let mut report = CommandReport::new("test", "/project");
        report.files.push(FileReport {
            path: "tests/a.webtest".into(),
            exit_class: ExitClass::TestFailure,
            source_revision: "abc".into(),
            duration_nanos: 12,
            diagnostics: Vec::new(),
            tests: vec![TestReport {
                name: "a < b".into(),
                exit_class: ExitClass::TestFailure,
                passed: false,
                duration_nanos: 10,
                failure: Some(FailureReport {
                    code: "runtime.locator_not_found".into(),
                    message: "missing & gone".into(),
                    span: Some(SourceSpanReport {
                        line: 2,
                        column: 7,
                        source_line: "click id(\"missing\")".into(),
                        underline_start: 6,
                        underline_width: 13,
                    }),
                    artifacts: Vec::new(),
                }),
            }],
            infrastructure_error: None,
            events: Vec::new(),
        });
        report.exit_class = ExitClass::TestFailure;
        report.finish();
        report
    }

    #[test]
    fn human_output_has_source_snippet_and_precise_underline() {
        let mut output = Vec::new();
        sample()
            .write(Reporter::Human, &mut output)
            .expect("render");
        let output = String::from_utf8(output).expect("utf8");
        assert!(output.contains("tests/a.webtest:2:7"));
        assert!(output.contains("2 | click id(\"missing\")"));
        assert!(output.contains("      ^^^^^^^^^^^^^"));
    }

    #[test]
    fn failure_artifact_links_are_in_human_and_json_reports() {
        let mut report = sample();
        report.files[0].tests[0]
            .failure
            .as_mut()
            .expect("failure")
            .artifacts = vec!["/project/.webtest/artifacts/test-0-step-1-execution-2.png".into()];
        let mut human = Vec::new();
        report
            .write(Reporter::Human, &mut human)
            .expect("human report");
        assert!(
            String::from_utf8(human)
                .expect("UTF-8")
                .contains("evidence: /project/.webtest/artifacts")
        );
        let mut json = Vec::new();
        report
            .write(Reporter::Json, &mut json)
            .expect("JSON report");
        let value: serde_json::Value = serde_json::from_slice(&json).expect("JSON");
        assert_eq!(
            value["files"][0]["tests"][0]["failure"]["artifacts"][0],
            "/project/.webtest/artifacts/test-0-step-1-execution-2.png"
        );
    }

    #[test]
    fn json_and_events_are_versioned() {
        let mut json = Vec::new();
        sample().write(Reporter::Json, &mut json).expect("json");
        let value: serde_json::Value = serde_json::from_slice(&json).expect("valid json");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["exit_class"], "test_failure");
        assert_eq!(
            String::from_utf8(json).expect("UTF-8 JSON"),
            include_str!("../tests/fixtures/report-v1.json")
        );

        let mut events = Vec::new();
        sample()
            .write(Reporter::Events, &mut events)
            .expect("events");
        let events = String::from_utf8(events).expect("UTF-8 events");
        for line in events.lines() {
            let value: serde_json::Value = serde_json::from_str(line).expect("json line");
            assert_eq!(value["schema_version"], 1);
        }
        assert_eq!(events, include_str!("../tests/fixtures/report-v1.jsonl"));
    }

    #[test]
    fn junit_escapes_values_and_distinguishes_failures() {
        let mut output = Vec::new();
        sample().write(Reporter::Junit, &mut output).expect("junit");
        let output = String::from_utf8(output).expect("utf8");
        assert!(output.contains("name=\"a &lt; b\""));
        assert!(output.contains("missing &amp; gone"));
        assert!(output.contains("<failure"));
        assert_eq!(output, include_str!("../tests/fixtures/report-v1.xml"));
    }

    #[test]
    fn exit_classes_combine_by_documented_priority() {
        assert_eq!(
            ExitClass::TestFailure.combine(ExitClass::Infrastructure),
            ExitClass::Infrastructure
        );
        assert_eq!(
            ExitClass::Internal.combine(ExitClass::Usage),
            ExitClass::Internal
        );
    }
}
