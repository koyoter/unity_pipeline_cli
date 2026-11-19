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
/// `command` is always the absolute path of the running binary — never a bare
/// name resolved through PATH, which the MCP host may not share (and might
/// shadow with an older copy).
///
/// Two clearly separated sections are printed: the Editor (Unity project)
/// connection and the Player (built program) connection — each with its own
/// entry and hints. When `project_path` is provided, the editor entry's args
/// pin the server to that specific project, so multiple Editors can coexist
/// without ambiguity. When `runtime_path` is provided, ONLY the Player section
/// is printed, with that path baked in. When `trace` is set, every entry
/// carries `UNITY_MCP_TRACE` pointing at a log file next to the binary,
/// turning on the server's full protocol trace (see `mcp.rs`).
pub fn print_mcp_config(
    project_path: Option<&Path>,
    runtime_path: Option<&Path>,
    adb_package: Option<&str>,
    adb_serial: Option<&str>,
    trace: bool,
) -> Result<()> {
    let exe = std::env::current_exe()?;
    // Normalize to an absolute path. The MCP host is a separate process whose
    // PATH may be stale or shadowed by an older copy of this binary; and macOS's
    // _NSGetExecutablePath is not required to return an absolute path at all.
    let exe = std::path::absolute(&exe).unwrap_or(exe);
    let command = exe.to_string_lossy().into_owned();
    let stem = server_name(&exe);
    let trace_env = if trace {
        exe.parent().map(|dir| {
            json!({
                "UNITY_MCP_TRACE": dir
                    .join("unity-mcp-trace.log")
                    .to_string_lossy()
                    .into_owned(),
            })
        })
    } else {
        None
    };

    // Explicit single-target requests print exactly one section.
    if let Some(rp) = runtime_path {
        let entry = mcp_entry(
            &command,
            &["mcp", "--runtime-path", &rp.to_string_lossy()],
            trace_env.as_ref(),
        );
        print_section(&t("configure.section_player"), &format!("{stem}-player"), &entry);
        eprintln!("{}", t("configure.hint_player_entry"));
        eprintln!("{}", t("configure.hint_player_degrade"));
        eprintln!("{}", t("configure.hint_player_move"));
        return Ok(());
    }
    if let Some(package) = adb_package {
        let mut args = vec!["mcp".to_owned(), "--adb".to_owned(), package.to_owned()];
        if let Some(s) = adb_serial {
            args.push("--adb-serial".to_owned());
            args.push(s.to_owned());
        }
        let entry = mcp_entry(
            &command,
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            trace_env.as_ref(),
        );
        print_section(&t("configure.section_android"), &format!("{stem}-android"), &entry);
        eprintln!("{}", t("configure.hint_player_entry"));
        eprintln!("{}", t("configure.hint_player_degrade"));
        eprintln!("{}", t("configure.hint_player_android"));
        return Ok(());
    }

    // Editor section: pin args when a project was given.
    let editor_entry = match project_path {
        Some(p) => mcp_entry(
            &command,
            &["mcp", "--project-path", &p.to_string_lossy()],
            trace_env.as_ref(),
        ),
        None => mcp_entry(&command, &["mcp"], trace_env.as_ref()),
    };
    print_section(&t("configure.section_editor"), &stem, &editor_entry);
    if project_path.is_none() {
        eprintln!("{}", t("configure.hint_not_pinned"));
        eprintln!("{}", t("configure.hint_single_editor"));
        eprintln!("{}", t("configure.hint_multi_project_ambiguous"));
        eprintln!("{}", t("configure.hint_project_path_arg"));
        eprintln!(
            "      \"args\": [\"mcp\", \"--project-path\", \"{}\"]",
            project_path_example()
        );
        eprintln!("{}", t("configure.hint_per_project_entry"));
    } else {
        eprintln!("{}", t("configure.hint_pinned"));
        eprintln!("{}", t("configure.hint_other_projects_1"));
        eprintln!("{}", t("configure.hint_other_projects_2"));
    }

    // Desktop Player section: same binary, runtime flags — the path form is
    // platform-specific (Windows: the build dir; macOS: the .app bundle).
    eprintln!();
    let entry = mcp_entry(
        &command,
        &["mcp", "--runtime-path", runtime_path_placeholder()],
        trace_env.as_ref(),
    );
    print_section(&t("configure.section_player"), &format!("{stem}-player"), &entry);
    eprintln!("{}", t("configure.hint_player_entry"));
    eprintln!("{}", t("configure.hint_player_degrade"));
    eprintln!("{}", t("configure.hint_player_move"));

    // Android Player section: its own entry — package name (and optionally a serial) to adapt.
    eprintln!();
    let entry = mcp_entry(&command, &["mcp", "--adb", "com.example.yourgame"], trace_env.as_ref());
    print_section(&t("configure.section_android"), &format!("{stem}-android"), &entry);
    eprintln!("{}", t("configure.hint_player_entry"));
    eprintln!("{}", t("configure.hint_player_degrade"));
    eprintln!("{}", t("configure.hint_player_android"));
    Ok(())
}

/// Example project path in the pin-a-project hint, in the current platform's form.
fn project_path_example() -> &'static str {
    if cfg!(target_os = "macos") {
        "/path/to/YourProject"
    } else {
        "D:\\\\path\\\\to\\\\YourProject"
    }
}

/// Placeholder build location for the player entry, in the current platform's form
/// (matching the package README: Windows — the directory next to the exe; macOS — the .app bundle).
fn runtime_path_placeholder() -> &'static str {
    if cfg!(target_os = "macos") {
        "/Users/you/Builds/YourGame.app"
    } else {
        "C:\\Builds\\YourGame"
    }
}

fn mcp_entry(command: &str, args: &[&str], env: Option<&serde_json::Value>) -> serde_json::Value {
    let mut entry = json!({
        "command": command,
        "args": args,
    });
    if let Some(env) = env {
        entry["env"] = env.clone();
    }
    entry
}

fn print_section(title: &str, key: &str, entry: &serde_json::Value) {
    eprintln!("{}", title);
    let config = json!({
        "mcpServers": {
            key: entry
        }
    });
    println!("{}", crate::json::pretty(&config));
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
