use std::{
    collections::BTreeMap,
    io::{self, Write},
};

use serde::Serialize;
use serde_json::Value;
use webtest_browser::PageSummary;
use webtest_feedback::RepairHint;
use webtest_observation::ValueDiff;
use webtest_project::Project;

use crate::{error::AppError, project_context::normalized_path};

pub(crate) const REPORT_SCHEMA_VERSION: u32 = 2;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunReportOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub infrastructure_error: Option<FailureReport>,
    #[serde(skip)]
    pub events: Vec<EventReport>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiagnosticReport {
    pub diagnostic_schema_version: u32,
    pub repair_hint_schema_version: u32,
    pub severity: String,
    pub code: String,
    pub message: String,
    pub span: SourceSpanReport,
    pub source: MachineSourceReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_details: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repair_hints: Vec<RepairHint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reference_queries: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TestReport {
    pub name: String,
    pub exit_class: ExitClass,
    pub outcome: TestReportOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_nanos: Option<u64>,
    pub duration_nanos: u64,
    pub failure: Option<FailureReport>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TestReportOutcome {
    Passed,
    Failed,
    TimedOut,
    Cancelled,
    Skipped,
    Aborted,
}

impl TestReportOutcome {
    const fn status(self) -> &'static str {
        match self {
            Self::Passed => "ok",
            Self::Failed => "FAILED",
            Self::TimedOut => "TIMED OUT",
            Self::Cancelled => "CANCELLED",
            Self::Skipped => "SKIPPED",
            Self::Aborted => "ABORTED",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunReportOutcome {
    Completed,
    Cancelled,
    Aborted,
}

#[derive(Clone, Debug, Serialize)]
pub struct FailureReport {
    pub diagnostic_schema_version: u32,
    pub repair_hint_schema_version: u32,
    pub code: String,
    pub message: String,
    pub span: Option<SourceSpanReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<ValueDiff>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_details: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repair_hints: Vec<RepairHint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<PageSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secondary: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MachineSourceReport {
    pub path: String,
    pub source_revision: String,
    pub byte_range: ByteRangeReport,
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct ByteRangeReport {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct SourceSpanReport {
    pub line: usize,
    pub column: usize,
    pub source_line: String,
    pub underline_start: usize,
    pub underline_width: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub byte_start: u32,
    pub byte_end: u32,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SummaryReport {
    pub files: usize,
    pub diagnostics: usize,
    pub tests: usize,
    pub passed: usize,
    pub failed: usize,
    pub timed_out: usize,
    pub cancelled: usize,
    pub skipped: usize,
    pub aborted: usize,
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
    pub outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_class: Option<ExitClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_kind: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub arguments: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_schema_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repair_hint_schema_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<ValueDiff>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repair_hints: Vec<RepairHint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<PageSummary>,
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
            schema_version: REPORT_SCHEMA_VERSION,
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
        self.summary.passed = 0;
        self.summary.failed = 0;
        self.summary.timed_out = 0;
        self.summary.cancelled = 0;
        self.summary.skipped = 0;
        self.summary.aborted = 0;
        for test in self.files.iter().flat_map(|file| &file.tests) {
            match test.outcome {
                TestReportOutcome::Passed => self.summary.passed += 1,
                TestReportOutcome::Failed => self.summary.failed += 1,
                TestReportOutcome::TimedOut => self.summary.timed_out += 1,
                TestReportOutcome::Cancelled => self.summary.cancelled += 1,
                TestReportOutcome::Skipped => self.summary.skipped += 1,
                TestReportOutcome::Aborted => self.summary.aborted += 1,
            }
        }
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
        self.write_human_with_status(output, true)
    }

    pub(crate) fn write_human_after_progress(&self, output: &mut dyn Write) -> io::Result<()> {
        self.write_human_with_status(output, false)
    }

    fn write_human_with_status(
        &self,
        output: &mut dyn Write,
        include_test_status: bool,
    ) -> io::Result<()> {
        for warning in &self.warnings {
            writeln!(output, "warning[{}]: {}", warning.code, warning.message)?;
        }
        if self.command == "test" && include_test_status {
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
                if include_test_status {
                    writeln!(output, "test {:?} ... {}", test.name, test.outcome.status())?;
                }
                if let Some(failure) = &test.failure {
                    if !include_test_status {
                        writeln!(output, "failure in test {:?}:", test.name)?;
                    }
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
                "{} passed; {} failed; {} timed out; {} cancelled; {} skipped; {} aborted; {} infrastructure error{}",
                self.summary.passed,
                self.summary.failed,
                self.summary.timed_out,
                self.summary.cancelled,
                self.summary.skipped,
                self.summary.aborted,
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
                    test.outcome.status()
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
            "files={} diagnostics={} tests={} passed={} failed={} timed_out={} cancelled={} skipped={} aborted={} infrastructure={} exit={}",
            self.summary.files,
            self.summary.diagnostics,
            self.summary.tests,
            self.summary.passed,
            self.summary.failed,
            self.summary.timed_out,
            self.summary.cancelled,
            self.summary.skipped,
            self.summary.aborted,
            self.summary.infrastructure_errors,
            self.exit_class.code()
        )
    }

    fn write_events(&self, output: &mut dyn Write) -> io::Result<()> {
        write_json_line(
            output,
            &serde_json::json!({
                "schema_version": REPORT_SCHEMA_VERSION,
                "type": "command_started",
                "command": self.command,
                "project_root": self.project_root,
            }),
        )?;
        for warning in &self.warnings {
            write_json_line(
                output,
                &serde_json::json!({
                    "schema_version": REPORT_SCHEMA_VERSION,
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
                        "schema_version": REPORT_SCHEMA_VERSION,
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
                        "schema_version": REPORT_SCHEMA_VERSION,
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
                "schema_version": REPORT_SCHEMA_VERSION,
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
        let failures = self.summary.failed + self.summary.timed_out + static_cases;
        let errors = self.summary.aborted + self.summary.infrastructure_errors;
        let skipped = self.summary.cancelled + self.summary.skipped;
        writeln!(output, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
        writeln!(
            output,
            "<testsuites tests=\"{tests}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\">"
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
                match test.outcome {
                    TestReportOutcome::Passed => {
                        writeln!(
                            output,
                            "    <testcase name=\"{}\" time=\"{seconds:.9}\" />",
                            xml(&test.name)
                        )?;
                    }
                    TestReportOutcome::Failed | TestReportOutcome::TimedOut => {
                        writeln!(
                            output,
                            "    <testcase name=\"{}\" time=\"{seconds:.9}\">",
                            xml(&test.name)
                        )?;
                        let failure = test.failure.as_ref();
                        let (fallback_code, fallback_message) = match test.outcome {
                            TestReportOutcome::TimedOut => ("test_timeout", "test timed out"),
                            TestReportOutcome::Failed => ("test_failure", "test failed"),
                            TestReportOutcome::Passed
                            | TestReportOutcome::Cancelled
                            | TestReportOutcome::Skipped
                            | TestReportOutcome::Aborted => {
                                unreachable!("failure outcome already matched")
                            }
                        };
                        let code = failure.map_or(fallback_code, |failure| failure.code.as_str());
                        let message =
                            failure.map_or(fallback_message, |failure| failure.message.as_str());
                        writeln!(
                            output,
                            "      <failure type=\"{}\" message=\"{}\">{}</failure>",
                            xml(code),
                            xml(message),
                            xml(message)
                        )?;
                        writeln!(output, "    </testcase>")?;
                    }
                    TestReportOutcome::Cancelled | TestReportOutcome::Skipped => {
                        writeln!(
                            output,
                            "    <testcase name=\"{}\" time=\"{seconds:.9}\">",
                            xml(&test.name)
                        )?;
                        let message = test.reason.as_deref().unwrap_or("test skipped");
                        writeln!(output, "      <skipped message=\"{}\" />", xml(message))?;
                        writeln!(output, "    </testcase>")?;
                    }
                    TestReportOutcome::Aborted => {
                        writeln!(
                            output,
                            "    <testcase name=\"{}\" time=\"{seconds:.9}\">",
                            xml(&test.name)
                        )?;
                        let failure = test.failure.as_ref();
                        let code = failure.map_or("run_aborted", |failure| failure.code.as_str());
                        let message =
                            failure.map_or("test aborted", |failure| failure.message.as_str());
                        writeln!(
                            output,
                            "      <error type=\"{}\" message=\"{}\">{}</error>",
                            xml(code),
                            xml(message),
                            xml(message)
                        )?;
                        writeln!(output, "    </testcase>")?;
                    }
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

pub(crate) fn base_report(command: &str, project: &Project) -> CommandReport {
    let mut report = CommandReport::new(command, normalized_path(&project.root));
    report.warnings = project
        .warnings
        .iter()
        .map(|warning| WarningReport {
            code: "config.unknown".into(),
            key: warning.key.clone(),
            message: warning.message.clone(),
        })
        .collect();
    report
}

pub(crate) fn write_report(report: &CommandReport, reporter: Reporter) -> Result<(), AppError> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    report
        .write(reporter, &mut output)
        .map_err(AppError::infrastructure)
}

pub(crate) fn write_human_report_after_progress(report: &CommandReport) -> Result<(), AppError> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    report
        .write_human_after_progress(&mut output)
        .map_err(AppError::infrastructure)
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
    use webtest_hir::TestId;
    use webtest_observation::{
        ExecutionEvent, ExecutionId, RunOutcomeKind, SkipReason, TestOutcomeKind,
    };

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
                outcome: TestReportOutcome::Failed,
                reason: None,
                timeout_nanos: None,
                duration_nanos: 10,
                failure: Some(FailureReport {
                    diagnostic_schema_version: webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
                    repair_hint_schema_version: webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
                    code: "runtime.locator_not_found".into(),
                    message: "missing & gone".into(),
                    span: Some(SourceSpanReport {
                        line: 2,
                        column: 7,
                        source_line: "click id(\"missing\")".into(),
                        underline_start: 6,
                        underline_width: 13,
                        end_line: 2,
                        end_column: 20,
                        byte_start: 6,
                        byte_end: 19,
                    }),
                    diff: None,
                    artifacts: Vec::new(),
                    semantic_details: None,
                    repair_hints: Vec::new(),
                    page: None,
                    secondary: Vec::new(),
                }),
            }],
            outcome: Some(RunReportOutcome::Completed),
            reason: None,
            infrastructure_error: None,
            events: Vec::new(),
        });
        report.exit_class = ExitClass::TestFailure;
        report.finish();
        report
    }

    fn cancelled_sample() -> CommandReport {
        let execution_id = ExecutionId(7);
        let events = crate::runtime_output::event_reports(
            "tests/cancel.webtest",
            &[
                ExecutionEvent::RunStarted { execution_id },
                ExecutionEvent::TestStarted {
                    execution_id,
                    test_id: TestId(0),
                    name: "active".into(),
                },
                ExecutionEvent::TestFinished {
                    execution_id,
                    test_id: TestId(0),
                    outcome: TestOutcomeKind::Cancelled,
                },
                ExecutionEvent::TestSkipped {
                    execution_id,
                    test_id: TestId(1),
                    name: "later".into(),
                    reason: SkipReason::RunCancelled,
                },
                ExecutionEvent::RunFinished {
                    execution_id,
                    outcome: RunOutcomeKind::Cancelled,
                },
            ],
        );
        let mut report = CommandReport::new("test", "/project");
        report.files.push(FileReport {
            path: "tests/cancel.webtest".into(),
            exit_class: ExitClass::TestFailure,
            source_revision: "cancelled".into(),
            duration_nanos: 5,
            diagnostics: Vec::new(),
            tests: vec![
                TestReport {
                    name: "active".into(),
                    exit_class: ExitClass::TestFailure,
                    outcome: TestReportOutcome::Cancelled,
                    reason: Some("requested".into()),
                    timeout_nanos: None,
                    duration_nanos: 5,
                    failure: None,
                },
                TestReport {
                    name: "later".into(),
                    exit_class: ExitClass::TestFailure,
                    outcome: TestReportOutcome::Skipped,
                    reason: Some("run_cancelled".into()),
                    timeout_nanos: None,
                    duration_nanos: 0,
                    failure: None,
                },
            ],
            outcome: Some(RunReportOutcome::Cancelled),
            reason: Some("requested".into()),
            infrastructure_error: None,
            events,
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
    fn human_output_after_live_progress_keeps_details_without_repeating_status() {
        let report = sample();
        let mut output = Vec::new();
        report
            .write_human_after_progress(&mut output)
            .expect("render");
        let output = String::from_utf8(output).expect("UTF-8");
        assert!(!output.contains("running 1 test"));
        assert!(!output.contains("test \"a < b\" ... FAILED"));
        assert!(output.contains("failure in test \"a < b\":"));
        assert!(output.contains("error[runtime.locator_not_found]"));
        assert!(output.ends_with(
            "0 passed; 1 failed; 0 timed out; 0 cancelled; 0 skipped; 0 aborted; 0 infrastructure errors\n"
        ));
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
        assert_eq!(value["schema_version"], REPORT_SCHEMA_VERSION);
        assert_eq!(value["exit_class"], "test_failure");
        assert_eq!(
            String::from_utf8(json).expect("UTF-8 JSON"),
            include_str!("../tests/fixtures/report-v2.json")
        );

        let mut events = Vec::new();
        sample()
            .write(Reporter::Events, &mut events)
            .expect("events");
        let events = String::from_utf8(events).expect("UTF-8 events");
        for line in events.lines() {
            let value: serde_json::Value = serde_json::from_str(line).expect("json line");
            assert_eq!(value["schema_version"], REPORT_SCHEMA_VERSION);
        }
        assert_eq!(events, include_str!("../tests/fixtures/report-v2.jsonl"));
    }

    #[test]
    fn junit_escapes_values_and_distinguishes_failures() {
        let mut output = Vec::new();
        sample().write(Reporter::Junit, &mut output).expect("junit");
        let output = String::from_utf8(output).expect("utf8");
        assert!(output.contains("name=\"a &lt; b\""));
        assert!(output.contains("missing &amp; gone"));
        assert!(output.contains("<failure"));
        assert_eq!(output, include_str!("../tests/fixtures/report-v2.xml"));
    }

    #[test]
    fn cancellation_is_golden_across_every_reporter_and_never_serializes_passed() {
        for (reporter, expected) in [
            (
                Reporter::Human,
                include_str!("../tests/fixtures/cancellation-v2.txt"),
            ),
            (
                Reporter::Concise,
                include_str!("../tests/fixtures/cancellation-concise-v2.txt"),
            ),
            (
                Reporter::Json,
                include_str!("../tests/fixtures/cancellation-v2.json"),
            ),
            (
                Reporter::Events,
                include_str!("../tests/fixtures/cancellation-v2.jsonl"),
            ),
            (
                Reporter::Junit,
                include_str!("../tests/fixtures/cancellation-v2.xml"),
            ),
        ] {
            let mut output = Vec::new();
            cancelled_sample()
                .write(reporter, &mut output)
                .expect("cancellation report");
            let output = String::from_utf8(output).expect("UTF-8");
            assert_eq!(output, expected);
            assert!(!output.contains("\"passed\": true"));
            assert!(!output.contains("\"passed\":true"));
            assert!(!output.contains("... ok"));
        }
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
