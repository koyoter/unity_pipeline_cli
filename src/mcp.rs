use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::pipeline::{
    self, command_input_schema, ensure_reachable, execute, fetch_commands, find_target,
    is_failed_eval_envelope, is_transient, normalize_status, retry_delay, ExecBudget, Target,
};

/// Newest protocol revision this server speaks; also the fallback when a client asks for one we
/// don't know.
const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];
const SERVER_NAME: &str = "unity-mcp";
const SERVER_VERSION: &str = "1.0.0-rust";
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const RECOMPILE_POLL_TIMEOUT: Duration = Duration::from_secs(120);
/// Default budget for one tool call, applied to both sides of the wire (`tool_budget`).
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
/// Status polls are short reads of an already-running command, not command executions.
const POLL_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const CATALOG_TIMEOUT: Duration = Duration::from_secs(10);
/// How many times to re-send a request the server answered with a retryable "busy" reply
/// (Editor settling after startup, or blocked by a modal dialog) before giving up.
const BUSY_RETRY_ATTEMPTS: u32 = 30;
/// Wall-clock cap on riding out "editor busy" retries for one tool call, override with
/// UNITY_MCP_MAX_WAIT_SECS. The MCP host runs its own call timeout and drops or kills a server
/// that sits silent past it, so we must answer before that even while the editor is busy.
const BUSY_WAIT_DEFAULT: u64 = 60;

pub struct Options {
    pub project_path: Option<String>,
}

/// When UNITY_MCP_TRACE names a file, every protocol line in and out, every busy retry and any
/// panic land there with a millisecond timestamp — the only record that survives when a host
/// kills the server from outside. Off unless the variable is set; `install --trace` and
/// `configure mcp --trace` print a config snippet that points it at the exe-side log.
static TRACE_FILE: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Long tool results (base64 screenshots especially) are clipped so one capture_game_view call
/// cannot balloon the log.
const TRACE_LINE_LIMIT: usize = 2000;

fn trace_init() -> Option<&'static PathBuf> {
    let path = std::env::var_os("UNITY_MCP_TRACE")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    if path.is_some() {
        std::panic::set_hook(Box::new(|info| {
            trace(&format!(
                "!! panic: {info}\n{}",
                std::backtrace::Backtrace::force_capture()
            ));
        }));
    }
    let _ = TRACE_FILE.set(path);
    TRACE_FILE.get().and_then(|p| p.as_ref())
}

fn trace(msg: &str) {
    let Some(path) = TRACE_FILE.get().and_then(|p| p.as_ref()) else {
        return;
    };
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    // UTC on both sides of the epoch stamp: the ISO form lines up with Unity/host logs,
    // the raw millis keeps existing greps and duration math working.
    let stamp = iso8601_utc(ms);
    if msg.len() > TRACE_LINE_LIMIT {
        let mut end = TRACE_LINE_LIMIT;
        while !msg.is_char_boundary(end) {
            end -= 1;
        }
        let _ = writeln!(
            file,
            "{stamp} {ms} {}… [+{} bytes truncated]",
            &msg[..end],
            msg.len() - TRACE_LINE_LIMIT
        );
    } else {
        let _ = writeln!(file, "{stamp} {ms} {msg}");
    }
}

/// UTC ISO-8601 with milliseconds, from Unix epoch millis. No local time zone: the standard
/// library cannot read it without a platform dependency, and a fixed offset is what log
/// correlation needs anyway.
fn iso8601_utc(epoch_ms: u128) -> String {
    let secs = (epoch_ms / 1000) as i64;
    let millis = (epoch_ms % 1000) as u32;
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's civil_from_days).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Short, non-reversible fingerprint so traces can tell token rotations apart without writing
/// the secret itself to disk.
fn token_fingerprint(token: Option<&str>) -> String {
    let Some(token) = token.filter(|t| !t.is_empty()) else {
        return "none".to_owned();
    };
    use sha1::{Digest, Sha1};
    format!("{:x}", Sha1::digest(token.as_bytes()))
        .chars()
        .take(8)
        .collect()
}

/// Effective busy-wait budget for one tool call.
fn max_wait_secs() -> u64 {
    std::env::var("UNITY_MCP_MAX_WAIT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(BUSY_WAIT_DEFAULT)
}

pub fn run(opts: Options) -> Result<()> {
    let state = Mutex::new(None::<Target>);
    // Tool names from the last successful catalog fetch, so a change can be announced.
    let known_tools = Mutex::new(None::<HashSet<String>>);
    // Last full catalog, served instead of an empty list when a refresh fails: emptying the
    // host's tool set is what turns a recoverable outage into "MCP tool is not found".
    let last_tools = Mutex::new(None::<Vec<Value>>);
    let initialized = AtomicBool::new(false);
    let stop = AtomicBool::new(false);
    let stdout = std::io::stdout();

    // Announce readiness on stderr — stdout carries only protocol messages.
    let trace_path = trace_init();
    trace(&format!(
        "== start pid={} v{SERVER_VERSION} busy-wait-cap={}s project_path={:?} trace={}",
        std::process::id(),
        max_wait_secs(),
        opts.project_path,
        trace_path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "off".to_owned())
    ));
    eprintln!("unity mcp: server started on stdio, waiting for client requests");

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut line = String::new();

    while !stop.load(Ordering::Relaxed) {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                trace("client closed stdin");
                break; // client closed stdin
            }
            Ok(_) => {}
            Err(err) => {
                trace(&format!("!! stdin read error: {err}"));
                break;
            }
        }
        // A UTF-8 BOM in front of the first message is not valid JSON but is what some hosts
        // (e.g. a PowerShell script driving the server through its default stdin writer) emit;
        // dropping it keeps the first request from being answered with a spurious parse error.
        let trimmed = line
            .trim_end_matches(['\r', '\n'])
            .trim_start_matches('\u{feff}')
            .to_owned();
        if trimmed.is_empty() {
            continue;
        }
        trace(&format!("→ {trimmed}"));

        let msg: Value = match serde_json::from_str(&trimmed) {
            Ok(v) => v,
            Err(err) => {
                let resp = error_response(Value::Null, -32700, &format!("parse error: {err}"));
                write_message(&stdout, &resp)?;
                continue;
            }
        };

        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();

        // Notifications carry no id — no response to write.
        if msg.get("id").is_none() {
            if method == "notifications/initialized" {
                initialized.store(true, Ordering::Relaxed);
            }
            continue;
        }

        match method.as_str() {
            "initialize" => {
                let response = handle_initialize(id.clone(), &msg);
                write_message(&stdout, &response)?;
            }
            "ping" => write_message(&stdout, &success_response(id.clone(), json!({})))?,
            "tools/list" => {
                let (response, changed) =
                    handle_tools_list(id.clone(), &state, &opts, &known_tools, &last_tools);
                write_message(&stdout, &response)?;
                if changed && initialized.load(Ordering::Relaxed) {
                    write_message(
                        &stdout,
                        &json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }),
                    )?;
                }
            }
            "tools/call" => {
                let response = handle_tools_call(id.clone(), &msg, &state, &opts);
                write_message(&stdout, &response)?;
            }
            "shutdown" => {
                stop.store(true, Ordering::Relaxed);
                write_message(&stdout, &success_response(id.clone(), Value::Null))?;
            }
            other => {
                let response = error_response(id.clone(), -32601, &format!("method not found: {other}"));
                write_message(&stdout, &response)?;
            }
        }
        if method == "shutdown" {
            break;
        }
    }

    trace("== exit");
    Ok(())
}

fn write_message(stdout: &std::io::Stdout, msg: &Value) -> Result<()> {
    let text = crate::json::compact(msg);
    trace(&format!("← {text}"));
    let mut lock = stdout.lock();
    let sent = lock
        .write_all(text.as_bytes())
        .and_then(|_| lock.write_all(b"\n"))
        .and_then(|_| lock.flush());
    if let Err(err) = sent {
        // The host closed its side: nothing left to serve, but the trace keeps the reason.
        trace(&format!("!! stdout write failed: {err}"));
        return Err(err.into());
    }
    Ok(())
}

fn success_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        }
    })
}

/// Answer the handshake with the client's own protocol revision when we speak it, and advertise
/// that the tool list can change at runtime (new commands appear after a domain reload).
fn handle_initialize(id: Value, msg: &Value) -> Value {
    let requested = msg
        .pointer("/params/protocolVersion")
        .and_then(|v| v.as_str());
    let version = match requested {
        Some(v) if SUPPORTED_PROTOCOL_VERSIONS.contains(&v) => v,
        _ => PROTOCOL_VERSION,
    };
    success_response(
        id,
        json!({
            "protocolVersion": version,
            "capabilities": {
                "tools": { "listChanged": true }
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION,
            }
        }),
    )
}

/// `(response, tool set changed since the last successful fetch)`.
fn handle_tools_list(
    id: Value,
    state: &Mutex<Option<Target>>,
    opts: &Options,
    known_tools: &Mutex<Option<HashSet<String>>>,
    last_tools: &Mutex<Option<Vec<Value>>>,
) -> (Value, bool) {
    match refresh_tools(state, opts, known_tools, last_tools) {
        Ok((tools, changed)) => (success_response(id, json!({ "tools": tools })), changed),
        Err(err) => {
            // An unreachable Editor is a normal state to start in, not a protocol error: hand
            // back an empty catalog (so the client still completes initialize + list) and let a
            // later call re-discover the Editor.
            invalidate(state);
            trace(&format!("stderr: {err:#}"));
            eprintln!("unity mcp: {err:#}");
            // Once a catalog has been served, never answer with an empty one: hosts treat that
            // as every tool being unregistered ("MCP tool is not found") and do not re-list on
            // their own. The stale list keeps calls working until the next discovery succeeds.
            let stale = last_tools.lock().unwrap().clone();
            match stale {
                Some(tools) if !tools.is_empty() => {
                    trace(&format!(
                        ".. serving cached catalog ({} tools) until the editor is reachable",
                        tools.len()
                    ));
                    (success_response(id, json!({ "tools": tools })), false)
                }
                _ => {
                    trace(".. no catalog served yet; answering with an empty list");
                    (success_response(id, json!({ "tools": [] })), false)
                }
            }
        }
    }
}

fn handle_tools_call(
    id: Value,
    msg: &Value,
    state: &Mutex<Option<Target>>,
    opts: &Options,
) -> Value {
    let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    if name.is_empty() {
        return error_response(id, -32602, "invalid params: tools/call requires a name");
    }
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

    match execute_tool_with_retry(&name, &args, state, opts) {
        Ok(result) => success_response(id, result),
        Err(err) => {
            // Report tool errors inside the result envelope, not as JSON-RPC
            // errors — matches the MCP spec and the JS implementation.
            let mut msg = format!("Error: {err:#}");
            if let Some(note) = host_note(state, &err) {
                msg.push_str("\nNote: ");
                msg.push_str(&note);
            }
            let redacted = redact_token(&msg, state);
            success_response(
                id,
                json!({
                    "content": [{
                        "type": "text",
                        "text": redacted,
                    }],
                    "isError": true,
                }),
            )
        }
    }
}

/// The cached Editor's own guidance about a failure its state explains. Read from the cached
/// target, which is still populated for a busy failure (only transport failures invalidate it).
fn host_note(state: &Mutex<Option<Target>>, err: &anyhow::Error) -> Option<String> {
    let guard = state.lock().unwrap();
    guard
        .as_ref()
        .and_then(|target| pipeline::host_note(target, err))
        .map(str::to_owned)
}

fn refresh_tools(
    state: &Mutex<Option<Target>>,
    opts: &Options,
    known_tools: &Mutex<Option<HashSet<String>>>,
    last_tools: &Mutex<Option<Vec<Value>>>,
) -> Result<(Vec<Value>, bool)> {
    let target = ensure_connection(state, opts)?;
    // Re-fetch the catalog on every list, mirroring the JS behavior so that
    // domain reloads picking up new commands surface promptly.
    let tools = match fetch_tools(&target) {
        Ok(tools) => tools,
        Err(first_err) => {
            // The cached target carries the eval token read when it was discovered. A domain
            // reload or Editor restart replaces the descriptor and rotates that token, which
            // answers 401 — drop the stale target and rediscover before giving up, otherwise
            // the host is handed an empty catalog and every tool reads as "not found".
            trace(&format!(".. tools/list: {first_err:#}; rediscovering editor"));
            invalidate(state);
            let fresh = ensure_connection(state, opts).map_err(|err| {
                trace(&format!(".. rediscovery failed: {err:#}"));
                err
            })?;
            match fetch_tools(&fresh) {
                Ok(tools) => {
                    trace(".. rediscovered editor; catalog refreshed");
                    tools
                }
                Err(err) => {
                    invalidate(state);
                    trace(&format!(".. rediscovered editor but catalog fetch failed: {err:#}"));
                    return Err(err);
                }
            }
        }
    };

    let names: HashSet<String> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(|v| v.as_str()).map(str::to_owned))
        .collect();
    let changed = {
        let mut guard = known_tools.lock().unwrap();
        // A first successful fetch counts as a change: the client may have been handed an empty
        // list before this (Unity wasn't up yet at handshake time), so it is told to re-read.
        let changed = guard.as_ref() != Some(&names) && !names.is_empty();
        *guard = Some(names);
        changed
    };
    *last_tools.lock().unwrap() = Some(tools.clone());
    Ok((tools, changed))
}

/// Run one tool, transparently recovering from the two states that are not real failures:
/// a connection this process has gone stale on (the Editor restarted or reloaded its domain),
/// and a server that says it is busy and asks to be retried shortly.
fn execute_tool_with_retry(
    name: &str,
    args: &Value,
    state: &Mutex<Option<Target>>,
    opts: &Options,
) -> Result<Value> {
    let wait_budget = Duration::from_secs(max_wait_secs());
    let started = Instant::now();
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match execute_tool_once(name, args, state, opts) {
            Ok(v) => {
                trace(&format!(
                    ".. {name}: ok in {:.0}ms",
                    started.elapsed().as_secs_f64() * 1000.0
                ));
                return Ok(v);
            }
            Err(err) => {
                if (pipeline::is_401(&err) || is_transient(&err)) && attempt <= 2 {
                    invalidate(state);
                    continue;
                }
                if let Some(delay) = retry_delay(&err) {
                    let waited = started.elapsed();
                    // The MCP host times this call out on its side and will drop or kill a
                    // server that stays silent past that point — surface the busy error here
                    // instead of hanging until the host decides the server is gone.
                    if attempt <= BUSY_RETRY_ATTEMPTS && waited < wait_budget {
                        trace(&format!(
                            ".. {name}: editor busy, retry {attempt} in {delay}s (waited {:.1}s)",
                            waited.as_secs_f32()
                        ));
                        std::thread::sleep(Duration::from_secs(delay));
                        continue;
                    }
                    trace(&format!(
                        ".. {name}: giving up after {attempt} attempts / {:.1}s",
                        waited.as_secs_f32()
                    ));
                    return Err(anyhow!(
                        "Unity Editor still busy after {:.0}s ({attempt} attempts): {err:#}",
                        waited.as_secs_f32()
                    ));
                }
                trace(&format!(
                    ".. {name}: failed after {:.0}ms: {err:#}",
                    started.elapsed().as_secs_f64() * 1000.0
                ));
                return Err(err);
            }
        }
    }
}

fn execute_tool_once(
    name: &str,
    args: &Value,
    state: &Mutex<Option<Target>>,
    opts: &Options,
) -> Result<Value> {
    let target = ensure_connection(state, opts)?;

    // Arguments are forwarded verbatim: the server binds them against the command's real
    // parameter types, which the client has no reliable view of.
    let params = match args {
        Value::Object(_) => args.clone(),
        _ => json!({}),
    };

    let mut result = match execute(&target, name, &params, tool_budget(name, &params)) {
        Ok(v) => v,
        Err(err) => {
            // A successful recompile reloads the AppDomain that is answering this very request,
            // so the connection dropping mid-call is expected. The recompile_status poll below
            // is what reports the outcome.
            if name == "recompile" && is_transient(&err) {
                Value::Null
            } else {
                return Err(err);
            }
        }
    };

    if name == "recompile" {
        result = poll_status_until(
            &target,
            "recompile_status",
            RECOMPILE_POLL_TIMEOUT,
            is_recompile_done,
        )?;
    }

    Ok(shape_tool_result(name, &result, target.eval_token.as_deref()))
}

/// How long one tool call gets, on both sides of the wire.
///
/// The server aborts a main-thread command once the budget in the request is exceeded (60s when
/// the request carries none), so a tool whose own argument already states how long it intends to
/// run is honored end to end: `run_tests {timeout: 300}` would otherwise be cut off at 60s on
/// the server while the caller had asked for 300.
fn tool_budget(name: &str, args: &Value) -> ExecBudget {
    match name {
        // "Test execution timeout in seconds (default: 300)".
        "run_tests" => {
            ExecBudget::shared(seconds_arg(args, "timeout").unwrap_or(Duration::from_secs(300)))
        }
        // "Maximum seconds to wait (default 30, clamped to 0-600)".
        "wait_for" => {
            ExecBudget::shared(seconds_arg(args, "timeout_s").unwrap_or(Duration::from_secs(30)))
        }
        _ => ExecBudget::shared(HTTP_TIMEOUT),
    }
}

/// A whole-second duration argument, in either the number or the string form a tool schema may
/// advertise. Ignored when it is not a usable positive number, and clamped so a stray value
/// cannot turn into an unbounded wait.
fn seconds_arg(args: &Value, key: &str) -> Option<Duration> {
    let secs = match args.get(key)? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    if !secs.is_finite() || secs <= 0.0 {
        return None;
    }
    Some(Duration::from_secs_f64(secs.min(3600.0)))
}

fn shape_tool_result(name: &str, result: &Value, token: Option<&str>) -> Value {
    if matches!(name, "capture_game_view" | "capture_scene_view") {
        if let Some(shaped) = try_capture(result, token) {
            return shaped;
        }
    }
    let text = match result {
        Value::String(s) => s.clone(),
        other => crate::json::pretty(other),
    };
    let text = scrub(&text, token);

    let mut envelope = json!({
        "content": [{
            "type": "text",
            "text": text,
        }]
    });
    if matches!(name, "eval" | "eval_file") && is_failed_eval_envelope(result) {
        envelope["isError"] = Value::Bool(true);
    }
    if matches!(name, "recompile" | "recompile_status") && is_failed_recompile(result) {
        envelope["isError"] = Value::Bool(true);
    }
    envelope
}

/// Whether a recompile status payload reports a compile that did not succeed. `recompile_status`
/// answers with the status file's contents, where a failed build is `failed: true` (with the
/// errors listed) or the native `compilationFailed` flag.
fn is_failed_recompile(result: &Value) -> bool {
    let status = normalize_status(result);
    if status.get("failed").and_then(|v| v.as_bool()) == Some(true) {
        return true;
    }
    if status.get("compilationFailed").and_then(|v| v.as_bool()) == Some(true) {
        return true;
    }
    status.get("status").and_then(|v| v.as_str()) == Some("failed")
}

fn try_capture(result: &Value, token: Option<&str>) -> Option<Value> {
    let obj = result.as_object()?;
    if obj.get("encoding").and_then(|v| v.as_str()) != Some("png") {
        return None;
    }
    let base64 = obj.get("base64").and_then(|v| v.as_str())?;
    let saved_path = obj.get("savedPath").and_then(|v| v.as_str());
    let mut content = vec![json!({
        "type": "image",
        "data": base64,
        "mimeType": "image/png",
    })];
    if let Some(path) = saved_path {
        let mut meta = serde_json::Map::new();
        meta.insert("savedPath".to_owned(), Value::String(path.to_owned()));
        if let Some(w) = obj.get("inlineWidth") {
            meta.insert("inlineWidth".to_owned(), w.clone());
        }
        if let Some(h) = obj.get("inlineHeight") {
            meta.insert("inlineHeight".to_owned(), h.clone());
        }
        content.push(json!({
            "type": "text",
            "text": scrub(&crate::json::pretty(&Value::Object(meta)), token),
        }));
    }
    Some(json!({ "content": content }))
}

fn scrub(text: &str, token: Option<&str>) -> String {
    match token {
        Some(t) if !t.is_empty() && text.contains(t) => text.replace(t, "[redacted]"),
        _ => text.to_owned(),
    }
}

fn redact_token(text: &str, state: &Mutex<Option<Target>>) -> String {
    let guard = state.lock().unwrap();
    let token = guard.as_ref().and_then(|c| c.eval_token.clone());
    scrub(text, token.as_deref())
}

/// Resolve the Editor once and cache it; the connection is dropped whenever a call shows the
/// cached target is no longer answering.
fn ensure_connection(state: &Mutex<Option<Target>>, opts: &Options) -> Result<Target> {
    {
        let guard = state.lock().unwrap();
        if let Some(target) = guard.as_ref() {
            return Ok(target.clone());
        }
    }
    let target = find_target(opts.project_path.as_deref())?;
    ensure_reachable(&target)?;
    trace(&format!(
        ".. editor port={} pid={} token={} project={}",
        target.port,
        target
            .pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| "-".to_owned()),
        token_fingerprint(target.eval_token.as_deref()),
        target.project_path.display()
    ));
    let mut guard = state.lock().unwrap();
    *guard = Some(target.clone());
    Ok(target)
}

fn invalidate(state: &Mutex<Option<Target>>) {
    let mut guard = state.lock().unwrap();
    *guard = None;
}

fn fetch_tools(target: &Target) -> Result<Vec<Value>> {
    let catalog = fetch_commands(target, CATALOG_TIMEOUT)?;
    Ok(catalog
        .commands
        .into_iter()
        .filter_map(|cmd| {
            let name = cmd.name.clone().filter(|s| !s.is_empty())?;
            let description = cmd.description.clone().unwrap_or_else(|| name.clone());
            Some(json!({
                "name": name,
                "description": description,
                "inputSchema": command_input_schema(&cmd),
            }))
        })
        .collect())
}

fn is_recompile_done(status: &Value) -> bool {
    let status = normalize_status(status);
    if status.get("isCompiling").and_then(|v| v.as_bool()) == Some(false) {
        return true;
    }
    matches!(
        status.get("status").and_then(|v| v.as_str()),
        Some("completed") | Some("up_to_date") | Some("failed")
    )
}

fn poll_status_until<F>(
    target: &Target,
    command: &str,
    timeout: Duration,
    is_done: F,
) -> Result<Value>
where
    F: Fn(&Value) -> bool,
{
    let deadline = Instant::now() + timeout;
    let params = json!({});
    let mut last_ok: Option<Value> = None;
    let mut last_err: Option<anyhow::Error> = None;
    while Instant::now() < deadline {
        std::thread::sleep(POLL_INTERVAL);
        match execute(
            target,
            command,
            &params,
            ExecBudget::client_only(POLL_HTTP_TIMEOUT),
        ) {
            Ok(status) => {
                if is_done(&status) {
                    return Ok(status);
                }
                last_ok = Some(status);
                last_err = None;
            }
            // The Editor is expected to disappear for part of this window (domain reload,
            // restart), so transport failures are retried until the deadline.
            Err(err) => {
                if !is_transient(&err) && retry_delay(&err).is_none() {
                    return Err(err);
                }
                last_err = Some(err);
            }
        }
    }
    if let Some(v) = last_ok {
        return Err(anyhow!(
            "Timed out waiting for {command}; last response: {}",
            v
        ));
    }
    if let Some(err) = last_err {
        return Err(anyhow!("Timed out waiting for {command}; last error: {err:#}"));
    }
    Err(anyhow!("Timed out waiting for {command}"))
}

#[cfg(test)]
#[path = "../tests/mcp.rs"]
mod tests;
