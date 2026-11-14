//! Versioned browser boundary. Coordinates are UTF-16; plans retain canonical byte origins.
use crate::{PortableWorkspace, WorkspaceInputs, WorkspaceQuery};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use webtest_text::DocumentVersion;

pub const EDITOR_API_VERSION: u32 = 1;
pub const WORKER_PROTOCOL_VERSION: u32 = 1;
const MAX_MESSAGE: usize = 80 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WebRange {
    pub start: Position,
    pub end: Position,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    protocol: u32,
    id: u64,
    method: String,
    #[serde(default, rename = "workspaceRevision")]
    workspace_revision: Option<u64>,
    params: Value,
}
#[derive(Debug)]
struct Error {
    code: &'static str,
    message: String,
}
type Result<T> = std::result::Result<T, Error>;
fn error(code: &'static str, message: impl Into<String>) -> Error {
    Error {
        code,
        message: message.into(),
    }
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| error("invalid_params", e.to_string()))
}

pub fn byte_offset(text: &str, position: Position) -> std::result::Result<u32, String> {
    let mut start = 0;
    for _ in 0..position.line {
        start += text[start..].find('\n').ok_or("line outside document")? + 1;
    }
    let line = text[start..]
        .split('\n')
        .next()
        .unwrap_or("")
        .trim_end_matches('\r');
    let mut units = 0;
    for (offset, ch) in line.char_indices() {
        if units == position.character {
            return Ok((start + offset) as u32);
        }
        units += ch.len_utf16() as u32;
        if units > position.character {
            return Err("position splits a surrogate pair".into());
        }
    }
    if units == position.character {
        Ok((start + line.len()) as u32)
    } else {
        Err("character outside line".into())
    }
}
pub fn position(text: &str, offset: u32) -> std::result::Result<Position, String> {
    let offset = offset as usize;
    if !text.is_char_boundary(offset) {
        return Err("invalid UTF-8 boundary".into());
    }
    let before = &text[..offset];
    let line = before.bytes().filter(|b| *b == b'\n').count() as u32;
    let character = before
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .encode_utf16()
        .count() as u32;
    Ok(Position { line, character })
}

#[derive(Default)]
pub struct EditorSession {
    workspace: Option<PortableWorkspace>,
    inputs: Option<WorkspaceInputs>,
    revision: u64,
}
impl EditorSession {
    /// Always returns a structured response, including malformed envelopes.
    pub fn request(&mut self, message: &str) -> String {
        let parsed = if message.len() > MAX_MESSAGE {
            Err(error("limit_exceeded", "message byte budget exceeded"))
        } else {
            serde_json::from_str::<Request>(message)
                .map_err(|e| error("invalid_request", e.to_string()))
        };
        let id = parsed.as_ref().map_or(Value::Null, |r| json!(r.id));
        let result = parsed.and_then(|request| {
            if request.protocol != WORKER_PROTOCOL_VERSION {
                return Err(error("unsupported_protocol", "expected worker protocol 1"));
            }
            if request
                .workspace_revision
                .is_some_and(|r| r != self.revision)
            {
                return Err(error("stale_revision", "workspace revision changed"));
            }
            self.dispatch(&request.method, request.params)
        });
        let response = match result {
            Ok(result) => {
                json!({"protocol":1,"id":id,"ok":true,"revision":self.revision,"result":result})
            }
            Err(e) => {
                json!({"protocol":1,"id":id,"ok":false,"revision":self.revision,"error":{"code":e.code,"message":e.message}})
            }
        };
        // Value serialization is infallible for the DTOs above.
        let encoded = response.to_string();
        if encoded.len() > MAX_MESSAGE {
            return json!({"protocol":1,"id":id,"ok":false,"revision":self.revision,"error":{"code":"limit_exceeded","message":"response byte budget exceeded"}}).to_string();
        }
        encoded
    }
    fn commit(&mut self, inputs: WorkspaceInputs) -> Result<Value> {
        if inputs.sources.iter().any(|source| {
            source
                .version
                .is_some_and(|v| !(0..=9_007_199_254_740_991).contains(&v.0))
        }) {
            return Err(error(
                "invalid_params",
                "document version must be a nonnegative safe integer",
            ));
        }
        if let Some(workspace) = &mut self.workspace {
            let previous = workspace.revision();
            let result = workspace.replace_inputs(inputs.clone());
            // Invalid schemas deliberately evict previous semantic authority.
            if result.is_err() && workspace.revision() != previous {
                self.revision += 1;
                if let Some(current) = &mut self.inputs {
                    current.app_manifest = inputs.app_manifest.clone();
                }
            }
            result.map_err(|e| error("invalid_inputs", e))?;
        } else {
            self.workspace = Some(
                PortableWorkspace::new(inputs.clone()).map_err(|e| error("invalid_inputs", e))?,
            );
        }
        self.inputs = Some(inputs);
        self.revision += 1;
        Ok(json!({"api_version":EDITOR_API_VERSION}))
    }
    fn dispatch(&mut self, method: &str, params: Value) -> Result<Value> {
        match method {
            "compatibility" => Ok(json!({
                "wasm_editor_api":EDITOR_API_VERSION,"worker_protocol":WORKER_PROTOCOL_VERSION,
                "plan_format":webtest_plan::PLAN_FORMAT_VERSION,
                "runtime_semantics":webtest_plan::RUNTIME_SEMANTICS_VERSION,
                "description_schema":webtest_analysis::DESCRIPTION_SCHEMA_VERSION,
                "diagnostic_schema":webtest_feedback::DIAGNOSTIC_SCHEMA_VERSION,
                "repair_hint_schema":webtest_feedback::REPAIR_HINT_SCHEMA_VERSION,
                "app_bridge_protocol":[webtest_app_bridge::PROTOCOL_VERSION]
            })),
            "openWorkspace" => self.commit(decode(params)?),
            "dispose" => {
                self.workspace = None;
                self.inputs = None;
                self.revision += 1;
                Ok(Value::Null)
            }
            "execute" => Err(error(
                "capability_unavailable",
                "portable editor does not execute plans",
            )),
            "describe" => {
                let query = params.get("query").and_then(Value::as_str);
                let search = params.get("search").and_then(Value::as_str);
                serde_json::to_value(
                    if let Some(manifest) =
                        self.inputs.as_ref().and_then(|i| i.app_manifest.as_deref())
                    {
                        crate::describe_with_manifest(query, search, manifest)
                            .map_err(|e| error("invalid_inputs", e))?
                    } else {
                        crate::describe(query, search)
                    },
                )
                .map_err(|e| error("internal_error", e.to_string()))
            }
            "updateConfiguration"
            | "updateProviderSchema"
            | "openDocument"
            | "updateDocument"
            | "closeDocument"
            | "removeFile" => {
                let mut inputs = self
                    .inputs
                    .clone()
                    .ok_or_else(|| error("workspace_closed", "open a workspace first"))?;
                match method {
                    "updateConfiguration" => inputs.configuration = decode(params)?,
                    "updateProviderSchema" => {
                        inputs.app_manifest = if params.is_null() {
                            None
                        } else {
                            Some(decode(params)?)
                        }
                    }
                    "openDocument" => {
                        let source: crate::WorkspaceSource = decode(params)?;
                        if let Some(old) = inputs.sources.iter_mut().find(|s| s.path == source.path)
                        {
                            if old.version.is_some()
                                && source.version.map(|v| v.0) <= old.version.map(|v| v.0)
                            {
                                return Err(error(
                                    "stale_version",
                                    "document version must increase",
                                ));
                            }
                            *old = source;
                        } else {
                            inputs.sources.push(source);
                        }
                    }
                    _ => {
                        #[derive(Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Change {
                            path: String,
                            #[serde(default)]
                            version: Option<DocumentVersion>,
                            #[serde(default)]
                            edits: Vec<Edit>,
                        }
                        #[derive(Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Edit {
                            range: WebRange,
                            text: String,
                        }
                        let change: Change = decode(params)?;
                        let index = inputs
                            .sources
                            .iter()
                            .position(|s| s.path == change.path)
                            .ok_or_else(|| error("unknown_document", "document is not supplied"))?;
                        if method == "removeFile" {
                            inputs.sources.remove(index);
                            inputs.aliases.retain(|_, target| target != &change.path);
                        } else if method == "closeDocument" {
                            inputs.sources[index].version = None;
                        } else {
                            let source = &mut inputs.sources[index];
                            if change.version.is_none()
                                || change.version.map(|v| v.0) <= source.version.map(|v| v.0)
                            {
                                return Err(error(
                                    "stale_version",
                                    "document version must increase",
                                ));
                            }
                            // Sequential edits are staged; a rejected edit cannot mutate the workspace.
                            for edit in change.edits {
                                let start = byte_offset(&source.text, edit.range.start)
                                    .map_err(|e| error("invalid_position", e))?
                                    as usize;
                                let end = byte_offset(&source.text, edit.range.end)
                                    .map_err(|e| error("invalid_position", e))?
                                    as usize;
                                if start > end {
                                    return Err(error("invalid_position", "reversed range"));
                                }
                                source.text.replace_range(start..end, &edit.text);
                                if source.text.len() > 8 * 1024 * 1024 {
                                    return Err(error(
                                        "limit_exceeded",
                                        "document byte budget exceeded",
                                    ));
                                }
                            }
                            source.version = change.version;
                        }
                    }
                }
                self.commit(inputs)
            }
            "query" => self.web_query(params),
            _ => Err(error(
                "unknown_method",
                format!("unknown editor method `{method}`"),
            )),
        }
    }
    fn web_query(&self, mut params: Value) -> Result<Value> {
        let workspace = self
            .workspace
            .as_ref()
            .ok_or_else(|| error("workspace_closed", "open a workspace first"))?;
        let inputs = self
            .inputs
            .as_ref()
            .ok_or_else(|| error("workspace_closed", "open a workspace first"))?;
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let source = inputs.sources.iter().find(|s| s.path == path);
        let object = params
            .as_object_mut()
            .ok_or_else(|| error("invalid_params", "query must be an object"))?;
        if let Some(p) = object.remove("position") {
            let text = &source
                .ok_or_else(|| error("unknown_document", "document is not supplied"))?
                .text;
            object.insert(
                "offset".into(),
                json!(byte_offset(text, decode(p)?).map_err(|e| error("invalid_position", e))?),
            );
        }
        if let Some(range) = object.remove("range") {
            let text = &source
                .ok_or_else(|| error("unknown_document", "document is not supplied"))?
                .text;
            let range: WebRange = decode(range)?;
            let start = byte_offset(text, range.start).map_err(|e| error("invalid_position", e))?;
            let end = byte_offset(text, range.end).map_err(|e| error("invalid_position", e))?;
            if start > end {
                return Err(error("invalid_position", "reversed range"));
            }
            object.insert("start".into(), json!(start));
            object.insert("end".into(), json!(end));
        }
        let is_plan = matches!(
            object.get("kind").and_then(Value::as_str),
            Some("compile" | "compile_variant")
        );
        let request: WorkspaceQuery = decode(params)?;
        let mut result = workspace
            .query(request)
            .map_err(|e| error("query_failed", e))?["result"]
            .take();
        // Byte origins in canonical compiled plans are part of the shared plan format.
        if !is_plan {
            self.web_ranges(&mut result, source.map(|s| s.text.as_str()))?;
        }
        Ok(json!({"document_version":source.and_then(|s| s.version),"value":result}))
    }
    fn web_ranges(&self, value: &mut Value, text: Option<&str>) -> Result<()> {
        match value {
            Value::Array(items) => {
                for item in items {
                    self.web_ranges(item, text)?;
                }
            }
            Value::Object(object) => {
                let snapshot = self
                    .workspace
                    .as_ref()
                    .ok_or_else(|| error("workspace_closed", "workspace closed"))?
                    .snapshot();
                let file = object
                    .get("file")
                    .and_then(Value::as_u64)
                    .map(|id| webtest_text::FileId::new(id as u32));
                let path = object.get("path").and_then(Value::as_str);
                let owned = if let Some(path) = path {
                    snapshot
                        .file_for_path(path)
                        .and_then(|f| snapshot.source(f).ok())
                } else {
                    file.and_then(|f| snapshot.source(f).ok())
                };
                let text = owned.as_deref().or(text);
                for child in object.values_mut() {
                    self.web_ranges(child, text)?;
                }
                if let Some(text) = text {
                    if let (Some(start), Some(end)) = (
                        object.get("start").and_then(Value::as_u64),
                        object.get("end").and_then(Value::as_u64),
                    ) {
                        let start =
                            position(text, start as u32).map_err(|e| error("internal_error", e))?;
                        let end =
                            position(text, end as u32).map_err(|e| error("internal_error", e))?;
                        object.insert("range".into(), json!(WebRange { start, end }));
                    }
                    if let Some(offset) = object.get("offset").and_then(Value::as_u64) {
                        object.insert(
                            "position".into(),
                            json!(
                                position(text, offset as u32)
                                    .map_err(|e| error("internal_error", e))?
                            ),
                        );
                    }
                }
            }
            _ => (),
        }
        Ok(())
    }
}
#[cfg(target_arch = "wasm32")]
mod bindings {
    use super::*;
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen(js_name=EditorSession)]
    pub struct Session(EditorSession);
    #[wasm_bindgen(js_class=EditorSession)]
    impl Session {
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            Self(EditorSession::default())
        }
        pub fn request(&mut self, message: &str) -> String {
            self.0.request(message)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_boundaries_and_crlf_are_exact() {
        let text = "a😀e\u{301}\r\nnext\n";
        for (p, byte) in [
            (
                Position {
                    line: 0,
                    character: 3,
                },
                5,
            ),
            (
                Position {
                    line: 0,
                    character: 5,
                },
                8,
            ),
            (
                Position {
                    line: 1,
                    character: 0,
                },
                10,
            ),
            (
                Position {
                    line: 2,
                    character: 0,
                },
                15,
            ),
        ] {
            assert_eq!(byte_offset(text, p).unwrap(), byte);
            assert_eq!(position(text, byte).unwrap(), p);
        }
        assert!(
            byte_offset(
                text,
                Position {
                    line: 0,
                    character: 2
                }
            )
            .is_err()
        );
        assert!(
            byte_offset(
                text,
                Position {
                    line: 0,
                    character: 6
                }
            )
            .is_err()
        );
        assert!(
            byte_offset(
                text,
                Position {
                    line: 3,
                    character: 0
                }
            )
            .is_err()
        );
        assert!(position(text, 2).is_err());
    }
    fn call(s: &mut EditorSession, method: &str, params: Value) -> Value {
        serde_json::from_str(
            &s.request(&json!({"protocol":1,"id":1,"method":method,"params":params}).to_string()),
        )
        .unwrap()
    }
    #[test]
    fn variant_compilation_stale_revisions_and_independent_workspaces() {
        let mut first = EditorSession::default();
        let mut second = EditorSession::default();
        let inputs = json!({"configuration":{"root":"/workspace"},"sources":[{"path":"/workspace/a.webtest","text":"test \"first\" {} test \"second\" {}","version":1}]});
        assert_eq!(call(&mut first, "openWorkspace", inputs)["ok"], true);
        let discovery = call(&mut first, "query", json!({"kind":"discovery","limit":100}));
        let variant =
            discovery["result"]["value"]["modules"][0]["tests"][0]["variants"][0]["id"].clone();
        let compiled = call(
            &mut first,
            "query",
            json!({"kind":"compile_variant","variant":variant}),
        );
        assert_eq!(
            compiled["result"]["value"]["plan"]["tests"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            call(&mut second, "query", json!({"kind":"discovery","limit":10}))["error"]["code"],
            "workspace_closed"
        );
        let stale: Value = serde_json::from_str(&first.request(r#"{"protocol":1,"id":2,"workspaceRevision":0,"method":"query","params":{"kind":"discovery","limit":10}}"#)).unwrap();
        assert_eq!(stale["error"]["code"], "stale_revision");
    }
    #[test]
    fn invalid_schema_evicts_old_authority_and_input_limits_are_atomic() {
        let mut session = EditorSession::default();
        let inputs = json!({"configuration":{"root":"/workspace"},"sources":[{"path":"/workspace/a.webtest","text":"test \"app\" { server { let user = app.create_user(email: \"a@example.test\") } }","version":1}],"app_manifest":include_str!("../../../protocol/examples/app-schema.json")});
        assert_eq!(call(&mut session, "openWorkspace", inputs)["ok"], true);
        assert_eq!(
            call(
                &mut session,
                "query",
                json!({"kind":"diagnostics","path":"/workspace/a.webtest"})
            )["result"]["value"],
            json!([])
        );
        let before = session.revision;
        assert_eq!(
            call(
                &mut session,
                "openDocument",
                json!({"path":"/workspace/huge.webtest","text":"x".repeat(8*1024*1024+1),"version":1})
            )["error"]["code"],
            "invalid_inputs"
        );
        assert_eq!(session.revision, before);
        assert_eq!(
            call(&mut session, "updateProviderSchema", json!("{"))["error"]["code"],
            "invalid_inputs"
        );
        assert!(session.revision > before);
        assert_eq!(
            call(&mut session, "describe", json!({"query":"app.create_user"}))["error"]["code"],
            "invalid_inputs"
        );
        assert!(
            !call(
                &mut session,
                "query",
                json!({"kind":"diagnostics","path":"/workspace/a.webtest"})
            )["result"]["value"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            call(
                &mut session,
                "updateProviderSchema",
                json!(include_str!("../../../protocol/examples/app-schema.json"))
            )["ok"],
            true
        );
        assert_eq!(
            call(
                &mut session,
                "query",
                json!({"kind":"diagnostics","path":"/workspace/a.webtest"})
            )["result"]["value"],
            json!([])
        );
    }
    #[test]
    fn revisions_errors_edits_and_capability_boundary() {
        let mut s = EditorSession::default();
        let inputs = json!({"configuration":webtest_analysis::WorkspaceConfiguration::default(),"sources":[{"path":"/a.webtest","text":"test \"😀\" {}","version":1}]});
        assert_eq!(call(&mut s, "openWorkspace", inputs)["ok"], true);
        let before = call(
            &mut s,
            "query",
            json!({"kind":"compile","path":"/a.webtest"}),
        );
        assert_eq!(before["result"]["value"]["executable_on_wasm"], false);
        assert_eq!(
            call(&mut s, "execute", json!({}))["error"]["code"],
            "capability_unavailable"
        );
        let change = json!({"path":"/a.webtest","version":2,"edits":[{"range":{"start":{"line":0,"character":7},"end":{"line":0,"character":7}},"text":"x"}]});
        assert_eq!(
            call(&mut s, "updateDocument", change)["error"]["code"],
            "invalid_position"
        );
        assert_eq!(
            call(
                &mut s,
                "query",
                json!({"kind":"compile","path":"/a.webtest"})
            ),
            before
        );
        assert_eq!(
            call(
                &mut s,
                "openDocument",
                json!({"path":"/a.webtest","text":"test \"ok\" {}","version":1})
            )["error"]["code"],
            "stale_version"
        );
        assert_eq!(
            call(&mut s, "what", Value::Null)["error"]["code"],
            "unknown_method"
        );
        assert_eq!(
            serde_json::from_str::<Value>(
                &s.request(r#"{"protocol":2,"id":1,"method":"query","params":{}}"#)
            )
            .unwrap()["error"]["code"],
            "unsupported_protocol"
        );
        assert_eq!(call(&mut s, "dispose", Value::Null)["ok"], true);
        assert_eq!(
            call(&mut s, "query", json!({"kind":"discovery","limit":10}))["error"]["code"],
            "workspace_closed"
        );
    }
}
