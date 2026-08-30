use std::{sync::Arc, time::Instant};

use tracing::instrument;
use webtest_browser::BrowserHost;
use webtest_observation::{ExecutionEvent, ExecutionId, ObservationStore};
use webtest_plan::TestPlan;
use webtest_provider::{Capability, NativeProviderConfig, ProviderRegistry};

use crate::{
    RunControl, RunError, RunResult, RunnerOptions,
    execution::{ExecutedTest, execute_test},
};

pub struct Runner {
    observations: Arc<ObservationStore>,
    options: RunnerOptions,
    providers: ProviderRegistry,
}

impl Runner {
    pub fn new(observations: Arc<ObservationStore>) -> Self {
        Self {
            observations,
            options: RunnerOptions::default(),
            providers: ProviderRegistry::built_in(NativeProviderConfig::default()),
        }
    }

    pub fn with_options(mut self, options: RunnerOptions) -> Self {
        self.providers = ProviderRegistry::built_in(options.provider_config.clone());
        self.options = options;
        self
    }

    pub fn with_provider_registry(mut self, providers: ProviderRegistry) -> Self {
        self.providers = providers;
        self
    }

    #[instrument(skip_all, fields(file = plan.file.get()))]
    pub async fn run(
        &self,
        plan: &TestPlan,
        browser: &dyn BrowserHost,
    ) -> Result<RunResult, RunError> {
        self.run_with_control(plan, browser, None).await
    }

    #[instrument(skip_all, fields(file = plan.file.get()))]
    pub async fn run_with_control(
        &self,
        plan: &TestPlan,
        browser: &dyn BrowserHost,
        control: Option<&dyn RunControl>,
    ) -> Result<RunResult, RunError> {
        self.observations.clear_for_file(plan.file);
        let run_started = Instant::now();
        let execution_id = ExecutionId::next();
        let mut events = vec![ExecutionEvent::RunStarted { execution_id }];
        let needs_browser = plan
            .required_host_capabilities
            .contains(&Capability::Browser);
        let mut session = if needs_browser {
            Some(browser.start().await?)
        } else {
            None
        };
        let mut tests = Vec::with_capacity(plan.tests.len());

        for (index, test) in plan.tests.iter().enumerate() {
            if control.is_some_and(RunControl::is_cancelled) {
                break;
            }
            let ExecutedTest {
                result,
                session_tainted,
            } = execute_test(
                plan,
                test,
                execution_id,
                &mut events,
                &mut session,
                control,
                &self.options,
                &self.providers,
                &self.observations,
            )
            .await?;
            tests.push(result);
            if session_tainted && index + 1 < plan.tests.len() {
                if let Some(mut current) = session.take() {
                    let _ = current.close().await;
                }
                session = Some(browser.start().await?);
            }
        }

        events.push(ExecutionEvent::RunFinished { execution_id });
        if let Some(mut session) = session {
            session.close().await?;
        }
        Ok(RunResult {
            execution_id,
            tests,
            events,
            duration: run_started.elapsed(),
        })
    }
}
