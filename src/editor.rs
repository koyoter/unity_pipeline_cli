use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use crate::process::{find_processes_by_name, ProcessInfo};

/// Descriptor written by the Pipeline package into
/// `<project>/Library/Pipeline/.unity-pipeline-port`.
///
/// The server's identity and guidance fields are read even where this client only needs the
/// port: `info` is published by the server specifically so that every client reads it before
/// issuing commands, and the identity fields are authoritative where the CLI would otherwise
/// re-derive them from the process table and `ProjectVersion.txt`.
#[derive(Debug, Deserialize)]
pub struct PortDescriptor {
    pub port: u16,
    #[serde(default, rename = "evalToken")]
    pub eval_token: Option<String>,
    /// Guidance the Editor wants the client to know about itself, e.g. that it was not started
    /// with `-automated` and can therefore get stuck on modal dialogs. Absent when there is
    /// nothing to say.
    #[serde(default)]
    pub info: Option<String>,
    /// `"editor"` or `"batchmode"`. A batchmode Editor cannot show modal dialogs at all.
    #[serde(default, rename = "mode")]
    pub mode: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default, rename = "projectName")]
    pub project_name: Option<String>,
    #[serde(default, rename = "unityVersion")]
    pub unity_version: Option<String>,
}

#[derive(Debug)]
pub struct EditorInstance {
    pub project_path: PathBuf,
    pub project_name: String,
    pub pid: u32,
    pub unity_version: Option<String>,
    pub has_pipeline: bool,
    pub pipeline_version: Option<String>,
    pub descriptor: Option<PortDescriptor>,
    pub is_reachable: bool,
}

pub fn is_unity_project(dir: &Path) -> bool {
    dir.join("Assets").is_dir()
}

pub fn get_unity_project_version(dir: &Path) -> Option<String> {
    let file = dir.join("ProjectSettings").join("ProjectVersion.txt");
    let content = fs::read_to_string(file).ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("m_EditorVersion:") {
            return Some(rest.trim().to_owned());
        }
    }
    None
}

pub fn is_pipeline_installed(dir: &Path) -> bool {
    let manifest = dir.join("Packages").join("manifest.json");
    if let Ok(text) = fs::read_to_string(&manifest) {
        if let Ok(json) = serde_json::from_str::<Value>(&text) {
            if let Some(deps) = json.get("dependencies").and_then(|v| v.as_object()) {
                if deps.contains_key("com.unity.pipeline") {
                    return true;
                }
            }
        }
    }
    dir.join("Packages").join("com.unity.pipeline").is_dir()
}

pub fn get_pipeline_version(dir: &Path) -> Option<String> {
    let lock = dir.join("Packages").join("packages-lock.json");
    if let Ok(text) = fs::read_to_string(&lock) {
        if let Ok(json) = serde_json::from_str::<Value>(&text) {
            if let Some(v) = json
                .pointer("/dependencies/com.unity.pipeline/version")
                .and_then(|v| v.as_str())
            {
                return Some(v.to_owned());
            }
        }
    }
    let manifest = dir.join("Packages").join("manifest.json");
    if let Ok(text) = fs::read_to_string(&manifest) {
        if let Ok(json) = serde_json::from_str::<Value>(&text) {
            if let Some(v) = json
                .pointer("/dependencies/com.unity.pipeline")
                .and_then(|v| v.as_str())
            {
                return Some(v.to_owned());
            }
        }
    }
    let embedded = dir
        .join("Packages")
        .join("com.unity.pipeline")
        .join("package.json");
    if let Ok(text) = fs::read_to_string(&embedded) {
        if let Ok(json) = serde_json::from_str::<Value>(&text) {
            if let Some(v) = json.get("version").and_then(|v| v.as_str()) {
                return Some(v.to_owned());
            }
        }
    }
    None
}

pub fn read_editor_descriptor(dir: &Path) -> Option<PortDescriptor> {
    let path = dir
        .join("Library")
        .join("Pipeline")
        .join(".unity-pipeline-port");
    let text = fs::read_to_string(&path).ok()?;
    let parsed: PortDescriptor = serde_json::from_str(&text).ok()?;
    if parsed.port < 1 {
        return None;
    }
    Some(parsed)
}

/// Best-effort port of the JS `extractProjectPath`. Locates `-projectPath` or
/// `-createProject` (case-insensitive), then extracts the value — either a
/// quoted string or an unquoted run of tokens up to the next ` -<flag>` (or
/// end of string). The Rust `regex` crate has no look-ahead, so the terminator
/// is found by manual scanning instead of a lookahead assertion.
fn extract_project_path(cmd_line: &str) -> Option<(String, bool, String)> {
    let lower = cmd_line.to_lowercase();
    let flag = find_flag(&lower, &["-projectpath", "-createproject"])?;
    let after_flag = flag.end;

    // Skip whitespace between the flag and its value.
    let bytes = cmd_line.as_bytes();
    let mut i = after_flag;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }

    let value_start = i;
    let quote = bytes[value_start];
    if quote == b'"' || quote == b'\'' {
        let after_quote = value_start + 1;
        let close = cmd_line[after_quote..].find(quote as char)?;
        let end = after_quote + close;
        let path = cmd_line[after_quote..end].to_owned();
        let remainder = cmd_line[(end + 1)..].to_owned();
        return Some((path, true, remainder));
    }

    // Unquoted: consume until we see ` -<nonspace>` or EOL.
    let mut cursor = value_start;
    while cursor < bytes.len() {
        if bytes[cursor] == b' ' || bytes[cursor] == b'\t' {
            // Look ahead past the whitespace run.
            let mut j = cursor;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'-' && j + 1 < bytes.len() && !bytes[j + 1].is_ascii_whitespace() {
                break;
            }
            cursor = j;
        } else {
            cursor += 1;
        }
    }
    let path = cmd_line[value_start..cursor].trim_end().to_owned();
    if path.is_empty() {
        return None;
    }
    let remainder = cmd_line[cursor..].to_owned();
    Some((path, false, remainder))
}

struct FlagHit {
    end: usize,
}

/// Find the earliest whole-word occurrence of any flag name in `lower`
/// (already lowercased). Returns the byte offset just past the flag name.
fn find_flag(lower: &str, flags: &[&str]) -> Option<FlagHit> {
    let bytes = lower.as_bytes();
    let mut best: Option<usize> = None;
    for flag in flags {
        let fb = flag.as_bytes();
        let mut search = 0usize;
        while let Some(off) = lower[search..].find(*flag) {
            let abs = search + off;
            let after = abs + fb.len();
            let before_ok = abs == 0 || bytes[abs - 1].is_ascii_whitespace();
            let after_ok = after >= bytes.len() || bytes[after].is_ascii_whitespace();
            if before_ok && after_ok {
                best = Some(match best {
                    Some(existing) if existing < after => existing,
                    _ => after,
                });
                break;
            }
            search = abs + 1;
        }
    }
    best.map(|end| FlagHit { end })
}

fn path_has_assets(candidate: &str) -> bool {
    Path::new(candidate).join("Assets").is_dir()
}

/// Re-absorb tokens from `remainder` into `candidate` until `Assets/` appears,
/// matching the JS heuristic for unquoted paths that contain spaces.
fn extend_unquoted(candidate: &str, remainder: &str) -> String {
    if path_has_assets(candidate) {
        return candidate.to_owned();
    }
    let token_re = Regex::new(r"^(\s+)(\S+)").unwrap();
    let mut extended = candidate.to_owned();
    let mut rest = remainder.to_owned();
    for _ in 0..10 {
        if rest.is_empty() {
            break;
        }
        let m = match token_re.captures(&rest) {
            Some(c) => c,
            None => break,
        };
        let whole = m.get(0).unwrap().as_str().to_owned();
        let ws = m.get(1).map(|g| g.as_str()).unwrap_or("");
        let tok = m.get(2).map(|g| g.as_str()).unwrap_or("");
        extended.push_str(ws);
        extended.push_str(tok);
        rest = rest[whole.len()..].to_owned();
        if path_has_assets(&extended) {
            return extended;
        }
    }
    candidate.to_owned()
}

fn is_worker_process(cmd_line: &str) -> bool {
    cmd_line.contains("AssetImportWorker")
        || cmd_line.contains("-batchmode")
        || (cmd_line.contains("-name") && cmd_line.contains("AssetImport"))
}

/// Platform-specific substring used to locate the Unity Editor process.
/// Windows binaries are named `Unity.exe`; macOS ships `Unity.app/Contents/MacOS/Unity`.
#[cfg(target_os = "windows")]
const UNITY_PROCESS_NEEDLE: &str = "Unity.exe";
#[cfg(target_os = "macos")]
const UNITY_PROCESS_NEEDLE: &str = "Unity.app/Contents/MacOS/Unity";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const UNITY_PROCESS_NEEDLE: &str = "Unity";

/// On macOS, only the main Editor process has argv[0] ending in
/// `Unity.app/Contents/MacOS/Unity`. Auxiliary helpers live under
/// `Contents/Frameworks/`, `Contents/Tools/`, `Contents/Resources/`, etc., so
/// filtering by argv[0] suffix keeps us from treating them as Editors.
#[cfg(target_os = "macos")]
fn is_main_editor_argv0(cmd_line: &str) -> bool {
    let argv0 = cmd_line.split_whitespace().next().unwrap_or("");
    argv0.ends_with("/Unity.app/Contents/MacOS/Unity")
}

/// Read the working directory of `pid` via `lsof`. Unity Hub on macOS launches
/// the Editor without a `-projectPath` argument, but the process's cwd is
/// always the opened project root, so cwd is our best fallback.
#[cfg(target_os = "macos")]
fn read_process_cwd(pid: u32) -> Option<PathBuf> {
    use std::process::{Command, Stdio};
    let output = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // lsof -F emits records like "p<pid>\nfcwd\nn<path>\n"; grab the `n`-line.
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(rest) = line.strip_prefix('n') {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                return Some(PathBuf::from(trimmed));
            }
        }
    }
    None
}

/// Return every running Unity Editor instance, along with its project path.
///
/// The project path is discovered by two complementary strategies:
///   1. Parse `-projectPath` / `-createProject` from the command line
///      (Unity Hub on Windows and any CLI launch on either OS use this).
///   2. On macOS, when the flag is absent (Unity Hub launches the Editor via
///      LaunchServices without command-line args), fall back to the process's
///      current working directory, which is always the opened project root.
pub fn get_running_unity_editors() -> Result<Vec<(PathBuf, u32)>> {
    let processes: Vec<ProcessInfo> =
        find_processes_by_name(UNITY_PROCESS_NEEDLE).unwrap_or_default();
    let mut editors = Vec::new();
    for proc in processes {
        let cmd_line = proc.cmd.clone();
        if cmd_line.is_empty() {
            continue;
        }
        if is_worker_process(&cmd_line) {
            continue;
        }
        #[cfg(target_os = "macos")]
        {
            if !is_main_editor_argv0(&cmd_line) {
                continue;
            }
        }

        let path_from_flag = extract_project_path(&cmd_line).map(|(raw_path, quoted, remainder)| {
            if !quoted && Path::new(&raw_path).is_absolute() {
                extend_unquoted(&raw_path, &remainder)
            } else {
                raw_path
            }
        });

        let path_buf = match path_from_flag {
            Some(p) => PathBuf::from(p),
            None => {
                #[cfg(target_os = "macos")]
                {
                    match read_process_cwd(proc.pid) {
                        Some(p) => p,
                        None => continue,
                    }
                }
                #[cfg(not(target_os = "macos"))]
                {
                    continue;
                }
            }
        };
        if !path_buf.is_absolute() {
            continue;
        }
        let normalized = normalize_path(&path_buf);
        editors.push((normalized, proc.pid));
    }
    Ok(editors)
}

fn normalize_path(p: &Path) -> PathBuf {
    // On Windows, canonicalize would emit `\\?\` prefixes; the plain absolute
    // form the JS shows is closer to what a user pasted, so we prefer it.
    if let Ok(canon) = fs::canonicalize(p) {
        let s = canon.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
        return canon;
    }
    p.to_path_buf()
}

/// Assemble the same per-instance record the `pipeline list` handler emits.
pub fn discover_editor_instances() -> Result<Vec<EditorInstance>> {
    let mut out = Vec::new();
    let editors = get_running_unity_editors()?;
    for (project_path, pid) in editors {
        if !is_unity_project(&project_path) {
            continue;
        }
        let has_pipeline = is_pipeline_installed(&project_path);
        let pipeline_version = if has_pipeline {
            get_pipeline_version(&project_path)
        } else {
            None
        };
        let descriptor = if has_pipeline {
            read_editor_descriptor(&project_path)
        } else {
            None
        };
        let is_reachable = descriptor
            .as_ref()
            .map(|d| crate::http::probe_pipeline(d.port, d.eval_token.as_deref()))
            .unwrap_or(false);
        // The descriptor is authoritative once the Editor has written one — the process table
        // and ProjectVersion.txt are only fallbacks for a project that has not started a server.
        let project_name = descriptor
            .as_ref()
            .and_then(|d| d.project_name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| {
                project_path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| project_path.to_string_lossy().to_string())
            });
        let unity_version = descriptor
            .as_ref()
            .and_then(|d| d.unity_version.clone())
            .or_else(|| get_unity_project_version(&project_path));
        let pid = descriptor.as_ref().and_then(|d| d.pid).unwrap_or(pid);

        out.push(EditorInstance {
            project_path,
            project_name,
            pid,
            unity_version,
            has_pipeline,
            pipeline_version,
            descriptor,
            is_reachable,
        });
    }
    Ok(out)
}
