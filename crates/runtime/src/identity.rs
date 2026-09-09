//! Native identity allocation is an injectable service, not branch state.
use webtest_observation::ExecutionId;

#[derive(Clone, Debug, thiserror::Error)]
#[error("could not allocate an execution identity: {source}")]
pub struct ExecutionIdentityError {
    pub source: getrandom::Error,
}

/// Sources must allocate independent IDs for every invocation, including across
/// processes. Deterministic sources can be supplied for conformance testing.
pub trait ExecutionIdentitySource: Send + Sync {
    fn allocate(&self) -> Result<ExecutionId, ExecutionIdentityError>;
}

pub(crate) struct OsExecutionIdentitySource;
impl ExecutionIdentitySource for OsExecutionIdentitySource {
    fn allocate(&self) -> Result<ExecutionId, ExecutionIdentityError> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(|source| ExecutionIdentityError { source })?;
        Ok(ExecutionId::from_bytes(bytes))
    }
}
