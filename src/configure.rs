use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::json;

use crate::i18n::t;

/// Print a generic MCP `mcpServers` config entry pointing at the current
/// binary. Users can paste this into any client that accepts the standard
/// `{ "mcpServers": { "<name>": { "command": ..., "args": [...] } } }` shape
/// (Claude Desktop, Antigravity, etc.), or adapt it to a schema variant that
/// uses `servers` / `context_servers`.
///
/// When `project_path` is provided, the printed args pin the server to that
/// specific Unity project, so multiple Editors can coexist without ambiguity.
pub fn print_mcp_config(project_path: Option<&Path>) -> Result<()> {
    let exe = std::env::current_exe()?;
    let command = format_exe_for_config(&exe);
    let name = server_name(&exe);
    let mut args: Vec<String> = vec!["mcp".to_owned()];
    if let Some(p) = project_path {
        args.push("--project-path".to_owned());
        args.push(p.to_string_lossy().into_owned());
    }
    let config = json!({
        "mcpServers": {
            name: {
                "command": command,
                "args": args,
            }
        }
    });
    println!("{}", crate::json::pretty(&config));
    if project_path.is_none() {
        eprintln!();
        eprintln!("{}", t("configure.hint_not_pinned"));
        eprintln!("{}", t("configure.hint_single_editor"));
        eprintln!("{}", t("configure.hint_multi_project_ambiguous"));
        eprintln!("{}", t("configure.hint_project_path_arg"));
        eprintln!("      \"args\": [\"mcp\", \"--project-path\", \"D:\\\\path\\\\to\\\\YourProject\"]");
        eprintln!("{}", t("configure.hint_per_project_entry"));
    } else {
        eprintln!();
        eprintln!("{}", t("configure.hint_pinned"));
        eprintln!("{}", t("configure.hint_other_projects_1"));
        eprintln!("{}", t("configure.hint_other_projects_2"));
    }
    Ok(())
}

/// Prefer a bare `unity` invocation when the binary is on PATH, otherwise emit
/// the fully-qualified executable path so agents can spawn it directly.
fn format_exe_for_config(exe: &PathBuf) -> String {
    if let Some(stem) = exe.file_stem().and_then(|s| s.to_str()) {
        if which_on_path(stem).is_some() {
            return stem.to_owned();
        }
    }
    exe.to_string_lossy().into_owned()
}

/// Derive the MCP server entry key from the binary stem (falls back to
/// "unity"), so the printed config matches whatever the exe is called.
fn server_name(exe: &PathBuf) -> String {
    exe.file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_owned())
        .unwrap_or_else(|| "unity".to_owned())
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let bare = dir.join(name);
        if bare.is_file() {
            return Some(bare);
        }
        for ext in windows_pathext() {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// PATHEXT-derived extension list, populated only on Windows. On Unix the
/// binary is expected to be found by its bare name, so this returns empty.
#[cfg(target_os = "windows")]
fn windows_pathext() -> Vec<String> {
    let pathext = std::env::var_os("PATHEXT")
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_else(|| ".EXE;.BAT;.CMD".to_owned());
    pathext
        .split(';')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(not(target_os = "windows"))]
fn windows_pathext() -> Vec<String> {
    Vec::new()
}
