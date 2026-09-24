use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::json;

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
        eprintln!("提示：以上配置未绑定具体项目。");
        eprintln!("  • 只开着 1 个装有 Pipeline 的 Unity Editor 时，MCP 会自动连接它。");
        eprintln!("  • 同时开多个项目时，MCP 会因歧义而启动失败。");
        eprintln!("    请在 args 中追加 \"--project-path\" \"<项目绝对路径>\" 以锁定目标，例如：");
        eprintln!("      \"args\": [\"mcp\", \"--project-path\", \"D:\\\\path\\\\to\\\\YourProject\"]");
        eprintln!("  • 每个项目可各配置一条 mcpServers 条目（键名不同），互不干扰。");
    } else {
        eprintln!();
        eprintln!("提示：以上配置已绑定到指定项目，多项目并存时不会串。");
        eprintln!("  若需为其他项目也接入 MCP，请再运行一次 `unity install` 或手动复制该条目、");
        eprintln!("  修改键名及 --project-path 值后追加到 mcpServers 中。");
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
