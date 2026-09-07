//! Portable retry settings and operation repeatability contracts.
use crate::{BrowserOperation, PlanNode, PlanNodeKind, ResourcePlan, TestOperation};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use webtest_text::SyntaxOrigin;

/// Bounds retained attempt outcomes and their individually bounded evidence.
pub const MAX_RETRY_ATTEMPTS: u32 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryPolicy {
    SafeFailures,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrySettings {
    /// Total attempts, including the initial execution.
    pub attempts: u32,
    pub backoff: RetryBackoff,
    pub policy: RetryPolicy,
}

impl RetrySettings {
    pub fn is_valid(self) -> bool {
        (1..=MAX_RETRY_ATTEMPTS).contains(&self.attempts) && self.backoff.is_valid()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryBackoff {
    pub initial: Duration,
    pub max: Duration,
}

impl RetryBackoff {
    pub fn is_valid(self) -> bool {
        self.initial <= self.max && self.max <= crate::MAX_CONTROL_TIMEOUT
    }

    /// Zero-based retry delay: initial, twice initial, ... capped at max.
    pub fn delay(self, retry: u32) -> Duration {
        let mut delay = self.initial.min(self.max);
        if delay.is_zero() {
            return delay;
        }
        for _ in 0..retry {
            if delay == self.max {
                break;
            }
            delay = delay.saturating_mul(2).min(self.max);
        }
        delay
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetrySafetyViolation {
    pub origin: SyntaxOrigin,
}

impl ResourcePlan {
    pub fn is_retry_safe(self) -> bool {
        match self {
            Self::BrowserContext => true,
        }
    }
}

impl TestOperation {
    /// Browser mutations have no repeatability contract yet. Observation-only
    /// operations may reuse an enclosing context; provider contracts come from
    /// the fingerprinted schema, never from the runtime failure's retryable bit.
    pub fn is_retry_safe(&self) -> bool {
        match self {
            Self::EvaluatePure(_) | Self::Provide(_) | Self::Assertion(_) => true,
            Self::ServerProviderCall(call) => call.retry_safe,
            Self::Browser(operation) => matches!(
                operation,
                BrowserOperation::WaitForLocator { .. } | BrowserOperation::WaitForUrl { .. }
            ),
        }
    }
}

impl PlanNode {
    pub fn retry_safety_violations(&self) -> Vec<RetrySafetyViolation> {
        match &self.kind {
            PlanNodeKind::Operation { step } => {
                if step.operation.is_retry_safe() {
                    vec![]
                } else {
                    vec![RetrySafetyViolation {
                        origin: step.origin,
                    }]
                }
            }
            PlanNodeKind::Sequence { children }
            | PlanNodeKind::Parallel { children, .. }
            | PlanNodeKind::Race { children, .. } => children
                .iter()
                .flat_map(Self::retry_safety_violations)
                .collect(),
            PlanNodeKind::Retry { child, .. } | PlanNodeKind::Timeout { child, .. } => {
                child.retry_safety_violations()
            }
            PlanNodeKind::ResourceScope { resource, body } => {
                let mut violations = Vec::new();
                if !resource.is_retry_safe() {
                    violations.push(RetrySafetyViolation {
                        origin: self.origin,
                    });
                }
                violations.extend(body.retry_safety_violations());
                violations
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempt_bounds_and_backoff_are_finite_and_saturate_without_overflow() {
        let mut settings = RetrySettings {
            attempts: 1,
            backoff: RetryBackoff {
                initial: Duration::from_millis(10),
                max: Duration::from_millis(25),
            },
            policy: RetryPolicy::SafeFailures,
        };
        assert!(settings.is_valid());
        assert_eq!(
            (0..5)
                .map(|n| settings.backoff.delay(n).as_millis())
                .collect::<Vec<_>>(),
            [10, 20, 25, 25, 25]
        );
        assert_eq!(settings.backoff.delay(u32::MAX), settings.backoff.max);
        for attempts in [0, MAX_RETRY_ATTEMPTS + 1, u32::MAX] {
            settings.attempts = attempts;
            assert!(!settings.is_valid());
        }
        settings.attempts = MAX_RETRY_ATTEMPTS;
        assert!(settings.is_valid());
        settings.backoff.initial = Duration::from_secs(1);
        assert!(!settings.is_valid());
        assert_eq!(RetryBackoff::default().delay(u32::MAX), Duration::ZERO);
        assert!(
            !RetryBackoff {
                initial: Duration::ZERO,
                max: Duration::MAX
            }
            .is_valid()
        );
    }
}
