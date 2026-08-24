//! Stable protocol-neutral machine feedback shared by static and runtime adapters.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const DIAGNOSTIC_SCHEMA_VERSION: u32 = 1;
pub const REPAIR_HINT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairHintKind {
    LocatorCandidate,
    NameCandidate,
    MemberCandidate,
    ArgumentCandidate,
    OptionCandidate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RepairReplacement {
    Locator { source: String },
    Text(String),
}

impl RepairReplacement {
    pub fn locator(source: impl Into<String>) -> Self {
        Self::Locator {
            source: source.into(),
        }
    }

    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairHint {
    pub kind: RepairHintKind,
    pub replacement: RepairReplacement,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_range: Option<ByteRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub evidence: BTreeMap<String, String>,
}

impl RepairHint {
    pub fn locator(source: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            kind: RepairHintKind::LocatorCandidate,
            replacement: RepairReplacement::locator(source),
            source_range: None,
            reason: Some(reason.into()),
            evidence: BTreeMap::new(),
        }
    }

    pub fn text(kind: RepairHintKind, replacement: impl Into<String>) -> Self {
        Self {
            kind,
            replacement: RepairReplacement::text(replacement),
            source_range: None,
            reason: None,
            evidence: BTreeMap::new(),
        }
    }
}

pub type SemanticDetails = serde_json::Value;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locator_hint_has_the_versioned_machine_shape() {
        let value = serde_json::to_value(RepairHint::locator(
            "role(\"button\", name: \"Save\")",
            "same role",
        ))
        .expect("serialize hint");
        assert_eq!(value["kind"], "locator_candidate");
        assert_eq!(
            value["replacement"]["source"],
            "role(\"button\", name: \"Save\")"
        );
    }
}
