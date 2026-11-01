//! Android player targets over adb — the `--adb <包名>` / `--adb-serial <serial>` flags.
//!
//! The Android player's runtime server binds loopback on the device only, and its descriptor
//! (`.unity-pipeline-runtime-port`, carrying port + per-process token) sits in the app's
//! `persistentDataPath` (`/data/data/<pkg>/files`, internal storage). The host therefore
//! reaches it in two adb steps, both fully managed here so nothing is manual:
//!   1. `adb shell run-as <pkg> cat files/.unity-pipeline-runtime-port` — dev builds are
//!      debuggable, which run-as requires (the pipeline only runs in dev builds anyway).
//!   2. `adb forward tcp:<local> tcp:<device>` — the CLI then talks to 127.0.0.1:<local>.

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{anyhow, Result};

use crate::pipeline::Target;
use crate::runtime::RuntimeDescriptor;

/// Where an Android target lives: package name plus (optionally) a device serial.
#[derive(Debug, Clone)]
pub struct AdbTarget {
    pub serial: Option<String>,
    pub package: String,
}

fn adb(serial: Option<&str>, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("adb");
    if let Some(s) = serial {
        cmd.arg("-s").arg(s);
    }
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = cmd.output().map_err(|e| {
        anyhow!(
            "cannot run adb ({e}) — install Android platform-tools and make sure adb is on PATH"
        )
    })?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(anyhow!("adb {} failed: {}", args.join(" "), stderr));
    }
    Ok(stdout)
}

/// Serials of attached devices in the ready ("device") state — offline/unauthorized skipped.
fn parse_adb_devices(out: &str) -> Vec<String> {
    out.lines()
        .skip(1) // "List of devices attached" header
        .filter_map(|line| {
            let mut tokens = line.split_whitespace();
            let serial = tokens.next()?.to_owned();
            match tokens.next() {
                Some("device") => Some(serial),
                _ => None,
            }
        })
        .collect()
}

/// Pick the device to talk to. A given serial must be attached; with none given, exactly one
/// attached device is required (its serial is returned concretely either way).
fn select_device(serial: Option<&str>) -> Result<String> {
    let attached = parse_adb_devices(&adb(None, &["devices"])?);
    match serial {
        Some(s) => {
            if attached.iter().any(|a| a == s) {
                Ok(s.to_owned())
            } else {
                Err(anyhow!(
                    "adb device '{s}' is not attached (ready devices: {}). Replug USB / check `adb devices`.",
                    if attached.is_empty() { "none".to_owned() } else { attached.join(", ") }
                ))
            }
        }
        None => match attached.len() {
            0 => Err(anyhow!(
                "no adb device attached (ready state). Replug USB / check `adb devices`."
            )),
            1 => Ok(attached[0].clone()),
            n => Err(anyhow!(
                "{n} adb devices attached: {}.\n\nUse --adb-serial <serial> to pick one.",
                attached.join(", ")
            )),
        },
    }
}

/// Keep only a non-empty read: empty output means "the file isn't there" for our purposes.
fn non_empty(r: Result<String>) -> Option<String> {
    match r {
        Ok(t) if !t.trim().is_empty() => Some(t),
        _ => None,
    }
}

/// Read the runtime descriptor from the device. `persistentDataPath` is device-dependent —
/// internal (`/data/data/<pkg>/files`, what run-as's cwd is) or external
/// (`/storage/emulated/0/Android/data/<pkg>/files`) — so try both, then ask logcat where the
/// publisher actually wrote it (it logs "Descriptor written to <path>" at startup).
fn read_descriptor(at: &AdbTarget) -> Result<RuntimeDescriptor> {
    let serial = select_device(at.serial.as_deref())?;
    const FILE: &str = ".unity-pipeline-runtime-port";
    if let Some(text) = non_empty(adb(
        Some(&serial),
        &["shell", "run-as", &at.package, "cat", &format!("files/{FILE}")],
    )) {
        return parse_descriptor(&text);
    }
    if let Some(text) = non_empty(adb(
        Some(&serial),
        &[
            "shell",
            "cat",
            &format!("/sdcard/Android/data/{}/files/{FILE}", at.package),
        ],
    )) {
        return parse_descriptor(&text);
    }
    // Last resort: the publisher logs the exact path it wrote (PipelineDescriptorPublisher /
    // LiveDescriptorFile). Extracting it keeps this working wherever persistentDataPath lands.
    let log = adb(Some(&serial), &["logcat", "-d", "-s", "Unity"]).unwrap_or_default();
    let marker = "Descriptor written to ";
    if let Some(line) = log.lines().rev().find(|l| l.contains(marker)) {
        if let Some(path) = line.split(marker).nth(1).map(str::trim) {
            if !path.is_empty() {
                if let Some(text) = non_empty(adb(Some(&serial), &["shell", "cat", path])) {
                    return parse_descriptor(&text);
                }
            }
        }
    }
    Err(anyhow!(
        "no runtime descriptor found on the device (tried internal files/, external /sdcard/Android/data/{0}/files/, and the publisher's logcat hint).\n\nLikely causes, in order:\n  1. The build is not a Development Build (build with the Development option)\n  2. Project Settings → Pipeline → Runtime → enableInDevelopmentBuilds is off\n  3. The app process crashed before scene load\n\nCheck `adb logcat -d -s Unity` for 'Pipeline:' lines — a healthy app logs \"Runtime server started successfully on port N\" at launch.",
        at.package
    ))
}

fn parse_descriptor(text: &str) -> Result<RuntimeDescriptor> {
    let parsed: RuntimeDescriptor = serde_json::from_str(text.trim())
        .map_err(|e| anyhow!("invalid runtime descriptor from device: {e}"))?;
    if parsed.port < 1 {
        return Err(anyhow!("invalid runtime descriptor from device: port < 1"));
    }
    Ok(parsed)
}

/// True when nothing on this host listens on the local port yet (best-effort probe).
fn local_port_free(port: u16) -> bool {
    // ponytail: bind-then-drop races with other openers; retrying the scan is the upgrade path.
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Reuse an existing forward to the device port when one is already set up; otherwise create
/// one, starting at the device port and scanning upward for a free local port.
fn ensure_forward(serial: &str, device_port: u16) -> Result<u16> {
    let want = format!("tcp:{device_port}");
    // `forward --list` rows: <serial> tcp:<local> tcp:<device>
    let list = adb(Some(serial), &["forward", "--list"])?;
    for line in list.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() >= 3 && tokens[2] == want {
            if let Some(local) = tokens[1].strip_prefix("tcp:") {
                if let Ok(p) = local.parse::<u16>() {
                    return Ok(p);
                }
            }
        }
    }
    for offset in 0..100u32 {
        let candidate = device_port + offset as u16;
        if !local_port_free(candidate) {
            continue;
        }
        adb(
            Some(serial),
            &["forward", &format!("tcp:{candidate}"), &want],
        )?;
        return Ok(candidate);
    }
    Err(anyhow!(
        "no free local port in {device_port}..{} for the adb forward",
        device_port + 99
    ))
}

/// Resolve an Android target end to end: descriptor via run-as, then a loopback forward.
/// Safe to call repeatedly — the watcher and player_status re-run it to pick up app
/// restarts (fresh per-process token) and to re-establish a dropped forward.
pub fn resolve_adb_target(serial: Option<&str>, package: &str) -> Result<Target> {
    let at = AdbTarget {
        serial: serial.map(str::to_owned),
        package: package.to_owned(),
    };
    let desc = read_descriptor(&at)?;
    let concrete_serial = select_device(at.serial.as_deref())?;
    let local = ensure_forward(&concrete_serial, desc.port)?;
    Ok(Target {
        port: local,
        project_path: desc
            .working_directory
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("device:/data/data/{}", package))),
        eval_token: desc.eval_token,
        pid: desc.pid,
        info: None,
        is_runtime: true,
        is_adb: true,
    })
}

/// Tail of the app's Unity logcat buffer (`adb logcat -d -s Unity`), last `lines` lines.
pub fn logcat_tail(at: &AdbTarget, lines: usize) -> Result<String> {
    let serial = select_device(at.serial.as_deref())?;
    let out = adb(Some(&serial), &["logcat", "-d", "-s", "Unity"])?;
    let total = out.lines().count();
    let tail: Vec<&str> = out.lines().skip(total.saturating_sub(lines)).collect();
    Ok(format!(
        "adb logcat -d -s Unity — showing {}/{} lines\n{}",
        tail.len(),
        total,
        tail.join("\n")
    ))
}

#[cfg(test)]
#[path = "../tests/adb.rs"]
mod tests;
