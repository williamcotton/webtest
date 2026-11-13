//! Fixture ownership is independent of test occurrence identities.
use crate::ExecutionId;
use serde::{Deserialize, Serialize};
use webtest_model::{AttemptId, FixtureId, FixtureInstanceId, FixtureLifetime, TestExecutionId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FixtureOwnerKey {
    Test {
        test: TestExecutionId,
        attempt: Option<AttemptId>,
    },
    File {
        execution: ExecutionId,
    },
    Worker {
        invocation: ExecutionId,
        worker: u32,
    },
    Suite {
        invocation: ExecutionId,
    },
}
impl FixtureOwnerKey {
    pub fn lifetime(self) -> FixtureLifetime {
        match self {
            Self::Test { .. } => FixtureLifetime::Test,
            Self::File { .. } => FixtureLifetime::File,
            Self::Worker { .. } => FixtureLifetime::Worker,
            Self::Suite { .. } => FixtureLifetime::Suite,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixturePhase {
    Lifetime,
    Setup,
    Teardown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureExecutionContext {
    pub fixture: FixtureId,
    pub instance: FixtureInstanceId,
    pub owner: FixtureOwnerKey,
    pub phase: FixturePhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureBudgetKind {
    Instances,
    Waiters,
    ArgumentBytes,
    ResultBytes,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FixtureBudgetExceeded {
    pub budget: FixtureBudgetKind,
    pub limit: usize,
}
impl std::fmt::Display for FixtureBudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "fixture {:?} budget exceeded (limit {})",
            self.budget, self.limit
        )
    }
}
impl std::error::Error for FixtureBudgetExceeded {}
