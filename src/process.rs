#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::process::{Command, Stdio};

use anyhow::{anyhow, Result};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use anyhow::Context;
#[cfg(target_os = "windows")]
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub cmd: String,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Deserialize)]
struct RawProcess {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "ProcessId")]
    process_id: Option<serde_json::Value>,
    #[serde(rename = "CommandLine")]
    command_line: Option<String>,
    #[serde(rename = "ExecutablePath")]
    executable_path: Option<String>,
}

/// Find all processes on Windows whose name contains `pattern` (case-insensitive
/// substring against Name / ExecutablePath / CommandLine, matching the JS impl).
#[cfg(target_os = "windows")]
pub fn find_processes_by_name(pattern: &str) -> Result<Vec<ProcessInfo>> {
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance -ClassName Win32_Process | Select-Object Name,ProcessId,ParentProcessId,CommandLine,ExecutablePath | ConvertTo-Json -Compress -Depth 2",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("failed to spawn powershell to enumerate processes")?;

    if !output.status.success() {
        return Err(anyhow!(
            "powershell exited with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let parsed: serde_json::Value =
        serde_json::from_str(trimmed).context("failed to parse Get-CimInstance JSON output")?;
    let entries: Vec<RawProcess> = match parsed {
        serde_json::Value::Array(_) => serde_json::from_value(parsed)?,
        serde_json::Value::Object(_) => vec![serde_json::from_value(parsed)?],
        _ => Vec::new(),
    };

    let needle = pattern.to_lowercase();
    let mut result = Vec::new();
    for row in entries {
        let name = row.name.unwrap_or_default();
        let bin = row.executable_path.unwrap_or_default();
        let cmd = row.command_line.unwrap_or_default();
        let pid = match row.process_id {
            Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0) as u32,
            Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0),
            _ => 0,
        };
        if pid == 0 {
            continue;
        }
        let haystack = format!("{}\n{}\n{}", name, bin, cmd).to_lowercase();
        if !haystack.contains(&needle) {
            continue;
        }
        result.push(ProcessInfo { pid, cmd });
    }
    Ok(result)
}

/// Enumerate processes via `ps` on macOS. `-A` lists all users, `-ww` disables
/// column truncation so long Unity command lines survive intact, and `pid=`/
/// `command=` suppress the header so we can parse a stable two-column layout.
#[cfg(target_os = "macos")]
pub fn find_processes_by_name(pattern: &str) -> Result<Vec<ProcessInfo>> {
    let output = Command::new("ps")
        .args(["-Awwo", "pid=,command="])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("failed to spawn ps to enumerate processes")?;

    if !output.status.success() {
        return Err(anyhow!(
            "ps exited with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let needle = pattern.to_lowercase();
    let mut result = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        // Split into `<pid> <command line>` on the first run of whitespace.
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let pid_str = parts.next().unwrap_or("");
        let cmd = parts.next().unwrap_or("").trim().to_owned();
        let pid: u32 = match pid_str.parse() {
            Ok(n) if n > 0 => n,
            _ => continue,
        };
        if cmd.is_empty() {
            continue;
        }
        if !cmd.to_lowercase().contains(&needle) {
            continue;
        }
        result.push(ProcessInfo { pid, cmd });
    }
    Ok(result)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn find_processes_by_name(_pattern: &str) -> Result<Vec<ProcessInfo>> {
    Err(anyhow!(
        "process discovery is only implemented for Windows and macOS in this build"
    ))
}
