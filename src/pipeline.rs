use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::editor::{
    discover_editor_instances, is_unity_project, read_editor_descriptor, EditorInstance,
    PortDescriptor,
};
use crate::http::{http_get, http_post_json, probe_pipeline, HttpResponse};

/// Where to reach the Pipeline HTTP server for one Editor.
#[derive(Debug, Clone)]
pub struct Target {
    pub port: u16,
    pub project_path: PathBuf,
    pub eval_token: Option<String>,
    /// Guidance the Editor published about itself on its port descriptor (see
    /// [`crate::editor::PortDescriptor::info`]).
    pub info: Option<String>,
}

/// Slack added to this client's own wait so that a command outliving its budget fails with the
/// server's message ("Main thread operation timed out after Nms") instead of a bare socket read
/// timeout that says nothing about the cause.
const CLIENT_GRACE: Duration = Duration::from_secs(15);

/// How one `/api/exec` call is bounded on both sides of the wire.
///
/// The server aborts every main-thread command once the budget in the request (`timeout`, ms) is
/// exceeded, defaulting to 60s — so a client that never sends it is silently stuck with the
/// default no matter how long it is prepared to wait. Both numbers therefore travel together.
#[derive(Debug, Clone, Copy)]
pub struct ExecBudget {
    /// How long this client waits for the response.
    pub client: Duration,
    /// Budget for the command itself, sent as the request's `timeout` (ms). `None` leaves the
    /// server's own default in place.
    pub server_ms: Option<u64>,
}

impl ExecBudget {
    /// One budget for both sides. The client waits `CLIENT_GRACE` longer than the server is
    /// given, so the server is the one that reports a timeout, with the budget it names.
    pub fn shared(budget: Duration) -> Self {
        Self {
            client: budget + CLIENT_GRACE,
            server_ms: Some(budget.as_millis().max(1) as u64),
        }
    }

    /// Bound only this client's wait, letting the server apply its own budget.
    pub fn client_only(client: Duration) -> Self {
        Self {
            client,
            server_ms: None,
        }
    }
}

/// A single pipeline command as reported by `/api/commands`.
#[derive(Debug, Deserialize)]
pub struct PipelineCommand {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub parameters: Vec<PipelineParam>,
    /// The server's own generated JSON Schema for this command. Serialized by the server as a
    /// JSON *string* (`JObject.ToString()`), so it arrives as `Value::String`.
    #[serde(default)]
    pub schema: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct PipelineParam {
    #[serde(default)]
    pub name: Option<String>,
    /// The CLR type name as reported by the server (e.g. `Int32`, `String[]`, `List`1`).
    #[serde(default, rename = "type")]
    pub ty: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, rename = "defaultValue")]
    pub default_value: Option<Value>,
    #[serde(default)]
    pub order: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct PipelineCommands {
    #[serde(default)]
    pub commands: Vec<PipelineCommand>,
    #[serde(default)]
    pub count: Option<u64>,
    #[serde(default)]
    pub server: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct ExecEnvelope {
    #[serde(default)]
    pub success: Option<bool>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default, rename = "errorDetails")]
    pub error_details: Option<String>,
}

/// A non-2xx Pipeline response, with the server's structured error parsed out of the body.
///
/// The server answers every failure with a JSON envelope (`error`, `errorDetails`, and — for a
/// retryable busy reply — `status`/`retryable`/`busyReason`), so the body is worth reading: a
/// bare status code tells the caller nothing actionable.
#[derive(Debug)]
pub struct ServerError {
    pub status: u16,
    pub reason: String,
    pub message: String,
    /// True for the "server busy, retry shortly" replies (HTTP 503 while the Editor is settling
    /// or a modal dialog is blocking it).
    pub retryable: bool,
    /// The server's machine-readable cause for a busy reply: `"settling"` (clears by itself) or
    /// `"blocked_by_dialog"` (needs a human).
    pub busy_reason: Option<String>,
    /// The `Retry-After` header in seconds, when the server sent one.
    pub retry_after: Option<u64>,
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pipeline server returned {} {}", self.status, self.reason)?;
        if !self.message.is_empty() {
            write!(f, ": {}", self.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ServerError {}

/// Turn a non-2xx response into an error carrying the server's own error text.
pub fn server_error(resp: &HttpResponse) -> anyhow::Error {
    let reason = if resp.reason.is_empty() {
        "Error".to_owned()
    } else {
        resp.reason.clone()
    };
    let parsed: Value = serde_json::from_str(&resp.body).unwrap_or(Value::Null);
    let error = parsed.get("error").and_then(Value::as_str).unwrap_or("");
    let details = parsed
        .get("errorDetails")
        .and_then(Value::as_str)
        .unwrap_or("");
    let message = match (error.is_empty(), details.is_empty()) {
        (false, false) => format!("{error}. {details}"),
        (false, true) => error.to_owned(),
        (true, false) => details.to_owned(),
        // Nothing structured to report — fall back to a bounded snippet of the raw body.
        (true, true) => resp.body.chars().take(200).collect(),
    };

    anyhow!(ServerError {
        status: resp.status,
        reason,
        message,
        retryable: parsed
            .get("retryable")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        busy_reason: parsed
            .get("busyReason")
            .and_then(Value::as_str)
            .map(str::to_owned),
        retry_after: resp.retry_after,
    })
}

/// The HTTP status behind an error, when it came from the Pipeline server.
pub fn http_status(err: &anyhow::Error) -> Option<u16> {
    err.downcast_ref::<ServerError>().map(|e| e.status)
}

pub fn is_401(err: &anyhow::Error) -> bool {
    http_status(err) == Some(401)
}

/// How long to wait before retrying a "server busy" reply, or None when the caller should give
/// up instead. Clamped so a hostile/absent `Retry-After` cannot stall the client.
///
/// The server marks both busy causes retryable, but only one of them clears on its own:
/// `"settling"` ends when the post-startup import/compile finishes, whereas
/// `"blocked_by_dialog"` waits for a human to dismiss a modal — retrying that one would only
/// delay an error the caller has to act on (its message already says how).
pub fn retry_delay(err: &anyhow::Error) -> Option<u64> {
    let server = err.downcast_ref::<ServerError>()?;
    if !server.retryable || server.busy_reason.as_deref() == Some("blocked_by_dialog") {
        return None;
    }
    Some(server.retry_after.unwrap_or(1).clamp(1, 30))
}

/// The Editor's own guidance about a failure its state explains — currently, a modal dialog
/// blocking the main thread, which the server publishes on its port descriptor precisely so
/// clients can pass it on (see [`crate::editor::PortDescriptor::info`]). None when the failure
/// is something else, or when the Editor said nothing about itself.
pub fn host_note<'a>(target: &'a Target, err: &anyhow::Error) -> Option<&'a str> {
    let server = err.downcast_ref::<ServerError>()?;
    if server.busy_reason.as_deref() != Some("blocked_by_dialog") {
        return None;
    }
    target.info.as_deref()
}

/// Whether an error looks like a transport-level failure (the Editor's server went away, e.g.
/// mid-domain-reload or after a restart) rather than an answer from a live server. Such errors
/// are worth re-discovering the Editor for; a structured error reply is not.
pub fn is_transient(err: &anyhow::Error) -> bool {
    if err.downcast_ref::<ServerError>().is_some() {
        return false;
    }
    let msg = format!("{err:#}");
    [
        "connect ",
        "read failed",
        "malformed HTTP",
        "Connection reset",
        "Cannot connect to",
    ]
    .iter()
    .any(|needle| msg.contains(needle))
}

/// Resolve which Editor to talk to. Explicit `project_path` short-circuits
/// discovery; otherwise we require exactly one reachable Pipeline server.
pub fn find_target(project_path: Option<&str>) -> Result<Target> {
    let (path, descriptor) = if let Some(explicit) = project_path {
        let path = std::path::Path::new(explicit).to_path_buf();
        if !is_unity_project(&path) {
            return Err(anyhow!("not a Unity project: {}", path.display()));
        }
        let desc = read_editor_descriptor(&path).ok_or_else(|| {
            anyhow!(
                "No Pipeline instance found for project: {}. Make sure Unity Editor is running with the Pipeline package installed.",
                path.display()
            )
        })?;
        (path, desc)
    } else {
        pick_single_running_editor()?
    };
    Ok(Target {
        port: descriptor.port,
        project_path: path,
        eval_token: descriptor.eval_token,
        info: descriptor.info,
    })
}

fn pick_single_running_editor() -> Result<(PathBuf, PortDescriptor)> {
    let instances = discover_editor_instances()?;
    let available: Vec<&EditorInstance> = instances
        .iter()
        .filter(|inst| inst.has_pipeline && inst.is_reachable && inst.descriptor.is_some())
        .collect();
    match available.len() {
        0 => Err(anyhow!(
            "No Unity Editor instances found with reachable Pipeline servers.\n\nMake sure:\n\u{2022} Unity Editor is running with a project open\n\u{2022} The Pipeline package is installed in the project\n\u{2022} The Pipeline HTTP server is running\n\nYou can check available instances with: unity projects"
        )),
        1 => {
            let inst = available[0];
            let path = inst.project_path.clone();
            let desc = read_editor_descriptor(&path).ok_or_else(|| {
                anyhow!("Pipeline descriptor disappeared for {}", path.display())
            })?;
            Ok((path, desc))
        }
        _ => {
            let list = available
                .iter()
                .enumerate()
                .map(|(i, inst)| {
                    format!(
                        "  {}. {} (localhost:{}) - {}",
                        i + 1,
                        inst.project_name,
                        inst.descriptor.as_ref().map(|d| d.port).unwrap_or(0),
                        inst.project_path.display(),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            Err(anyhow!(
                "Multiple Unity Editor instances found with Pipeline servers:\n\n{list}\n\nPlease specify which instance to connect to:\n\u{2022} Use --project-path <path> to connect to a specific project"
            ))
        }
    }
}

/// Ensure the resolved Pipeline server actually answers.
pub fn ensure_reachable(target: &Target) -> Result<()> {
    if !probe_pipeline(target.port, target.eval_token.as_deref()) {
        return Err(anyhow!(
            "Cannot connect to Unity Editor Pipeline server at 127.0.0.1:{}. Make sure Unity Editor is running with the Pipeline package installed.",
            target.port
        ));
    }
    Ok(())
}

/// GET the Pipeline command catalog.
pub fn fetch_commands(target: &Target, timeout: Duration) -> Result<PipelineCommands> {
    let resp = http_get(
        target.port,
        "/api/commands",
        target.eval_token.as_deref(),
        timeout,
    )
    .map_err(|e| anyhow!("failed to fetch pipeline commands from 127.0.0.1:{}: {e}", target.port))?;
    if !resp.ok() {
        return Err(server_error(&resp));
    }
    serde_json::from_str::<PipelineCommands>(&resp.body)
        .map_err(|e| anyhow!("invalid JSON from /api/commands: {e}"))
}

/// POST a pipeline command via `/api/exec`, returning the unwrapped `.result`.
pub fn execute(
    target: &Target,
    command: &str,
    parameters: &Value,
    budget: ExecBudget,
) -> Result<Value> {
    let mut body = Map::new();
    body.insert("command".to_owned(), Value::String(command.to_owned()));
    body.insert("parameters".to_owned(), parameters.clone());
    if let Some(ms) = budget.server_ms {
        // The server's main-thread wait budget; it rejects a non-positive value.
        body.insert("timeout".to_owned(), Value::Number(ms.into()));
    }
    let payload = serde_json::to_string(&Value::Object(body))?;
    let resp = http_post_json(
        target.port,
        "/api/exec",
        target.eval_token.as_deref(),
        &payload,
        budget.client,
    )?;
    exec_result(&resp)
}

/// Interpret one `/api/exec` response: the unwrapped `.result` on success, the server's own
/// error text otherwise.
pub fn exec_result(resp: &HttpResponse) -> Result<Value> {
    if !resp.ok() {
        return Err(server_error(resp));
    }
    let parsed: ExecEnvelope = serde_json::from_str(&resp.body)
        .map_err(|e| anyhow!("Invalid response format from Pipeline server: {e}"))?;
    if parsed.success == Some(false) {
        let mut msg = parsed.error.unwrap_or_else(|| "Unknown error".to_owned());
        if let Some(details) = parsed.error_details {
            msg.push_str(". ");
            msg.push_str(&details);
        }
        return Err(anyhow!("Command execution failed: {msg}"));
    }
    Ok(parsed.result.unwrap_or(Value::Null))
}

/// Failed C# eval envelope: `success` field explicitly false.
pub fn is_failed_eval_envelope(result: &Value) -> bool {
    result
        .get("success")
        .and_then(|v| v.as_bool())
        .map(|b| !b)
        .unwrap_or(false)
}

/// Some Unity Pipeline endpoints double-encode their payload (return the JSON as a
/// `Value::String` rather than a `Value::Object`). Normalize so the completion predicates can
/// always index by field name.
pub fn normalize_status(status: &Value) -> std::borrow::Cow<'_, Value> {
    if let Some(s) = status.as_str() {
        if let Ok(v) = serde_json::from_str::<Value>(s) {
            return std::borrow::Cow::Owned(v);
        }
    }
    std::borrow::Cow::Borrowed(status)
}

/// The `inputSchema` for one MCP tool. Prefers the schema the server generates for the command
/// (it knows the real parameter types) and only falls back to deriving one from the parameter
/// list when the server did not supply it.
pub fn command_input_schema(cmd: &PipelineCommand) -> Value {
    server_schema_object(cmd).unwrap_or_else(|| fallback_input_schema(cmd))
}

/// Keys of the server's generated schema document that are not part of an MCP tool's
/// `inputSchema`: the schema's own metadata, and the command description (which the tool
/// already carries at the top level).
const NON_INPUT_SCHEMA_KEYS: [&str; 4] = ["$schema", "title", "description", "x-command-metadata"];

/// The server's own command schema, stripped of the keys that belong to the JSON Schema
/// document itself rather than to a tool's `inputSchema`.
fn server_schema_object(cmd: &PipelineCommand) -> Option<Value> {
    let raw = cmd.schema.as_ref()?;
    let parsed = match raw {
        // The server serializes the schema with `JObject.ToString()`, so it lands as a string.
        Value::String(s) => serde_json::from_str::<Value>(s).ok()?,
        other => other.clone(),
    };
    let mut obj = parsed.as_object()?.clone();
    for key in NON_INPUT_SCHEMA_KEYS {
        obj.shift_remove(key);
    }
    if !obj.contains_key("type") {
        obj.insert("type".to_owned(), Value::String("object".to_owned()));
    }
    Some(Value::Object(obj))
}

/// Schema derived from the command's parameter list, for servers too old to send one.
fn fallback_input_schema(cmd: &PipelineCommand) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for param in &cmd.parameters {
        let Some(pname) = param.name.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        let mut prop = match param.ty.as_deref() {
            Some(ty) => type_schema(ty),
            None => json!({ "type": "string" }),
        };
        if let Some(obj) = prop.as_object_mut() {
            if let Some(desc) = &param.description {
                obj.insert("description".to_owned(), Value::String(desc.clone()));
            }
            if let Some(default) = &param.default_value {
                obj.insert("default".to_owned(), default.clone());
            }
        }
        properties.insert(pname.to_owned(), prop);
        if param.required {
            required.push(Value::String(pname.to_owned()));
        }
    }

    let mut schema = Map::new();
    schema.insert("type".to_owned(), Value::String("object".to_owned()));
    schema.insert("properties".to_owned(), Value::Object(properties));
    schema.insert("required".to_owned(), Value::Array(required));
    schema.insert("additionalProperties".to_owned(), Value::Bool(false));
    Value::Object(schema)
}

/// The JSON Schema `type` name for a CLR type name as reported by `/api/commands`.
pub fn pipeline_type_to_json_schema(ty: Option<&str>) -> String {
    let Some(ty) = ty else {
        return "string".to_owned();
    };
    type_schema(ty)
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("string")
        .to_owned()
}

/// Map a CLR type name as reported by `/api/commands` (e.g. `Int32`, `Single`, `String[]`,
/// ``List`1``) onto the JSON Schema fragment for it.
fn type_schema(ty: &str) -> Value {
    let (base, dims) = strip_array_suffix(ty);
    if dims > 0 {
        let mut inner = type_schema(&base);
        for _ in 0..dims {
            inner = json!({ "type": "array", "items": inner });
        }
        return inner;
    }

    let lower = base.to_ascii_lowercase();
    if lower.starts_with("dictionary") || lower.starts_with("idictionary") {
        return json!({ "type": "object" });
    }
    let json_type = match lower.as_str() {
        "boolean" => "boolean",
        "int32" | "int64" | "int16" | "byte" | "uint32" | "uint64" | "uint16" | "sbyte" => "integer",
        "single" | "double" | "decimal" | "float" => "number",
        "object" => "object",
        _ => "string",
    };
    json!({ "type": json_type })
}

/// Split `Foo[]`, `Foo[][]` and ``List`1`` into (element type name, array depth). A generic
/// collection's element type is not recoverable from the CLR type *name* alone, so the element
/// falls back to the default scalar.
fn strip_array_suffix(ty: &str) -> (String, usize) {
    let mut base = ty.trim().to_owned();
    let mut dims = 0;
    loop {
        if let Some(rest) = base.strip_suffix("[]") {
            base = rest.trim_end().to_owned();
            dims += 1;
            continue;
        }
        let generic = base
            .split('`')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(
            generic.as_str(),
            "list" | "ilist" | "ienumerable" | "icollection" | "hashset" | "queue" | "stack"
        ) {
            dims += 1;
            base = String::new();
        }
        break;
    }
    (base, dims)
}
