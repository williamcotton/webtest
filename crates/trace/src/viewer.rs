//! Read-only presentation of verified facts. No project execution, external
//! assets, JavaScript, or interpretation of DSL syntax is involved.
use crate::VerifiedTrace;
use base64::{display::Base64Display, engine::general_purpose::STANDARD};
use std::{
    collections::BTreeMap,
    fmt::{self, Write},
};
use webtest_observation::{ExecutionEvent, RecordedEvent};

pub const VIEWER_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'";

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("trace HTML exceeds its {max_bytes}-byte limit")]
    TooLarge { max_bytes: usize },
    #[error("trace fact could not be rendered: {0}")]
    Json(#[from] serde_json::Error),
}

struct Html {
    text: String,
    max_bytes: usize,
}
impl Write for Html {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.text.len().saturating_add(text.len()) > self.max_bytes {
            return Err(fmt::Error);
        }
        self.text.push_str(text);
        Ok(())
    }
}
impl Html {
    fn add(&mut self, args: fmt::Arguments<'_>) -> Result<(), RenderError> {
        self.write_fmt(args).map_err(|_| RenderError::TooLarge {
            max_bytes: self.max_bytes,
        })
    }
    fn json(&mut self, value: &impl serde::Serialize) -> Result<(), RenderError> {
        self.add(format_args!(
            "<pre>{}</pre>",
            Escape(&serde_json::to_string_pretty(value)?)
        ))
    }
}

struct Escape<'a>(&'a str);
impl fmt::Display for Escape<'_> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        for ch in self.0.chars() {
            match ch {
                '&' => out.write_str("&amp;"),
                '<' => out.write_str("&lt;"),
                '>' => out.write_str("&gt;"),
                '"' => out.write_str("&quot;"),
                '\'' => out.write_str("&#39;"),
                _ => out.write_char(ch),
            }?;
        }
        Ok(())
    }
}

/// Produce a self-contained, script-free document. Limits include escaped text
/// and encoded attachments; failure returns no partial HTML. Facts are displayed
/// in per-execution journal order, not sorted by wall clock or inferred severity.
pub fn render_html(trace: &VerifiedTrace, max_bytes: usize) -> Result<String, RenderError> {
    let mut out = Html {
        text: String::new(),
        max_bytes,
    };
    out.add(format_args!("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"{}\"><meta name=\"referrer\" content=\"no-referrer\"><title>WebTest trace</title><style>{}</style></head><body><header><p class=\"muted\">WEBTEST / TRACE</p><h1>Execution record</h1><p>{} executions · {} recorded events</p><nav><a href=\"#timeline\">Timeline</a><a href=\"#scopes\">Branches and scopes</a><a href=\"#sources\">Sources</a><a href=\"#attachments\">Evidence</a><a href=\"#manifest\">Manifest</a></nav></header><main>", Escape(VIEWER_CSP), include_str!("viewer.css"), trace.manifest.executions.len(), trace.records.len()))?;
    out.add(format_args!("<section id=\"timeline\"><h2>Timeline</h2><p class=\"muted\">Elapsed time is local to each execution. Expand an event for its complete recorded fact, including wall time and typed failure details.</p>"))?;
    if let Some(report) = &trace.manifest.capture_report
        && !report.events.is_empty()
    {
        out.add(format_args!("<aside class=\"panel failure\"><h2>Capture truncated</h2><p>{} content members were omitted to meet trace capture limits. The recorded runtime journal is unchanged.</p><details><summary>Capture budget and truncation facts</summary>", report.events.len()))?;
        out.json(report)?;
        out.add(format_args!("</details></aside>"))?;
    }
    let mut executions = BTreeMap::new();
    let mut scopes = BTreeMap::new();
    let mut sources = BTreeMap::new();
    for source in &trace.manifest.sources {
        sources.insert((source.execution_id, source.file_id.get()), source);
    }
    let mut actors = BTreeMap::new();
    for record in &trace.records {
        if let ExecutionEvent::Semantic {
            event: webtest_observation::SemanticEvent::ActorOpened { actor, name },
            ..
        } = &record.event
        {
            actors.insert((record.identity.execution_id, *actor), name.clone());
        }
    }
    for record in &trace.records {
        executions
            .entry(record.identity.execution_id)
            .or_insert_with(Vec::new)
            .push(record);
        if let Some(id) = record.metadata.execution_context.scope_id {
            scopes
                .entry((record.identity.execution_id, id.0))
                .or_insert(record);
        }
    }
    for (execution, records) in executions {
        out.add(format_args!("<details class=\"panel\" open><summary>Execution <code>{execution}</code> · {} events</summary><table><thead><tr><th>Elapsed / sequence</th><th>Task / occurrence</th><th>Recorded event</th></tr></thead><tbody>", records.len()))?;
        let maximum = records
            .iter()
            .map(|r| r.timestamp.elapsed)
            .max()
            .unwrap_or_default()
            .as_secs_f64()
            .max(0.001);
        for record in records {
            let sequence = record.identity.event_sequence.0;
            let context = &record.metadata.execution_context;
            out.add(format_args!("<tr id=\"event-{execution}-{sequence}\" class=\"{}\"><td>{:.3} s · #{sequence}<meter min=\"0\" max=\"{maximum}\" value=\"{}\" aria-label=\"Elapsed time\"></meter></td><td>", if record.event.failure_code().is_some() { "failure" } else { "fact" }, record.timestamp.elapsed.as_secs_f64(), record.timestamp.elapsed.as_secs_f64()))?;
            let actor = context.actor.or_else(|| {
                if let ExecutionEvent::Semantic { event, .. } = &record.event {
                    event.actor()
                } else {
                    None
                }
            });
            if let Some(actor) = actor {
                out.add(format_args!(
                    "actor {} <code>{actor}</code><br>",
                    Escape(
                        actors
                            .get(&(execution, actor))
                            .map(String::as_str)
                            .unwrap_or("(name not recorded)")
                    )
                ))?;
            }
            if let Some(test) = context.test_id {
                out.add(format_args!("test {}<br>", test.0))?;
            }
            if let Some(path) = &context.task_path {
                out.add(format_args!("task {:?}<br>", path))?;
            }
            if let Some(scope) = context.scope_id {
                out.add(format_args!(
                    "<a href=\"#scope-{execution}-{}\">scope {}</a><br>",
                    scope.0, scope.0
                ))?;
            }
            if let Some(attempt) = context.attempt_id {
                out.add(format_args!("attempt {}<br>", attempt.0))?;
            }
            if let Some(operation) = context.operation_execution_id {
                out.add(format_args!("operation {}<br>", operation.0))?;
            }
            out.add(format_args!(
                "</td><td><details><summary>{}</summary>",
                Escape(&label(record))
            ))?;
            if let Some(code) = record.event.failure_code() {
                out.add(format_args!(
                    "<p><code>{}</code></p>",
                    code.diagnostic_code()
                ))?;
            }
            if let Some(origin) = record.metadata.origin
                && let Some(source) = sources.get(&(execution, origin.file.get()))
            {
                out.add(format_args!(
                    "<p><a href=\"#source-{execution}-{}\">{}</a> · bytes {}–{}</p>",
                    origin.file.get(),
                    Escape(&source.project_path),
                    u32::from(origin.range.start()),
                    u32::from(origin.range.end())
                ))?;
            }
            if let ExecutionEvent::AttachmentCreated { .. } = &record.event {
                out.add(format_args!(
                    "<p><a href=\"#attachment-{execution}-{sequence}\">Captured evidence</a></p>"
                ))?;
            }
            if let ExecutionEvent::OutputCaptured { output, .. } = &record.event {
                out.add(format_args!("<pre>{}</pre>", Escape(&output.text)))?;
            }
            out.json(record)?;
            out.add(format_args!("</details></td></tr>"))?;
        }
        out.add(format_args!("</tbody></table></details>"))?;
    }
    out.add(format_args!("</section><section id=\"scopes\"><h2>Branches and scopes</h2><p class=\"muted\">Parentage comes from explicit recorded scope identities. Missing facts remain missing.</p><div class=\"panel\"><table><thead><tr><th>Execution / scope</th><th>Parent</th><th>Task</th><th>Attempt / operation</th></tr></thead><tbody>"))?;
    for ((execution, id), record) in &scopes {
        let context = &record.metadata.execution_context;
        out.add(format_args!("<tr id=\"scope-{execution}-{id}\"><td><code>{execution}</code><br><a href=\"#event-{execution}-{}\">scope {id}</a></td><td>", record.identity.event_sequence.0))?;
        if let Some(parent) = context.parent_scope_id {
            if scopes.contains_key(&(*execution, parent.0)) {
                out.add(format_args!(
                    "<a href=\"#scope-{execution}-{}\">{}</a>",
                    parent.0, parent.0
                ))?;
            } else {
                out.add(format_args!("{} (not recorded)", parent.0))?;
            }
        } else {
            out.add(format_args!("root"))?;
        }
        if let Some(actor) = context.actor {
            out.add(format_args!(
                "</td><td>actor {} <code>{actor}</code><br>",
                Escape(
                    actors
                        .get(&(*execution, actor))
                        .map(String::as_str)
                        .unwrap_or("(name not recorded)")
                )
            ))?;
        } else {
            out.add(format_args!("</td><td>"))?;
        }
        out.add(format_args!(
            "{:?}</td><td>",
            context.task_path.as_deref().unwrap_or_default()
        ))?;
        if let Some(attempt) = context.attempt_id {
            out.add(format_args!("attempt {} ", attempt.0))?;
        }
        if let Some(operation) = context.operation_execution_id {
            out.add(format_args!("operation {}", operation.0))?;
        }
        out.add(format_args!("</td></tr>"))?;
    }
    out.add(format_args!(
        "</tbody></table></div></section><section id=\"sources\"><h2>Sources</h2>"
    ))?;
    for source in &trace.manifest.sources {
        let execution = source.execution_id;
        let file = source.file_id.get();
        out.add(format_args!("<details class=\"panel\" id=\"source-{execution}-{file}\"><summary>{}</summary><p><code>{execution}</code> · revision <code>{}</code></p>", Escape(&source.project_path), blake3::Hash::from(*source.revision.as_bytes()).to_hex()))?;
        if let Some(file) = &source.file {
            // The verifier has already checked UTF-8, content identity, and paths.
            if let Some(bytes) = trace.files.get(&file.path) {
                out.add(format_args!("<pre class=\"source\">"))?;
                for (index, line) in String::from_utf8_lossy(bytes).split('\n').enumerate() {
                    let ordinal = index + 1;
                    out.add(format_args!("<span id=\"source-{execution}-{}-line-{ordinal}\"><a href=\"#source-{execution}-{}-line-{ordinal}\">{ordinal}</a>{}</span>", source.file_id.get(), source.file_id.get(), Escape(line)))?;
                }
                out.add(format_args!("</pre>"))?;
            }
        } else if trace.manifest.metadata.capture.sources {
            out.add(format_args!(
                "<p>Source bytes were omitted because the trace capture budget was exceeded.</p>"
            ))?;
        } else {
            out.add(format_args!("<p>Source content was not included.</p>"))?;
        }
        out.add(format_args!("</details>"))?;
    }
    out.add(format_args!(
        "</section><section id=\"attachments\"><h2>Evidence</h2>"
    ))?;
    for attachment in &trace.manifest.attachments {
        let execution = attachment.event.execution_id;
        let sequence = attachment.event.event_sequence.0;
        out.add(format_args!("<details class=\"panel\" id=\"attachment-{execution}-{sequence}\"><summary>Attachment · event #{sequence}</summary><a href=\"#event-{execution}-{sequence}\">Recorded capture</a>"))?;
        if let Some(file) = &attachment.file {
            if let Some(bytes) = trace.files.get(&file.path) {
                out.add(format_args!(
                    "<p>{} bytes · BLAKE3 <code>{}</code></p>",
                    bytes.len(),
                    blake3::Hash::from(file.blake3).to_hex()
                ))?;
                if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                    out.add(format_args!(
                        "<img alt=\"Captured screenshot\" src=\"data:image/png;base64,{}\">",
                        Base64Display::new(bytes, &STANDARD)
                    ))?;
                } else if let Ok(text) = std::str::from_utf8(bytes) {
                    out.add(format_args!("<pre>{}</pre>", Escape(text)))?;
                }
                out.add(format_args!("<p><a download=\"attachment-{execution}-{sequence}.bin\" href=\"data:application/octet-stream;base64,{}\">Download captured bytes</a></p>", Base64Display::new(bytes, &STANDARD)))?;
            }
        } else if trace.manifest.metadata.capture.attachments {
            out.add(format_args!("<p>Attachment bytes were omitted because the trace capture budget was exceeded.</p>"))?;
        } else {
            out.add(format_args!("<p>Attachment bytes were not included.</p>"))?;
        }
        out.add(format_args!("</details>"))?;
    }
    out.add(format_args!("</section><details class=\"panel\" id=\"manifest\"><summary>Manifest and capture policy</summary>"))?;
    out.json(&trace.manifest)?;
    out.add(format_args!("</details></main></body></html>"))?;
    Ok(out.text)
}

fn label(record: &RecordedEvent) -> String {
    match &record.event {
        ExecutionEvent::FixtureAcquired {
            fixture,
            acquisition_execution_id,
            ..
        } => format!(
            "Fixture {} instance {} acquired from {} ({:?})",
            fixture.fixture, fixture.instance.0, acquisition_execution_id, fixture.owner
        ),
        ExecutionEvent::Scope { event, .. } => format!(
            "{} {}",
            event.kind(),
            event.outcome.map(|o| format!("{o:?}")).unwrap_or_default()
        ),
        ExecutionEvent::Semantic { event, .. } => {
            serde_json::to_string(event).unwrap_or_else(|_| event.kind().into())
        }
        ExecutionEvent::Attempt { event, scope, .. } => format!(
            "Attempt {}/{} · {}",
            event.ordinal,
            event.max_attempts,
            scope
                .outcome
                .map(|o| format!("{o:?}"))
                .unwrap_or_else(|| "started".into())
        ),
        ExecutionEvent::Resource { event, .. } => format!(
            "{} · {:?} #{} generation {}",
            event.kind.name(),
            event.resource.resource_kind,
            event.resource.key.resource_id.0,
            event.resource.key.generation_id.0
        ),
        ExecutionEvent::Wait { event, .. } => format!("Wait {:?}", event.kind),
        ExecutionEvent::TestStarted { name, .. } => format!("Test started · {name}"),
        ExecutionEvent::TestSkipped { name, reason, .. } => {
            format!("Test skipped · {name} · {reason:?}")
        }
        ExecutionEvent::TestFinished { outcome, .. } => format!("Test finished · {outcome:?}"),
        ExecutionEvent::RunStarted { .. } => "Execution started".into(),
        ExecutionEvent::RunFinished { outcome, .. } => format!("Execution finished · {outcome:?}"),
        ExecutionEvent::CancellationRequested { cause, .. } => format!(
            "Cancellation requested · {:?} · caused by scope {}",
            cause.reason, cause.causing_scope_id.0
        ),
        ExecutionEvent::ScopeAborted { .. } => "Scope aborted".into(),
        ExecutionEvent::OutputCaptured {
            provider,
            operation,
            output,
            ..
        } => format!("{provider}.{operation} · {:?}", output.stream),
        ExecutionEvent::AttachmentCreated { attachment, .. } => {
            format!("Attachment created · {:?}", attachment.artifact.kind)
        }
        ExecutionEvent::ProviderCallStarted {
            provider,
            operation,
            ..
        } => format!("{provider}.{operation} started"),
        ExecutionEvent::ProviderCallFinished {
            provider,
            operation,
            ..
        } => format!("{provider}.{operation} finished"),
        ExecutionEvent::ProviderCallFailed {
            provider,
            operation,
            ..
        } => format!("{provider}.{operation} failed"),
        ExecutionEvent::StepStarted { .. } => "Step started".into(),
        ExecutionEvent::StepPassed { .. } => "Step passed".into(),
        ExecutionEvent::StepFailed { .. } => "Assertion or operation failed".into(),
        ExecutionEvent::TestTimedOut { .. } => "Test timed out".into(),
        ExecutionEvent::CleanupFailed { .. } => "Cleanup failed".into(),
    }
}
