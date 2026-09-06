use std::time::Duration;
use webtest_browser::Page;
use webtest_plan::{PlannedStep, TestOperation};
use webtest_provider::ProviderRegistry;

use crate::{RunnerOptions, StepError, assertions::execute_assertion, evaluation::evaluate};

use super::{browser::execute_browser, provider::execute_provider, state::TestExecutionState};

pub(super) enum StepCompletion {
    Completed,
    Cancelled,
}

pub(super) async fn execute_step(
    providers: &ProviderRegistry,
    options: &RunnerOptions,
    page: &mut Option<Box<dyn Page>>,
    step: &PlannedStep,
    state: &mut TestExecutionState,
    remaining: Duration,
    context: std::sync::Arc<dyn webtest_host::OperationContext>,
) -> Result<StepCompletion, StepError> {
    if let Some(page) = page.as_deref_mut() {
        page.set_operation_context(Some(context.clone()));
    }
    match &step.operation {
        TestOperation::EvaluatePure(operation) => {
            let value = evaluate(&operation.expression, state.environment())?;
            if let Some(binding) = operation.result_binding {
                state.bind(binding, operation.result_name.as_deref(), value);
            }
            Ok(StepCompletion::Completed)
        }
        TestOperation::ServerProviderCall(call) => {
            let value = execute_provider(
                providers,
                options,
                call,
                state.environment(),
                remaining,
                context,
            )
            .await?;
            state.accept_provider_resources(&value);
            if let Some(binding) = call.result_binding {
                state.bind(binding, call.result_name.as_deref(), value);
            }
            Ok(StepCompletion::Completed)
        }
        TestOperation::Browser(operation) => {
            let page = page.as_deref_mut().ok_or_else(|| {
                StepError::Internal("browser operation has no browser page".into())
            })?;
            tokio::select! {
                biased;
                _ = context.cancelled() => Ok(StepCompletion::Cancelled),
                result = execute_browser(page, operation, state.environment(), options, remaining) => result.map(|()| StepCompletion::Completed),
            }
        }
        TestOperation::Assertion(assertion) => {
            let assertion = execute_assertion(
                page.as_deref_mut(),
                assertion,
                state.environment(),
                options,
                remaining,
            );
            tokio::select! {
                biased;
                _ = context.cancelled() => Ok(StepCompletion::Cancelled),
                result = assertion => result.map(|()| StepCompletion::Completed),
            }
        }
    }
}
