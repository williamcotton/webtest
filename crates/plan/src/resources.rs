//! Static resource access facts and deterministic conflict checking. References
//! identify semantic slots, never backend handles or runtime generations.
use crate::{PlanNode, PlanNodeKind, ResourcePlan};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use webtest_model::{BindingId, Capability};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceReference {
    BrowserContext,
    Binding(BindingId),
}

pub use webtest_model::ResourceAccess;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceUse {
    pub resource: ResourceReference,
    pub access: ResourceAccess,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceConflict {
    pub first_branch: usize,
    pub second_branch: usize,
    pub resource: ResourceReference,
}

/// Resource-independent conflict rules: only declared shared/shared access may
/// overlap. Sibling order, not completion order, determines diagnostic ordering.
pub fn conflicting_resource_accesses(branches: &[Vec<ResourceUse>]) -> Vec<ResourceConflict> {
    let mut conflicts = Vec::new();
    for (first_branch, first) in branches.iter().enumerate() {
        for (second_branch, second) in branches.iter().enumerate().skip(first_branch + 1) {
            let first = merged(first.iter().copied());
            let second = merged(second.iter().copied());
            for (resource, access) in first {
                if let Some(other) = second.get(&resource)
                    && (access == ResourceAccess::Exclusive || *other == ResourceAccess::Exclusive)
                {
                    conflicts.push(ResourceConflict {
                        first_branch,
                        second_branch,
                        resource,
                    });
                }
            }
        }
    }
    conflicts
}

fn merged(
    uses: impl IntoIterator<Item = ResourceUse>,
) -> BTreeMap<ResourceReference, ResourceAccess> {
    let mut merged = BTreeMap::new();
    for usage in uses {
        merged
            .entry(usage.resource)
            .and_modify(|access: &mut ResourceAccess| *access = (*access).max(usage.access))
            .or_insert(usage.access);
    }
    merged
}

impl PlanNode {
    /// Requirements escaping this subtree. A resource scope satisfies its own
    /// slot; its children still retain their individual access requirements.
    pub fn required_resources(&self) -> Vec<ResourceUse> {
        let resources = match &self.kind {
            PlanNodeKind::Operation { .. } => {
                if self.required_capabilities.contains(&Capability::Browser) {
                    return vec![ResourceUse {
                        resource: ResourceReference::BrowserContext,
                        access: ResourceAccess::Exclusive,
                    }];
                }
                return Vec::new();
            }
            PlanNodeKind::Sequence { children }
            | PlanNodeKind::Parallel { children, .. }
            | PlanNodeKind::Race { children, .. } => {
                merged(children.iter().flat_map(Self::required_resources))
            }
            PlanNodeKind::Timeout { child, .. } => return child.required_resources(),
            PlanNodeKind::ResourceScope {
                resource: ResourcePlan::BrowserContext,
                body,
            } => {
                let mut resources = merged(body.required_resources());
                resources.remove(&ResourceReference::BrowserContext);
                resources
            }
        };
        resources
            .into_iter()
            .map(|(resource, access)| ResourceUse { resource, access })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conflicts_are_generic_ordered_and_deduplicated() {
        let shared = ResourceUse {
            resource: ResourceReference::Binding(BindingId(4)),
            access: ResourceAccess::Shared,
        };
        let exclusive = ResourceUse {
            access: ResourceAccess::Exclusive,
            ..shared
        };
        assert!(conflicting_resource_accesses(&[vec![shared], vec![shared]]).is_empty());
        assert_eq!(
            conflicting_resource_accesses(&[
                vec![exclusive, shared],
                vec![shared],
                vec![exclusive]
            ]),
            vec![
                ResourceConflict {
                    first_branch: 0,
                    second_branch: 1,
                    resource: shared.resource
                },
                ResourceConflict {
                    first_branch: 0,
                    second_branch: 2,
                    resource: shared.resource
                },
                ResourceConflict {
                    first_branch: 1,
                    second_branch: 2,
                    resource: shared.resource
                },
            ]
        );
        assert!(
            conflicting_resource_accesses(&[
                vec![exclusive],
                vec![ResourceUse {
                    resource: ResourceReference::Binding(BindingId(5)),
                    ..exclusive
                }]
            ])
            .is_empty()
        );
    }
}
