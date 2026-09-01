use std::{
    io::{self, Write},
    sync::Mutex,
};

use webtest_observation::{ExecutionEvent, TestOutcomeKind};
use webtest_runtime::RunEventSink;

pub(crate) struct HumanTestProgress {
    inner: Mutex<ProgressState>,
}

struct ProgressState {
    output: Box<dyn Write + Send>,
    pending: PendingLine,
    browser_active: bool,
    error: Option<(io::ErrorKind, String)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PendingLine {
    #[default]
    None,
    Stage,
    BrowserStarting,
    Test,
    BrowserStopping,
}

impl HumanTestProgress {
    pub(crate) fn stdout() -> Self {
        Self::new(Box::new(io::stdout()))
    }

    fn new(output: Box<dyn Write + Send>) -> Self {
        Self {
            inner: Mutex::new(ProgressState {
                output,
                pending: PendingLine::None,
                browser_active: false,
                error: None,
            }),
        }
    }

    pub(crate) fn checking(&self, files: usize) -> io::Result<()> {
        self.write(|state| {
            write!(
                state.output,
                "checking {files} test file{} ... ",
                plural(files)
            )?;
            state.pending = PendingLine::Stage;
            Ok(())
        })
    }

    pub(crate) fn checked(&self, files_with_errors: usize) -> io::Result<()> {
        self.write(|state| {
            if files_with_errors == 0 {
                writeln!(state.output, "done")?;
            } else {
                writeln!(
                    state.output,
                    "done ({files_with_errors} file{} with static errors)",
                    plural(files_with_errors)
                )?;
            }
            state.pending = PendingLine::None;
            Ok(())
        })
    }

    pub(crate) fn checking_failed(&self) -> io::Result<()> {
        self.finish_stage("FAILED")
    }

    pub(crate) fn running(&self, tests: usize, files: usize) -> io::Result<()> {
        self.write(|state| {
            writeln!(
                state.output,
                "running {tests} test{} in {files} file{}",
                plural(tests),
                plural(files)
            )
        })
    }

    pub(crate) fn starting_application(&self, message: &str) -> io::Result<()> {
        self.write(|state| {
            write!(state.output, "{message} ... ")?;
            state.pending = PendingLine::Stage;
            Ok(())
        })
    }

    pub(crate) fn application_started(&self, ready: bool) -> io::Result<()> {
        self.finish_stage(if ready { "ready" } else { "FAILED" })
    }

    pub(crate) fn stopping_application(&self) -> io::Result<()> {
        self.write(|state| {
            write!(state.output, "stopping application ... ")?;
            state.pending = PendingLine::Stage;
            Ok(())
        })
    }

    pub(crate) fn application_stopped(&self, succeeded: bool) -> io::Result<()> {
        self.finish_stage(if succeeded { "done" } else { "FAILED" })
    }

    pub(crate) fn starting_browser(&self, path: &str, headed: bool) -> io::Result<()> {
        self.write(|state| {
            write!(
                state.output,
                "starting Chrome for {path} ({}) ... ",
                if headed { "headed" } else { "headless" }
            )?;
            state.pending = PendingLine::BrowserStarting;
            state.browser_active = true;
            Ok(())
        })
    }

    pub(crate) fn browser_run_finished(&self, succeeded: bool) -> io::Result<()> {
        self.write(|state| {
            if succeeded {
                if state.pending == PendingLine::BrowserStopping {
                    writeln!(state.output, "done")?;
                    state.pending = PendingLine::None;
                }
            } else {
                let status = match state.pending {
                    PendingLine::Test => "ERROR",
                    PendingLine::None => return Ok(()),
                    _ => "FAILED",
                };
                writeln!(state.output, "{status}")?;
                state.pending = PendingLine::None;
            }
            state.browser_active = false;
            Ok(())
        })
    }

    pub(crate) fn check_error(&self) -> io::Result<()> {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match state.error.take() {
            Some((kind, message)) => Err(io::Error::new(kind, message)),
            None => Ok(()),
        }
    }

    fn finish_stage(&self, status: &str) -> io::Result<()> {
        self.write(|state| {
            writeln!(state.output, "{status}")?;
            state.pending = PendingLine::None;
            Ok(())
        })
    }

    fn write(
        &self,
        operation: impl FnOnce(&mut ProgressState) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((kind, message)) = &state.error {
            return Err(io::Error::new(*kind, message.clone()));
        }
        if let Err(error) = operation(&mut state).and_then(|()| state.output.flush()) {
            state.error = Some((error.kind(), error.to_string()));
            return Err(error);
        }
        Ok(())
    }

    fn publish_event(&self, event: &ExecutionEvent) -> io::Result<()> {
        self.write(|state| {
            match event {
                ExecutionEvent::TestStarted { name, .. } => {
                    if state.pending == PendingLine::BrowserStarting {
                        writeln!(state.output, "ready")?;
                    }
                    write!(state.output, "test {name:?} ... ")?;
                    state.pending = PendingLine::Test;
                }
                ExecutionEvent::TestFinished { outcome, .. }
                    if state.pending == PendingLine::Test =>
                {
                    let status = match outcome {
                        TestOutcomeKind::Passed => "ok",
                        TestOutcomeKind::Failed => "FAILED",
                        TestOutcomeKind::TimedOut => "TIMED OUT",
                        TestOutcomeKind::Cancelled => "CANCELLED",
                        TestOutcomeKind::Aborted => "ABORTED",
                    };
                    writeln!(state.output, "{status}")?;
                    state.pending = PendingLine::None;
                }
                ExecutionEvent::TestSkipped { name, .. } => {
                    if state.pending == PendingLine::BrowserStarting {
                        writeln!(state.output, "ready")?;
                    }
                    writeln!(state.output, "test {name:?} ... SKIPPED")?;
                    state.pending = PendingLine::None;
                }
                ExecutionEvent::RunFinished { .. } if state.browser_active => {
                    if state.pending == PendingLine::BrowserStarting {
                        writeln!(state.output, "ready")?;
                    }
                    write!(state.output, "stopping Chrome ... ")?;
                    state.pending = PendingLine::BrowserStopping;
                }
                ExecutionEvent::TestFinished { .. } | ExecutionEvent::RunFinished { .. } => {}
                ExecutionEvent::RunStarted { .. }
                | ExecutionEvent::StepStarted { .. }
                | ExecutionEvent::StepPassed { .. }
                | ExecutionEvent::ProviderCallStarted { .. }
                | ExecutionEvent::ProviderCallFinished { .. }
                | ExecutionEvent::ProviderCallFailed { .. }
                | ExecutionEvent::StepFailed { .. }
                | ExecutionEvent::TestTimedOut { .. }
                | ExecutionEvent::CleanupFailed { .. } => {}
            }
            Ok(())
        })
    }
}

impl RunEventSink for HumanTestProgress {
    fn publish(&self, event: &ExecutionEvent) {
        let _ = self.publish_event(event);
    }
}

const fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use webtest_hir::TestId;
    use webtest_observation::ExecutionId;

    use super::*;

    #[derive(Clone, Default)]
    struct SharedOutput(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn progress_renders_observable_lifecycle_stages_in_order() {
        let output = SharedOutput::default();
        let progress = HumanTestProgress::new(Box::new(output.clone()));
        let execution_id = ExecutionId(7);

        progress.checking(2).expect("checking");
        progress.checked(0).expect("checked");
        progress.running(1, 2).expect("running");
        progress
            .starting_application("starting application and verifying app bridge")
            .expect("application");
        progress.application_started(true).expect("ready");
        progress
            .starting_browser("tests/login.webtest", false)
            .expect("browser");
        progress.publish(&ExecutionEvent::RunStarted { execution_id });
        progress.publish(&ExecutionEvent::TestStarted {
            execution_id,
            test_id: TestId(3),
            name: "signs in".into(),
        });
        progress.publish(&ExecutionEvent::TestFinished {
            execution_id,
            test_id: TestId(3),
            outcome: TestOutcomeKind::Passed,
            failure_class: None,
        });
        progress.publish(&ExecutionEvent::RunFinished {
            execution_id,
            outcome: webtest_observation::RunOutcomeKind::Completed,
            failure_class: None,
        });
        progress.browser_run_finished(true).expect("browser done");
        progress.stopping_application().expect("stopping app");
        progress.application_stopped(true).expect("app stopped");

        let bytes = output
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert_eq!(
            String::from_utf8(bytes).expect("UTF-8"),
            concat!(
                "checking 2 test files ... done\n",
                "running 1 test in 2 files\n",
                "starting application and verifying app bridge ... ready\n",
                "starting Chrome for tests/login.webtest (headless) ... ready\n",
                "test \"signs in\" ... ok\n",
                "stopping Chrome ... done\n",
                "stopping application ... done\n",
            )
        );
    }

    #[test]
    fn infrastructure_failure_closes_the_active_progress_line() {
        let output = SharedOutput::default();
        let progress = HumanTestProgress::new(Box::new(output.clone()));
        progress
            .starting_browser("tests/login.webtest", false)
            .expect("browser");
        progress.browser_run_finished(false).expect("failed");

        let bytes = output
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert_eq!(
            String::from_utf8(bytes).expect("UTF-8"),
            "starting Chrome for tests/login.webtest (headless) ... FAILED\n"
        );
    }
}
