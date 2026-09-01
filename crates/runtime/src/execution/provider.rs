use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

use webtest_hir::BindingId;
use webtest_plan::ServerProviderCall;
use webtest_provider::{
    CallContext, OperationName, ProviderCall, ProviderName, ProviderRegistry, Value,
};

use crate::{RunnerOptions, StepError, evaluation::evaluate};

pub(super) async fn execute_provider(
    providers: &ProviderRegistry,
    options: &RunnerOptions,
    call: &ServerProviderCall,
    environment: &HashMap<BindingId, Value>,
    remaining: Duration,
) -> Result<Value, StepError> {
    let mut arguments = BTreeMap::new();
    for (name, expression) in &call.arguments {
        arguments.insert(name.clone(), evaluate(expression, environment)?);
    }
    let result = providers
        .call(
            ProviderCall {
                provider: ProviderName(call.provider.clone()),
                operation: OperationName(call.operation.clone()),
                arguments,
                schema_hash: call.schema_hash.clone(),
            },
            CallContext {
                project_root: options.project_root.clone(),
                timeout: call
                    .timeout
                    .unwrap_or(options.provider_call_timeout)
                    .min(remaining),
                redacted_json_fields: options.redacted_json_fields.clone(),
            },
        )
        .await
        .map_err(StepError::Provider)?;
    Ok(result.value)
}
