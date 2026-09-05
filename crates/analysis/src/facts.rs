//! Public data-only results returned by semantic and editor queries.

use webtest_model::{Capability, Type};
use webtest_text::TextRange;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeFact {
    pub range: TextRange,
    pub ty: Type,
    pub capability: Capability,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionKind {
    Function,
    Parameter,
    Property,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Completion {
    pub label: String,
    pub detail: String,
    pub documentation: String,
    pub kind: CompletionKind,
    pub insert_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SignatureParameter {
    pub label: String,
    pub documentation: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Signature {
    pub label: String,
    pub documentation: String,
    pub parameters: Vec<SignatureParameter>,
    pub active_parameter: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentationFact {
    pub range: TextRange,
    pub contents: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_kind_serialization_is_stable() {
        assert_eq!(
            serde_json::to_value(CompletionKind::Function).expect("serialize completion kind"),
            "function"
        );
        assert_eq!(
            serde_json::to_value(CompletionKind::Parameter).expect("serialize completion kind"),
            "parameter"
        );
        assert_eq!(
            serde_json::to_value(CompletionKind::Property).expect("serialize completion kind"),
            "property"
        );
    }
}
