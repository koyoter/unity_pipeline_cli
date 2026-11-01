use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::pipeline::{
    self, command_input_schema, ensure_reachable, execute, fetch_commands,
    is_failed_eval_envelope, is_transient, retry_delay, tool_name, ExecBudget, Target,
};

/// Newest protocol revision this server speaks; also the fallback when a client asks for one we
/// don't know.
const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];
const SERVER_NAME: &str = "unity-mcp";
const SERVER_VERSION: &str = "1.0.0-rust";
/// Default budget for one tool call, applied to both sides of the wire (`tool_budget`).
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
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
    /// Official `--runtime`: target a running Player by process/executable name.
    pub runtime: Option<String>,
    /// Official `--runtime-path`: target a running Player by the directory holding its
    /// runtime port file.
    pub runtime_path: Option<PathBuf>,
    /// Target an Android player build over adb: package name (e.g. com.acme.mygame).
    pub adb: Option<String>,
    /// Serial of the adb device when several are attached.
    pub adb_serial: Option<String>,
}

/// Runtime-target context, resolved once at startup so the offline paths (tool cache,
/// Player.log) do not depend on the player still being alive.
#[derive(Clone)]
struct RuntimeCtx {
    is_runtime: bool,
    /// Raw target flags, kept so `player_status` can re-resolve on every call — a restarted
    /// player rewrites the descriptor with a fresh port and per-process token.
    runtime: Option<String>,
    runtime_path: Option<PathBuf>,
    /// Android target (`--adb`): package + serial. Every adb operation re-selects the device
    /// and re-reads the descriptor, so reconnects and app restarts self-heal.
    adb: Option<crate::adb::AdbTarget>,
    /// The player's build dir (desktop) or local cache dir (Android): cache file location
    /// and Player.log discovery root. `None` when it could not be determined (e.g.
    /// `--runtime` by name and the player is dead) — the local tools then degrade to
    /// explicit-path/empty-catalog behavior.
    dir: Option<PathBuf>,
    /// Auto-located Player.log, best effort (desktop only; Android uses logcat).
    log_path: Option<PathBuf>,
}

impl RuntimeCtx {
    fn discover(opts: &Options) -> Self {
        if let Some(package) = opts.adb.as_deref() {
            // Android: the descriptor lives on the device, so there is no local build dir —
            // the tool cache goes to a per-package temp dir instead, and the log is logcat.
            let dir = std::env::temp_dir()
                .join("unity-pipeline-mcp")
                .join(package);
            let _ = std::fs::create_dir_all(&dir);
            return Self {
                is_runtime: true,
                runtime: None,
                runtime_path: None,
                adb: Some(crate::adb::AdbTarget {
                    serial: opts.adb_serial.clone(),
                    package: package.to_owned(),
                }),
                dir: Some(dir),
                log_path: None,
            };
        }
        if opts.runtime.is_none() && opts.runtime_path.is_none() {
            return Self {
                is_runtime: false,
                runtime: None,
                runtime_path: None,
                adb: None,
                dir: None,
                log_path: None,
            };
        }
        // Best effort by design: a dead player still leaves its descriptor on disk, so the
        // dir is usually known even when the server will never answer again.
        let target = crate::runtime::find_runtime_target(
            opts.runtime.as_deref(),
            opts.runtime_path.as_deref(),
        )
        .ok();
        let dir = target
            .as_ref()
            .map(|t| t.project_path.clone())
            .or_else(|| {
                opts.runtime_path.as_deref().and_then(|p| {
                    if p.is_dir() {
                        Some(p.to_path_buf())
                    } else {
                        p.parent().map(Path::to_path_buf)
                    }
                })
            });
        let log_path = dir.as_deref().and_then(crate::runtime::find_player_log);
        Self {
            is_runtime: true,
            runtime: opts.runtime.clone(),
            runtime_path: opts.runtime_path.clone(),
            adb: None,
            dir,
            log_path,
        }
    }

    /// The name agents see in serverInfo: plain for the editor, suffixed with the build dir
    /// name for a player, so two configured MCP connections never look identical.
    fn server_name(&self) -> String {
        if !self.is_runtime {
            return SERVER_NAME.to_owned();
        }
        let label = self
            .dir
            .as_ref()
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "runtime".to_owned());
        format!("{SERVER_NAME}@{label}")
    }
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
    // Arc so the runtime-mode catalog watcher thread can share them; `&Arc<Mutex<_>>` still
    // deref-coerces to `&Mutex<_>` at every existing call site.
    let state = Arc::new(Mutex::new(None::<Target>));
    // Tool names from the last successful catalog fetch, so a change can be announced.
    let known_tools = Arc::new(Mutex::new(None::<HashSet<String>>));
    // Last full catalog, served instead of an empty list when a refresh fails: emptying the
    // host's tool set is what turns a recoverable outage into "MCP tool is not found".
    let last_tools = Arc::new(Mutex::new(None::<Vec<Value>>));
    let initialized = AtomicBool::new(false);
    let stop = Arc::new(AtomicBool::new(false));
    let stdout = std::io::stdout();
    let ctx = RuntimeCtx::discover(&opts);
    let server_name = ctx.server_name();

    // Announce readiness on stderr — stdout carries only protocol messages.
    let trace_path = trace_init();
    trace(&format!(
        "== start pid={} v{SERVER_VERSION} busy-wait-cap={}s project_path={:?} runtime={} runtime_dir={} trace={}",
        std::process::id(),
        max_wait_secs(),
        opts.project_path,
        ctx.is_runtime,
        ctx.dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "-".to_owned()),
        trace_path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "off".to_owned())
    ));
    eprintln!("unity mcp: server started on stdio, waiting for client requests");

    // Runtime mode: the player usually starts AFTER this MCP connection (degraded startup is
    // the normal flow), so a watcher thread pushes notifications/tools/list_changed the
    // moment the player's catalog appears or changes — hosts re-list on that notification and
    // never need the agent to re-ask.
    if ctx.is_runtime {
        spawn_catalog_watcher(&ctx, &state, &known_tools, &last_tools, &stop);
    }

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
                let response = handle_initialize(id.clone(), &msg, &server_name);
                write_message(&stdout, &response)?;
            }
            "ping" => write_message(&stdout, &success_response(id.clone(), json!({})))?,
            "tools/list" => {
                let (response, changed) = handle_tools_list(
                    id.clone(),
                    &state,
                    &opts,
                    &ctx,
                    &known_tools,
                    &last_tools,
                );
                write_message(&stdout, &response)?;
                if changed && initialized.load(Ordering::Relaxed) {
                    write_message(
                        &stdout,
                        &json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }),
                    )?;
                }
            }
            "tools/call" => {
                let response =
                    handle_tools_call(id.clone(), &msg, &state, &opts, &ctx, &last_tools);
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
    stop.store(true, Ordering::Relaxed);
    Ok(())
}

/// How often the runtime-mode watcher probes the player for catalog changes.
const CATALOG_WATCH_INTERVAL: Duration = Duration::from_secs(3);

/// Poll the runtime target for its command catalog and announce changes. Runs on its own
/// thread, runtime mode only. Each write_message is one locked stdout transaction, so the
/// notification cannot interleave with the main loop's responses.
fn spawn_catalog_watcher(
    ctx: &RuntimeCtx,
    state: &Arc<Mutex<Option<Target>>>,
    known_tools: &Arc<Mutex<Option<HashSet<String>>>>,
    last_tools: &Arc<Mutex<Option<Vec<Value>>>>,
    stop: &Arc<AtomicBool>,
) {
    let ctx = ctx.clone();
    let state = state.clone();
    let known_tools = known_tools.clone();
    let last_tools = last_tools.clone();
    let stop = stop.clone();
    std::thread::spawn(move || {
        // Stdout is a global handle — a fresh clone per thread is fine.
        let stdout = std::io::stdout();
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(CATALOG_WATCH_INTERVAL);
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let Ok(target) = resolve_target_for_ctx(&ctx) else {
                continue; // no descriptor / device gone: local tools still serve
            };
            if !crate::http::probe_pipeline(target.port, target.eval_token.as_deref()) {
                // Player went away (or is starting): drop the cached connection so the next
                // forwarded call re-resolves instead of failing on a stale socket target.
                invalidate(&state);
                continue;
            }
            let Ok(tools) = fetch_tools(&target) else {
                trace(".. watcher: player reachable but catalog fetch failed");
                continue;
            };
            let names: HashSet<String> = tools
                .iter()
                .filter_map(|t| t.get("name").and_then(|v| v.as_str()).map(str::to_owned))
                .collect();
            let changed = {
                let mut guard = known_tools.lock().unwrap();
                // Same rule as refresh_tools: empty catalogs announce nothing, and the known
                // set updates even when the answer is "no change".
                let changed = guard.as_ref() != Some(&names) && !names.is_empty();
                *guard = Some(names);
                changed
            };
            if !changed {
                // Keep the live target warm for forwarded calls either way.
                *state.lock().unwrap() = Some(target);
                continue;
            }
            trace(".. watcher: player catalog changed; notifying client");
            *last_tools.lock().unwrap() = Some(with_local_tools(tools, &ctx));
            *state.lock().unwrap() = Some(target);
            let notification =
                json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" });
            if let Err(err) = write_message(&stdout, &notification) {
                trace(&format!("!! watcher notification failed: {err}"));
                break;
            }
        }
    });
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
fn handle_initialize(id: Value, msg: &Value, server_name: &str) -> Value {
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
                "name": server_name,
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
    ctx: &RuntimeCtx,
    known_tools: &Mutex<Option<HashSet<String>>>,
    last_tools: &Mutex<Option<Vec<Value>>>,
) -> (Value, bool) {
    match refresh_tools(state, opts, ctx, known_tools, last_tools) {
        Ok((tools, changed)) => (success_response(id, json!({ "tools": tools })), changed),
        Err(err) => {
            // An unreachable Editor/Player is a normal state to start in, not a protocol
            // error: hand back a catalog (so the client still completes initialize + list)
            // and let a later call re-discover the server.
            invalidate(state);
            trace(&format!("stderr: {err:#}"));
            eprintln!("unity mcp: {err:#}");
            // Once a catalog has been served, never answer with an empty one: hosts treat that
            // as every tool being unregistered ("MCP tool is not found") and do not re-list on
            // their own. The stale list keeps calls working until the next discovery succeeds.
            // A runtime target additionally falls back to the on-disk catalog cache, so even a
            // first list after the player crashed still shows the tool set (Q3 degraded mode).
            let stale = last_tools.lock().unwrap().clone();
            let stale = match stale {
                Some(tools) if !tools.is_empty() => Some(tools),
                _ => {
                    if ctx.is_runtime {
                        let cached = ctx.dir.as_deref().and_then(read_tools_cache);
                        if let Some(tools) = cached {
                            trace(&format!(
                                ".. serving on-disk catalog cache ({} tools) until the player is reachable",
                                tools.len()
                            ));
                            Some(tools)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
            };
            let tools = stale.unwrap_or_default();
            let served = with_local_tools(tools, ctx);
            if served.is_empty() {
                trace(".. no catalog served yet; answering with an empty list");
            }
            (success_response(id, json!({ "tools": served })), false)
        }
    }
}

fn handle_tools_call(
    id: Value,
    msg: &Value,
    state: &Mutex<Option<Target>>,
    opts: &Options,
    ctx: &RuntimeCtx,
    last_tools: &Mutex<Option<Vec<Value>>>,
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

    // Local tools never touch the server — they exist precisely for when it cannot be reached.
    if ctx.is_runtime {
        match name.as_str() {
            "player_status" => return success_response(id, local_player_status(ctx)),
            "player_log_read" => return success_response(id, local_player_log_read(&args, ctx)),
            _ => {}
        }
    }

    match execute_tool_with_retry(&name, &args, state, opts, last_tools) {
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
    ctx: &RuntimeCtx,
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

    // A runtime target persists its catalog next to the descriptor: the next server start
    // can serve it even when the player has since exited or crashed.
    if ctx.is_runtime {
        if let Some(dir) = ctx.dir.as_deref() {
            write_tools_cache(dir, &tools);
        }
    }

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
    let served = with_local_tools(tools.clone(), ctx);
    *last_tools.lock().unwrap() = Some(served.clone());
    Ok((served, changed))
}

/// Run one tool, transparently recovering from the two states that are not real failures:
/// a connection this process has gone stale on (the Editor restarted or reloaded its domain),
/// and a server that says it is busy and asks to be retried shortly.
///
/// `schemas` is the last successfully served tool list; it carries each tool's `inputSchema`,
/// whose `timeout` default (when the caller passed none) states how long the command intends
/// to run.
fn execute_tool_with_retry(
    name: &str,
    args: &Value,
    state: &Mutex<Option<Target>>,
    opts: &Options,
    schemas: &Mutex<Option<Vec<Value>>>,
) -> Result<Value> {
    let wait_budget = Duration::from_secs(max_wait_secs());
    let started = Instant::now();
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match execute_tool_once(name, args, state, opts, schemas) {
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

/// Run one tool: forward the name and arguments verbatim to `/api/exec` and shape the reply
/// into MCP content. No command is special-cased — a command whose result implies follow-up
/// (e.g. a recompile returning "poll recompile_status") says so in its own description and
/// result text, which is what the caller acts on.
fn execute_tool_once(
    name: &str,
    args: &Value,
    state: &Mutex<Option<Target>>,
    opts: &Options,
    schemas: &Mutex<Option<Vec<Value>>>,
) -> Result<Value> {
    let target = ensure_connection(state, opts)?;

    // Arguments are forwarded verbatim: the server binds them against the command's real
    // parameter types, which the client has no reliable view of.
    let params = match args {
        Value::Object(_) => args.clone(),
        _ => json!({}),
    };

    let schema = {
        let guard = schemas.lock().unwrap();
        guard
            .as_ref()
            .and_then(|tools| {
                tools
                    .iter()
                    .find(|t| t.get("name").and_then(Value::as_str) == Some(name))
                    .and_then(|t| t.get("inputSchema"))
                    .cloned()
            })
    };

    let result = execute(&target, name, &params, tool_budget(&params, schema.as_ref()))?;
    Ok(shape_tool_result(&result, target.eval_token.as_deref()))
}

/// How long one tool call gets, on both sides of the wire.
///
/// Any tool that states its own duration in a `timeout` (seconds) argument gets exactly that,
/// honored end to end: the server aborts a main-thread command once the budget in the request
/// is exceeded, so forwarding the caller's `timeout` is what makes an explicitly raised one
/// mean anything (`test run` with `timeout: 300` would otherwise be cut off at 60s).
/// `timeout_s` is the pre-0.9 spelling of the same argument (`wait_for`). When the caller
/// passed neither, the command's own advertised default — the catalog schema's
/// `properties.timeout.default` — applies before the 60s fallback. The server unified the
/// per-command timeout meaning in 0.9 (AUTHAPI-58), so reading the argument shape covers
/// every command without naming any of them.
fn tool_budget(args: &Value, schema: Option<&Value>) -> ExecBudget {
    let explicit = seconds_arg(args, "timeout").or_else(|| seconds_arg(args, "timeout_s"));
    let declared = schema.and_then(|s| {
        let props = s.get("properties")?;
        seconds_arg(props.get("timeout")?, "default")
            .or_else(|| seconds_arg(props.get("timeout_s")?, "default"))
    });
    ExecBudget::shared(explicit.or(declared).unwrap_or(HTTP_TIMEOUT))
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

/// Shape one command result into MCP content by its form alone — never by command name, so
/// every version's and every future command that returns the same shape is handled the same
/// way: a PNG payload becomes an image block (hosts do not render base64 inside JSON text),
/// an envelope that says `success: false` is marked `isError`, everything else is passed
/// through as text.
fn shape_tool_result(result: &Value, token: Option<&str>) -> Value {
    if let Some(shaped) = try_capture(result, token) {
        return shaped;
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
    if is_failed_eval_envelope(result) {
        envelope["isError"] = Value::Bool(true);
    }
    envelope
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

/// Resolve the Editor/Player once and cache it; the connection is dropped whenever a call
/// shows the cached target is no longer answering.
fn ensure_connection(state: &Mutex<Option<Target>>, opts: &Options) -> Result<Target> {
    {
        let guard = state.lock().unwrap();
        if let Some(target) = guard.as_ref() {
            return Ok(target.clone());
        }
    }
    let target = crate::runtime::resolve_target(
        opts.runtime.as_deref(),
        opts.runtime_path.as_deref(),
        opts.adb.as_deref(),
        opts.adb_serial.as_deref(),
        opts.project_path.as_deref(),
    )?;
    ensure_reachable(&target)?;
    trace(&format!(
        ".. target port={} pid={} token={} root={}",
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
            // The dotted full name (`asset.create`): unique across groups and valid for MCP
            // hosts, where the server's bare `name` (`create`) collides. See
            // [`pipeline::tool_name`].
            let name = tool_name(&cmd)?;
            let description = cmd.description.clone().unwrap_or_else(|| name.clone());
            Some(json!({
                "name": name,
                "description": description,
                "inputSchema": command_input_schema(&cmd),
            }))
        })
        .collect())
}

/// ---- Runtime-target local tools ----
///
/// The one tool that is not forwarded (`player_log_read`) exists for exactly the state the
/// forwarded tools cannot serve: the player has exited or crashed. It reads Player.log from
/// disk, no server involved. Only advertised on a runtime target; editor mode is unchanged.

const PLAYER_LOG_TOOL: &str = "player_log_read";
const PLAYER_STATUS_TOOL: &str = "player_status";

/// One resolution point for the runtime target behind the local tools and the watcher:
/// re-reads the descriptor on every call so app restarts (fresh per-process token) and
/// reconnected devices (fresh adb forward) are picked up.
fn resolve_target_for_ctx(ctx: &RuntimeCtx) -> Result<Target> {
    if let Some(at) = ctx.adb.as_ref() {
        return crate::adb::resolve_adb_target(at.serial.as_deref(), &at.package);
    }
    crate::runtime::find_runtime_target(ctx.runtime.as_deref(), ctx.runtime_path.as_deref())
}

fn with_local_tools(mut tools: Vec<Value>, ctx: &RuntimeCtx) -> Vec<Value> {
    if ctx.is_runtime {
        tools.push(json!({
            "name": PLAYER_STATUS_TOOL,
            "description": "Check whether this player build is running and its runtime server answers. When it runs, the runtime commands are available directly as MCP tools — call tools/list to refresh and invoke them here, no separate CLI call needed. When it does not, the result says how to start the exe.",
            // MCP spec: inputSchema is required on every tool; hosts reject the list without it.
            "inputSchema": { "type": "object", "properties": {} }
        }));
        tools.push(json!({
            "name": PLAYER_LOG_TOOL,
            "description": "Read the tail of this player's log — works with the player offline, e.g. right after a crash. Desktop: Player.log from disk; Android: the device's Unity logcat via adb (`path` is desktop-only). Omit `path` to read the auto-located log.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Explicit log file path; overrides auto-discovery (desktop only; Windows: %USERPROFILE%\\AppData\\LocalLow\\<Company>\\<Product>\\Player.log)."
                    },
                    "lines": {
                        "type": "integer",
                        "description": "How many trailing lines to return (default 400, max 5000)."
                    }
                }
            }
        }));
    }
    tools
}

fn local_player_status(ctx: &RuntimeCtx) -> Value {
    match player_status_text(ctx) {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
        Err(err) => json!({
            "content": [{ "type": "text", "text": format!("Error: {err:#}") }],
            "isError": true,
        }),
    }
}

fn player_status_text(ctx: &RuntimeCtx) -> Result<String> {
    // Re-resolve on every call: a restarted player rewrites its descriptor with a fresh port
    // and per-process token, and this tool is exactly how the agent notices that.
    let log_note = if ctx.adb.is_some() {
        "\nLog: adb logcat -s Unity (player_log_read tails it).".to_owned()
    } else {
        match ctx.log_path.as_ref() {
            Some(p) => format!("\nPlayer.log: {}", p.display()),
            None => "\nPlayer.log: not located automatically — player_log_read accepts an explicit path.".to_owned(),
        }
    };
    let target = match resolve_target_for_ctx(ctx) {
        Ok(t) => t,
        Err(err) => {
            if ctx.adb.is_some() {
                return Ok(format!(
                    "Android app is NOT reachable.\n\n{err:#}{log_note}\n\nLaunch the app on the device (a development build with enableInDevelopmentBuilds), then call this tool again; once it reports the app as running, the runtime commands are available directly as MCP tools (call tools/list to refresh)."
                ));
            }
            return Ok(format!(
                "Player is NOT running — no runtime descriptor found.\n\n{err:#}{log_note}\n\nAfter starting the exe, call this tool again; once it reports the player as running, the runtime commands are available directly as MCP tools (call tools/list to refresh)."
            ));
        }
    };
    if !crate::http::probe_pipeline(target.port, target.eval_token.as_deref()) {
        if ctx.adb.is_some() {
            return Ok(format!(
                "Android app is not answering at 127.0.0.1:{} (adb forward, device path {}). The app may still be starting, or this build has the runtime server disabled.\n\nLaunch the app on the device — a development build with enableInDevelopmentBuilds; when it is up, the runtime commands are available directly as MCP tools (call tools/list to refresh).{log_note}",
                target.port,
                target.project_path.display()
            ));
        }
        return Ok(format!(
            "Player is NOT answering at 127.0.0.1:{} (descriptor: pid {:?}, build dir {}). The exe may still be starting, or this build has the runtime server disabled.\n\nStart the exe in {} — a development build with enableInDevelopmentBuilds; when it is up, the runtime commands are available directly as MCP tools (call tools/list to refresh).{log_note}",
            target.port,
            target.pid,
            target.project_path.display(),
            target.project_path.display()
        ));
    }
    if ctx.adb.is_some() {
        return Ok(format!(
            "Android app is running: pid {:?} via adb forward 127.0.0.1:{} (device path {}).\nIts runtime commands are exposed as MCP tools — call tools/list to refresh the list if they are not shown yet, then invoke any of them directly through MCP (no separate CLI call needed).{log_note}",
            target.pid,
            target.port,
            target.project_path.display()
        ));
    }
    Ok(format!(
        "Player is running: pid {:?} at 127.0.0.1:{}, build dir {}.\nIts runtime commands are exposed as MCP tools — call tools/list to refresh the list if they are not shown yet, then invoke any of them directly through MCP (no separate CLI call needed).{log_note}",
        target.pid,
        target.port,
        target.project_path.display()
    ))
}

fn local_player_log_read(args: &Value, ctx: &RuntimeCtx) -> Value {
    match player_log_text(args, ctx) {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
        Err(err) => json!({
            "content": [{ "type": "text", "text": format!("Error: {err:#}") }],
            "isError": true,
        }),
    }
}

fn player_log_text(args: &Value, ctx: &RuntimeCtx) -> Result<String> {
    // Android: the log is the device's Unity logcat buffer, not a file.
    if let Some(at) = ctx.adb.as_ref() {
        let lines = args
            .get("lines")
            .and_then(Value::as_u64)
            .unwrap_or(400)
            .clamp(1, 5000) as usize;
        return crate::adb::logcat_tail(at, lines);
    }
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| ctx.log_path.clone())
        .ok_or_else(|| {
            anyhow!(
                "Player.log not found — pass an explicit \"path\" (Windows: %USERPROFILE%\\AppData\\LocalLow\\<Company>\\<Product>\\Player.log)"
            )
        })?;
    let lines = args
        .get("lines")
        .and_then(Value::as_u64)
        .unwrap_or(400)
        .clamp(1, 5000) as usize;
    let text =
        fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    let total = text.lines().count();
    let tail: Vec<&str> = text.lines().skip(total.saturating_sub(lines)).collect();
    Ok(format!(
        "{} — showing {}/{} lines (file is {} bytes)\n{}",
        path.display(),
        tail.len(),
        total,
        text.len(),
        tail.join("\n")
    ))
}

/// On-disk catalog cache, written next to the runtime descriptor after every successful
/// fetch so a later start can serve a tool list for a player that is dead or crashing.
const TOOLS_CACHE_FILE: &str = ".unity-pipeline-mcp-tools.json";

fn write_tools_cache(dir: &Path, tools: &[Value]) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let payload = json!({ "savedAt": iso8601_utc(now_ms), "tools": tools });
    if let Err(err) = fs::write(dir.join(TOOLS_CACHE_FILE), crate::json::pretty(&payload)) {
        trace(&format!(".. tools cache write failed: {err}"));
    }
}

fn read_tools_cache(dir: &Path) -> Option<Vec<Value>> {
    let text = fs::read_to_string(dir.join(TOOLS_CACHE_FILE)).ok()?;
    serde_json::from_str::<Value>(&text)
        .ok()?
        .get("tools")?
        .as_array()
        .cloned()
}

#[cfg(test)]
#[path = "../tests/mcp.rs"]
mod tests;
