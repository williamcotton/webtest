//! Explicit source-mapped fixture phases; runtime supplies lifetime ownership.
use crate::{PlanNode, PlanTreeError};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FixtureAcquire {
    pub fixture: FixtureId,
    pub definition_revision: SourceRevision,
    pub input_fingerprint: SourceRevision,
    pub lifetime: FixtureLifetime,
    pub arguments: Vec<crate::HelperArgument>,
    pub result_binding: Option<BindingId>,
    pub result_name: Option<String>,
    pub result_type: Type,
    pub required_capabilities: Vec<Capability>,
    pub retry_safe: bool,
}
use serde::{Deserialize, Serialize};
use webtest_model::{BindingId, Capability, FixtureId, FixtureLifetime, PlanDeclarationId, Type};
use webtest_text::{SourceRevision, SyntaxOrigin};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FixtureParameter {
    pub binding: BindingId,
    pub name: String,
    pub ty: Type,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FixtureDefinition {
    pub id: FixtureId,
    pub name: String,
    pub origin: SyntaxOrigin,
    pub source_revision: SourceRevision,
    /// Includes resolved dependency sources and provider schema identities.
    pub input_fingerprint: SourceRevision,
    pub lifetime: FixtureLifetime,
    pub shareable: bool,
    pub parameters: Vec<FixtureParameter>,
    pub result_type: Type,
    pub setup: PlanNode,
    pub teardown: PlanNode,
}
impl FixtureDefinition {
    pub fn lifetime_node(&self) -> PlanNode {
        PlanNode::sequence(
            PlanDeclarationId(self.id.0),
            self.origin,
            self.source_revision,
            Vec::new(),
            vec![self.setup.clone(), self.teardown.clone()],
        )
    }

    pub fn required_capabilities(&self) -> Vec<Capability> {
        self.setup
            .required_capabilities
            .iter()
            .chain(&self.teardown.required_capabilities)
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }
    pub fn validate(&self) -> Result<(), PlanTreeError> {
        if self.name.is_empty()
            || self.result_type == Type::Unknown
            || (self.lifetime != FixtureLifetime::Test
                && (!self.shareable
                    || !self.result_type.is_transferable()
                    || self
                        .parameters
                        .iter()
                        .any(|parameter| !parameter.ty.is_transferable())))
        {
            return Err(PlanTreeError::InvalidFixture);
        }
        let mut names = std::collections::BTreeSet::new();
        let mut bindings = std::collections::BTreeSet::new();
        for parameter in &self.parameters {
            if parameter.name.is_empty()
                || !parameter.ty.is_transferable()
                || !names.insert(&parameter.name)
                || !bindings.insert(parameter.binding)
            {
                return Err(PlanTreeError::InvalidFixture);
            }
        }
        let mut steps = std::collections::BTreeSet::new();
        let declaration = PlanDeclarationId(self.id.0);
        self.setup
            .validate(declaration, self.source_revision, &[0], &mut steps)?;
        self.teardown
            .validate(declaration, self.source_revision, &[1], &mut steps)?;
        if !self
            .setup
            .validate_results(crate::tree::ResultFlow::Helper, Some(&self.result_type))?
        {
            return Err(PlanTreeError::InvalidFixture);
        }
        self.teardown
            .validate_results(crate::tree::ResultFlow::None, None)?;
        Ok(())
    }
}

pub(crate) fn validate_references(
    fixtures: &[FixtureDefinition],
    tests: &[crate::PlannedTest],
    source: &impl Fn(webtest_text::FileId) -> Option<SourceRevision>,
) -> Result<(), PlanTreeError> {
    let mut definitions = std::collections::BTreeMap::new();
    for definition in fixtures {
        if definitions.insert(definition.id, definition).is_some() {
            return Err(PlanTreeError::InvalidFixture);
        }
        definition.validate()?;
        if source(definition.origin.file) != Some(definition.source_revision) {
            return Err(PlanTreeError::MissingSourceFile);
        }
        definition.setup.validate_sources(source)?;
        definition.teardown.validate_sources(source)?;
    }
    let mut edges = std::collections::BTreeMap::new();
    let nodes = tests
        .iter()
        .map(|test| (None, &test.body))
        .chain(fixtures.iter().flat_map(|definition| {
            [
                (Some(definition), &definition.setup),
                (Some(definition), &definition.teardown),
            ]
        }));
    for (owner, node) in nodes {
        for step in node.steps() {
            let crate::TestOperation::AcquireFixture(call) = &step.operation else {
                continue;
            };
            let Some(definition) = definitions.get(&call.fixture) else {
                return Err(PlanTreeError::InvalidFixture);
            };
            let mut bindings = std::collections::BTreeSet::new();
            if call.definition_revision != definition.source_revision
                || call.input_fingerprint != definition.input_fingerprint
                || call.lifetime != definition.lifetime
                || call.result_type != definition.result_type
                || call.required_capabilities != definition.required_capabilities()
                || call.arguments.len() != definition.parameters.len()
                || call.arguments.iter().any(|argument| {
                    !bindings.insert(argument.parameter)
                        || !definition.parameters.iter().any(|parameter| {
                            parameter.binding == argument.parameter
                                && parameter.name == argument.name
                        })
                })
                || call
                    .result_name
                    .as_ref()
                    .is_some_and(|name| name.is_empty() || call.result_binding.is_none())
            {
                return Err(PlanTreeError::InvalidFixture);
            }
            let repeatable = definition.lifetime != FixtureLifetime::Test
                || (definition.setup.retry_safety_violations().is_empty()
                    && definition.teardown.retry_safety_violations().is_empty());
            if call.retry_safe != repeatable {
                return Err(PlanTreeError::InvalidFixture);
            }
            if let Some(owner) = owner {
                if definition.lifetime.rank() < owner.lifetime.rank() {
                    return Err(PlanTreeError::InvalidFixture);
                }
                edges
                    .entry(owner.id)
                    .or_insert_with(Vec::new)
                    .push(definition.id);
            }
        }
    }
    // Explicit iterative traversal keeps forged archives from recursing the host stack.
    let mut completed = std::collections::BTreeSet::new();
    for start in definitions.keys() {
        let mut path = std::collections::BTreeSet::new();
        let mut pending = vec![(*start, false)];
        while let Some((id, exiting)) = pending.pop() {
            if exiting {
                path.remove(&id);
                completed.insert(id);
                continue;
            }
            if completed.contains(&id) {
                continue;
            }
            if path.len() >= 128 || !path.insert(id) {
                return Err(PlanTreeError::InvalidFixture);
            }
            pending.push((id, true));
            pending.extend(
                edges
                    .get(&id)
                    .into_iter()
                    .flatten()
                    .rev()
                    .map(|dependency| (*dependency, false)),
            );
        }
    }
    Ok(())
}
