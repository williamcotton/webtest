use std::time::Duration;
use webtest_browser::Page;
use webtest_plan::{PlannedStep, TestOperation};
use webtest_provider::ProviderRegistry;

use crate::{RunnerOptions, StepError, assertions::execute_assertion, evaluation::evaluate};

use super::{browser::execute_browser, provider::execute_provider, state::TestExecutionState};

pub(super) async fn execute_step(
    providers: &ProviderRegistry,
    options: &RunnerOptions,
    page: &mut Option<Box<dyn Page>>,
    step: &PlannedStep,
    state: &mut TestExecutionState,
    remaining: Duration,
) -> Result<(), StepError> {
    match &step.operation {
        TestOperation::EvaluatePure(operation) => {
            let value = evaluate(&operation.expression, state.environment())?;
            if let Some(binding) = operation.result_binding {
                state.bind(binding, operation.result_name.as_deref(), value);
            }
            Ok(())
        }
        TestOperation::ServerProviderCall(call) => {
            let value =
                execute_provider(providers, options, call, state.environment(), remaining).await?;
            if let Some(binding) = call.result_binding {
                state.bind(binding, call.result_name.as_deref(), value);
            }
            Ok(())
        }
        TestOperation::Browser(operation) => {
            let page = page.as_deref_mut().ok_or_else(|| {
                StepError::Internal("browser operation has no browser page".into())
            })?;
            execute_browser(page, operation, state.environment(), options, remaining).await
        }
        TestOperation::Assertion(assertion) => {
            execute_assertion(
                page.as_deref_mut(),
                assertion,
                state.environment(),
                options,
                remaining,
            )
            .await
        }
    }
}
