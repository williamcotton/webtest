use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Component, Path, PathBuf},
};

use webtest_hir::BindingId;
use webtest_plan::{PlannedStep, ServerProviderCall};
use webtest_provider::Value;

use crate::{
    evaluation::runtime_transferable,
    redaction::{
        bounded_value_summary, collect_provider_result_secrets, collect_provider_secrets,
        provider_argument_summaries, visible_step_bindings,
    },
};

pub(super) struct TestExecutionState {
    environment: HashMap<BindingId, Value>,
    binding_names: HashMap<BindingId, String>,
    secrets: Vec<String>,
    redacted_fields: Vec<String>,
    project_root: PathBuf,
    owned_temporary_directories: BTreeSet<PathBuf>,
}

impl TestExecutionState {
    pub(super) fn new(redacted_fields: Vec<String>, project_root: PathBuf) -> Self {
        Self {
            environment: HashMap::new(),
            binding_names: HashMap::new(),
            secrets: Vec::new(),
            redacted_fields,
            project_root,
            owned_temporary_directories: BTreeSet::new(),
        }
    }

    pub(super) fn environment(&self) -> &HashMap<BindingId, Value> {
        &self.environment
    }

    pub(super) fn bind(&mut self, id: BindingId, name: Option<&str>, value: Value) {
        self.environment.insert(id, value);
        self.binding_names.insert(
            id,
            name.map(str::to_owned)
                .unwrap_or_else(|| format!("binding_{}", id.0)),
        );
    }

    pub(super) fn prepare_provider_arguments(&mut self, call: &ServerProviderCall) {
        collect_provider_secrets(
            call,
            &self.environment,
            &self.redacted_fields,
            &mut self.secrets,
        );
    }

    pub(super) fn accept_provider_result_metadata(&mut self, call: &ServerProviderCall) {
        self.redacted_fields
            .extend(call.redacted_result_fields.iter().cloned());
        self.redacted_fields.sort();
        self.redacted_fields.dedup();
        collect_provider_result_secrets(
            call,
            &self.environment,
            &self.redacted_fields,
            &mut self.secrets,
        );
        if let Some(value) = call
            .result_binding
            .and_then(|binding| self.environment.get(&binding))
        {
            for path in temporary_directories(value) {
                self.owned_temporary_directories
                    .insert(normalize_owned_path(&self.project_root, &path));
            }
        }
    }

    pub(super) fn visible_step_bindings(&self, step: &PlannedStep) -> BTreeMap<String, Value> {
        visible_step_bindings(
            step,
            &self.environment,
            &self.binding_names,
            &self.redacted_fields,
            &self.secrets,
        )
    }

    pub(super) fn provider_argument_summaries(
        &self,
        call: &ServerProviderCall,
    ) -> BTreeMap<String, String> {
        provider_argument_summaries(
            call,
            &self.environment,
            &self.redacted_fields,
            &self.secrets,
        )
    }

    pub(super) fn provider_result_summary(&self, call: &ServerProviderCall) -> Option<String> {
        call.result_binding
            .and_then(|binding| self.environment.get(&binding))
            .map(|value| {
                bounded_value_summary(
                    &value.redacted_with_secrets(&self.redacted_fields, &self.secrets),
                )
            })
    }

    pub(super) fn redaction(&self) -> (&[String], &[String]) {
        (&self.redacted_fields, &self.secrets)
    }

    pub(super) fn final_transferable_bindings(
        &self,
        default_redacted_fields: &[String],
    ) -> BTreeMap<String, Value> {
        self.binding_names
            .iter()
            .filter_map(|(id, name)| {
                self.environment
                    .get(id)
                    .filter(|value| runtime_transferable(value))
                    .map(|value| {
                        value.redacted_with_secrets(default_redacted_fields, &self.secrets)
                    })
                    .map(|value| (name.clone(), value))
            })
            .collect()
    }

    pub(super) fn temporary_directories(&self) -> Vec<PathBuf> {
        self.owned_temporary_directories.iter().cloned().collect()
    }
}

fn normalize_owned_path(project_root: &Path, path: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };
    let rooted = path.has_root();
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::ParentDir) | None if !rooted => normalized.push(".."),
                Some(Component::Prefix(_))
                | Some(Component::RootDir)
                | Some(Component::CurDir)
                | Some(Component::ParentDir)
                | None => {}
            },
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn temporary_directories(value: &Value) -> Vec<PathBuf> {
    match value {
        Value::TempDirectory(path) => vec![path.clone()],
        Value::List(values) => values.iter().flat_map(temporary_directories).collect(),
        Value::Record(values) => values.values().flat_map(temporary_directories).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_owned_path;
    use std::path::Path;

    #[test]
    fn owned_path_normalization_deduplicates_without_changing_parent_semantics() {
        assert_eq!(
            normalize_owned_path(Path::new("project"), Path::new("tmp/./owned")),
            Path::new("project/tmp/owned")
        );
        assert_eq!(
            normalize_owned_path(Path::new("project"), Path::new("../outside")),
            Path::new("outside")
        );
        assert_eq!(
            normalize_owned_path(Path::new("."), Path::new("../outside")),
            Path::new("../outside")
        );
    }
}
