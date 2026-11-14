//! Explicit language semantics carried by structured execution nodes.
use crate::PlanExpr;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckContinuation {
    AssertionsOnly,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SemanticOperation {
    Guards {
        clauses: Vec<GuardClause>,
        failures: [usize; 3],
    },
    Select {
        actor: webtest_model::ActorId,
        cursor: Option<webtest_model::BindingId>,
        duration: std::time::Duration,
        failures: [usize; 4],
        alternatives: Vec<SelectAlternative>,
        timeout: Option<usize>,
        result: Option<crate::RaceBinding>,
    },
    ActorScope {
        actor: webtest_model::ActorId,
        name: String,
        sources: Vec<webtest_model::EventSource>,
    },
    ActorAccess {
        actor: webtest_model::ActorId,
        name: String,
    },
    Check {
        continuation: CheckContinuation,
    },
    Assume {
        condition: PlanExpr,
    },
}
impl SemanticOperation {
    pub fn identity(&self) -> &'static str {
        match self {
            Self::Guards { .. } => "guards/v1",
            Self::Select { .. } => "select/v1",
            Self::ActorScope { .. } => "actor/scope/v1",
            Self::ActorAccess { .. } => "actor/access/v1",
            Self::Check { .. } => "check/v1",
            Self::Assume { .. } => "assume/v1",
        }
    }
    pub fn expressions(&self) -> Vec<&PlanExpr> {
        match self {
            Self::Guards { clauses, .. } => clauses
                .iter()
                .filter_map(|c| c.alternative.filter.as_ref())
                .collect(),
            Self::Assume { condition } => vec![condition],
            Self::Select { alternatives, .. } => alternatives
                .iter()
                .filter_map(|a| a.filter.as_ref())
                .collect(),
            _ => Vec::new(),
        }
    }
    pub fn expressions_mut(&mut self) -> Vec<&mut PlanExpr> {
        match self {
            Self::Guards { clauses, .. } => clauses
                .iter_mut()
                .filter_map(|c| c.alternative.filter.as_mut())
                .collect(),
            Self::Assume { condition } => vec![condition],
            Self::Select { alternatives, .. } => alternatives
                .iter_mut()
                .filter_map(|a| a.filter.as_mut())
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectAlternative {
    pub id: webtest_model::AlternativeId,
    pub source: webtest_model::EventSource,
    pub binding: webtest_model::BindingId,
    pub name: String,
    pub filter: Option<PlanExpr>,
}
impl crate::PlanNode {
    pub fn event_sources(&self, actor: webtest_model::ActorId) -> Vec<webtest_model::EventSource> {
        let mut sources = std::collections::BTreeSet::new();
        let mut pending = vec![self];
        while let Some(node) = pending.pop() {
            if let crate::PlanNodeKind::Semantic {
                operation:
                    SemanticOperation::Select {
                        actor: owner,
                        alternatives,
                        ..
                    },
                ..
            } = &node.kind
                && *owner == actor
            {
                sources.extend(alternatives.iter().map(|a| a.source));
            }
            if let crate::PlanNodeKind::Semantic {
                operation: SemanticOperation::Guards { clauses, .. },
                ..
            } = &node.kind
            {
                sources.extend(
                    clauses
                        .iter()
                        .filter(|c| c.actor == actor)
                        .map(|c| c.alternative.source),
                );
            }
            pending.extend(node.children());
        }
        sources.into_iter().collect()
    }
}

impl crate::PlanNode {
    pub fn children(&self) -> Vec<&Self> {
        use crate::PlanNodeKind::*;
        match &self.kind {
            Sequence { children }
            | Semantic { children, .. }
            | Parallel { children, .. }
            | Race { children, .. }
            | Conditional {
                branches: children, ..
            } => children.iter().collect(),
            Retry { child, .. }
            | Timeout { child, .. }
            | Helper { body: child, .. }
            | ResourceScope { body: child, .. } => vec![child],
            Operation { .. } => vec![],
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventFailureKind {
    SelectionTimeout,
    Overflow,
    CursorExpired,
    SourceClosed,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReactiveOperation {
    Checkpoint {
        actor: webtest_model::ActorId,
        binding: webtest_model::BindingId,
        name: String,
    },
    Failure {
        reason: EventFailureKind,
        actor: webtest_model::ActorId,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuardClause {
    pub guard: webtest_model::GuardId,
    pub actor: webtest_model::ActorId,
    pub alternative: SelectAlternative,
    pub child: usize,
}
impl SemanticOperation {
    pub(crate) fn validate(&self, children: &[crate::PlanNode]) -> bool {
        use crate::{PlanNodeKind, TestOperation};
        fn pure(node: &crate::PlanNode) -> bool {
            match &node.kind {
                PlanNodeKind::Sequence { children }
                | PlanNodeKind::Conditional {
                    branches: children, ..
                } => children.iter().all(pure),
                PlanNodeKind::Helper { body, call } => {
                    call.result_type.is_transferable() && pure(body)
                }
                PlanNodeKind::Operation { step } => {
                    matches!(&step.operation,TestOperation::EvaluatePure(value) if value.result_type.is_transferable())
                        || matches!(&step.operation,TestOperation::Return(value) if value.result_type.is_transferable())
                }
                _ => false,
            }
        }
        fn safe(node: &crate::PlanNode) -> bool {
            match &node.kind {
                PlanNodeKind::Sequence { children } => children.iter().all(safe),
                PlanNodeKind::Operation { step }
                    if matches!(
                        &step.operation,
                        TestOperation::Verdict {
                            verdict: webtest_model::VerdictKind::Fail
                                | webtest_model::VerdictKind::Inconclusive,
                            ..
                        }
                    ) =>
                {
                    true
                }
                _ => pure(node),
            }
        }
        fn terminal(node: &crate::PlanNode) -> bool {
            match &node.kind {
                PlanNodeKind::Sequence { children } => children.last().is_some_and(terminal),
                PlanNodeKind::Operation { step } => matches!(
                    &step.operation,
                    TestOperation::Verdict {
                        verdict: webtest_model::VerdictKind::Fail
                            | webtest_model::VerdictKind::Inconclusive,
                        ..
                    }
                ),
                _ => false,
            }
        }
        let guard_body = |node: &crate::PlanNode| safe(node) && terminal(node);
        let failure = |index: usize, reason: EventFailureKind| {
            children.get(index).is_some_and(|node|matches!(&node.kind,PlanNodeKind::Operation{step} if matches!(&step.operation,TestOperation::Reactive(ReactiveOperation::Failure{reason:actual,..}) if *actual==reason)))
        };
        match self {
            Self::ActorScope { sources, .. } => {
                children.len() == 1
                    && sources.len() <= webtest_model::EventSource::ALL.len()
                    && sources
                        .iter()
                        .copied()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        == sources.len()
            }
            Self::ActorAccess { .. } | Self::Assume { .. } => children.len() == 1,
            Self::Check { .. } => true,
            Self::Select {
                alternatives,
                duration,
                timeout,
                failures,
                result,
                ..
            } => {
                !alternatives.is_empty()
                    && alternatives.len() <= 64
                    && !duration.is_zero()
                    && *duration <= crate::MAX_CONTROL_TIMEOUT
                    && result.as_ref().is_none_or(|r| r.ty.is_transferable())
                    && timeout.is_none_or(|index| index == alternatives.len())
                    && children.len() == alternatives.len() + usize::from(timeout.is_some()) + 4
                    && failures.iter().enumerate().all(|(i, index)| {
                        *index == alternatives.len() + usize::from(timeout.is_some()) + i
                    })
                    && failure(failures[0], EventFailureKind::SelectionTimeout)
                    && failure(failures[1], EventFailureKind::Overflow)
                    && failure(failures[2], EventFailureKind::CursorExpired)
                    && failure(failures[3], EventFailureKind::SourceClosed)
            }
            Self::Guards { clauses, failures } => {
                !clauses.is_empty()
                    && clauses.len() <= 64
                    && children.len() == clauses.len() + 4
                    && clauses.iter().enumerate().all(|(i, clause)| {
                        clause.child == i + 1 && children.get(clause.child).is_some_and(guard_body)
                    })
                    && failure(failures[0], EventFailureKind::Overflow)
                    && failure(failures[1], EventFailureKind::CursorExpired)
                    && failure(failures[2], EventFailureKind::SourceClosed)
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StaticFlow {
    Continue,
    Boundary,
    Terminal,
}
impl crate::PlanNode {
    /// Whether every successful path exits the test variant with a verdict.
    pub fn terminates_variant(&self) -> bool {
        self.static_flow() == StaticFlow::Terminal
    }
    fn static_flow(&self) -> StaticFlow {
        use crate::{PlanNodeKind as N, TestOperation as O};
        use StaticFlow::*;
        match &self.kind {
            N::Operation { step } => match step.operation {
                O::Verdict { .. } => Terminal,
                O::Provide(_) | O::Return(_) => Boundary,
                _ => Continue,
            },
            N::Sequence { children } => children
                .iter()
                .map(Self::static_flow)
                .find(|f| *f != Continue)
                .unwrap_or(Continue),
            N::Helper { body, .. } => {
                if body.terminates_variant() {
                    Terminal
                } else {
                    Continue
                }
            }
            N::Retry { child, .. }
            | N::Timeout { child, .. }
            | N::ResourceScope { body: child, .. } => child.static_flow(),
            N::Parallel { children, .. } => {
                if children.iter().any(Self::terminates_variant) {
                    Terminal
                } else {
                    Continue
                }
            }
            N::Race { children, .. } => {
                if !children.is_empty() && children.iter().all(Self::terminates_variant) {
                    Terminal
                } else {
                    Continue
                }
            }
            N::Conditional { branches, .. } => {
                if !branches.is_empty() && branches.iter().all(Self::terminates_variant) {
                    Terminal
                } else {
                    Continue
                }
            }
            N::Semantic {
                operation,
                children,
            } => match operation {
                SemanticOperation::Assume { .. } => Continue,
                SemanticOperation::Select {
                    alternatives,
                    timeout,
                    ..
                } => {
                    if !alternatives.is_empty()
                        && (0..alternatives.len())
                            .chain(timeout.iter().copied())
                            .all(|i| children.get(i).is_some_and(Self::terminates_variant))
                    {
                        Terminal
                    } else {
                        Continue
                    }
                }
                SemanticOperation::Guards { .. } => {
                    children.first().map_or(Continue, Self::static_flow)
                }
                _ => children
                    .iter()
                    .map(Self::static_flow)
                    .find(|f| *f != Continue)
                    .unwrap_or(Continue),
            },
        }
    }
}
