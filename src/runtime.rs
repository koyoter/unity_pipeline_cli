//! Runtime (player build) target discovery — the official `--runtime` / `--runtime-path`
//! flags (package README: "Connect to a running Player (Runtime)"). A player build with the
//! Pipeline runtime server writes its own descriptor next to its executable
//! (`.unity-pipeline-runtime-port`, see `RuntimeInstanceDescriptor.cs`); it carries the port
//! and the per-process bearer token, so the descriptor is the only discovery path that works
//! for a player that is not (or no longer) reachable over HTTP.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde::Deserialize;

use crate::pipeline::Target;
use crate::process::find_processes_by_name;

const RUNTIME_DESCRIPTOR_FILE: &str = ".unity-pipeline-runtime-port";

/// Player-side descriptor. `evalToken` is generated per process at server start, so it can
/// only ever come from this file — there is no editor-side authority to ask.
#[derive(Debug, Deserialize)]
pub struct RuntimeDescriptor {
    pub port: u16,
    #[serde(default, rename = "evalToken")]
    pub eval_token: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default, rename = "workingDirectory")]
    pub working_directory: Option<String>,
}

/// Read the runtime descriptor from a directory (the build output dir) or a direct file path.
pub fn read_runtime_descriptor(path: &Path) -> Result<RuntimeDescriptor> {
    let file = if path.is_dir() {
        path.join(RUNTIME_DESCRIPTOR_FILE)
    } else {
        path.to_path_buf()
    };
    let text = fs::read_to_string(&file).map_err(|_| {
        anyhow!(
            "no runtime descriptor at {}\n\nMake sure the Player was built with the Pipeline runtime server enabled (development build + enableInDevelopmentBuilds) and is running.",
            file.display()
        )
    })?;
    let parsed: RuntimeDescriptor = serde_json::from_str(&text)
        .map_err(|e| anyhow!("invalid runtime descriptor {}: {e}", file.display()))?;
    if parsed.port < 1 {
        return Err(anyhow!(
            "invalid runtime descriptor {}: port < 1",
            file.display()
        ));
    }
    Ok(parsed)
}

/// Resolve a runtime target. `runtime_path` is the deterministic path (dir or file);
/// `runtime` names the player process/executable and locates the descriptor next to its
/// executable.
pub fn find_runtime_target(runtime: Option<&str>, runtime_path: Option<&Path>) -> Result<Target> {
    if let Some(path) = runtime_path {
        let dir = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };
        let desc = read_runtime_descriptor(path)?;
        let work = desc
            .working_directory
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or(dir);
        return Ok(Target {
            port: desc.port,
            project_path: work,
            eval_token: desc.eval_token,
            pid: desc.pid,
            info: None,
            is_runtime: true,
            is_adb: false,
        });
    }

    let name = runtime.ok_or_else(|| anyhow!("no runtime target given"))?;
    let processes = find_processes_by_name(name)
        .map_err(|e| anyhow!("cannot enumerate processes to locate player '{name}': {e:#}"))?;
    let mut matches: Vec<Target> = Vec::new();
    let mut scanned: Vec<String> = Vec::new();
    for proc in &processes {
        let exe = PathBuf::from(proc.exe.trim_matches('"'));
        if exe.as_os_str().is_empty() {
            continue;
        }
        // The descriptor sits next to the player executable (Windows); inside a macOS .app
        // bundle it is at the bundle root, so the bundle ancestor is tried too.
        let mut dirs: Vec<PathBuf> = exe
            .parent()
            .filter(|d| d.is_dir())
            .map(|d| vec![d.to_path_buf()])
            .unwrap_or_default();
        let mut cur = exe.parent().map(Path::to_path_buf);
        for _ in 0..4 {
            let Some(d) = cur else { break };
            if d.extension().and_then(|e| e.to_str()) == Some("app") {
                dirs.push(d);
                break;
            }
            cur = d.parent().map(Path::to_path_buf);
        }
        scanned.push(exe.display().to_string());
        for dir in dirs {
            let Ok(desc) = read_runtime_descriptor(&dir) else {
                continue;
            };
            let work = desc
                .working_directory
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or(dir);
            matches.push(Target {
                port: desc.port,
                project_path: work,
                eval_token: desc.eval_token,
                pid: desc.pid,
                info: None,
                is_runtime: true,
                is_adb: false,
            });
            break;
        }
    }
    match matches.len() {
        0 if scanned.is_empty() => Err(anyhow!(
            "no running process matches '{name}'. Pass the player executable name (e.g. --runtime MyGame.exe) or its build directory (--runtime-path)."
        )),
        0 => Err(anyhow!(
            "process(es) matching '{name}' have no runtime descriptor next to their executable (scanned: {}).\n\nThe player must be built with the Pipeline runtime server enabled. Use --runtime-path to point at the directory holding .unity-pipeline-runtime-port.",
            scanned.join(", ")
        )),
        1 => Ok(matches.remove(0)),
        n => {
            let list = matches
                .iter()
                .enumerate()
                .map(|(i, t)| format!("  {}. pid {:?} at 127.0.0.1:{}", i + 1, t.pid, t.port))
                .collect::<Vec<_>>()
                .join("\n");
            Err(anyhow!(
                "{n} running players match '{name}':\n\n{list}\n\nUse --runtime-path <dir> to pick one."
            ))
        }
    }
}

/// `--adb` selects an Android player over adb; `--runtime`/`--runtime-path` select a desktop
/// player; with neither, fall back to editor resolution. One branch shared by `command` and `mcp`.
pub fn resolve_target(
    runtime: Option<&str>,
    runtime_path: Option<&Path>,
    adb_package: Option<&str>,
    adb_serial: Option<&str>,
    project_path: Option<&str>,
) -> Result<Target> {
    if let Some(package) = adb_package {
        return crate::adb::resolve_adb_target(adb_serial, package);
    }
    if runtime.is_some() || runtime_path.is_some() {
        return find_runtime_target(runtime, runtime_path);
    }
    crate::pipeline::find_target(project_path)
}

/// `Company\nProductName` two-liner Unity writes into `<game>_Data/app.info` on Windows
/// players — the only local map from a build dir to its `%USERPROFILE%\AppData\LocalLow`
/// log folder.
pub fn parse_app_info(text: &str) -> Option<(String, String)> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let company = lines.next()?.to_owned();
    let product = lines.next()?.to_owned();
    (!company.is_empty() && !product.is_empty()).then_some((company, product))
}

/// Player.log for a player build dir. Windows players write it to
/// `%USERPROFILE%\AppData\LocalLow\<Company>\<Product>\Player.log`; the company/product pair
/// comes from the build's `*_Data/app.info`. Returns None when any step is missing — the
/// caller then asks for an explicit path.
pub fn find_player_log(build_dir: &Path) -> Option<PathBuf> {
    let data_dir = fs::read_dir(build_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .find(|e| {
            e.path().is_dir() && e.file_name().to_string_lossy().ends_with("_Data")
        })?
        .path();
    let (company, product) = parse_app_info(&fs::read_to_string(data_dir.join("app.info")).ok()?)?;
    let log = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)?
        .join("AppData")
        .join("LocalLow")
        .join(company)
        .join(product)
        .join("Player.log");
    log.is_file().then_some(log)
}

#[cfg(test)]
#[path = "../tests/runtime.rs"]
mod tests;
