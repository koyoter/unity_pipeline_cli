use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

use crate::i18n::{t, tr};
use crate::pipeline::{
    ensure_reachable, execute, fetch_commands, find_target, tool_name, ExecBudget, Target,
};

pub struct Options {
    /// Command name exactly as the server resolves it — `"asset create"`, `asset.create`,
    /// or a pre-0.9 flat name — forwarded verbatim.
    pub name: Option<String>,
    /// The command's parameters as a JSON object string, forwarded verbatim.
    pub params_json: Option<String>,
    pub project_path: Option<String>,
    pub timeout_seconds: u64,
    pub as_json: bool,
}

const CATALOG_TIMEOUT: Duration = Duration::from_secs(10);

pub fn run(opts: Options) -> Result<()> {
    let target = find_target(opts.project_path.as_deref())?;
    ensure_reachable(&target)?;

    match opts.name {
        None => list_commands(&target, opts.as_json),
        Some(name) => {
            let params = parse_params(opts.params_json.as_deref())?;
            run_command(&target, &name, &params, opts.timeout_seconds, opts.as_json)
        }
    }
}

/// Parse the optional parameters JSON. `None`, an empty string and `"null"` all mean "no
/// parameters"; anything else must be a JSON object. No local flag parsing and no schema
/// validation: the object is forwarded verbatim and the server binds it against the command's
/// real parameter types, reporting problems as `argProblems` for the caller to fix.
fn parse_params(raw: Option<&str>) -> Result<Value> {
    let Some(raw) = raw else {
        return Ok(json!({}));
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(json!({}));
    }
    let value: Value = serde_json::from_str(trimmed)
        .with_context(|| tr("command.params_parse_failed", &[&trimmed]))?;
    match value {
        Value::Object(_) => Ok(value),
        _ => Err(anyhow!("{}", t("command.params_not_object"))),
    }
}

fn list_commands(target: &Target, as_json: bool) -> Result<()> {
    let catalog = fetch_commands(target, CATALOG_TIMEOUT)?;
    if as_json {
        let envelope = json!({
            "command": "command",
            "target": {
                "host": "127.0.0.1",
                "port": target.port,
                "projectPath": target.project_path.to_string_lossy(),
            },
            "server": catalog.server.clone().unwrap_or(Value::Null),
            "count": catalog.count.unwrap_or(catalog.commands.len() as u64),
            "commands": catalog.commands.iter().map(command_to_json).collect::<Vec<_>>(),
        });
        println!("{}", crate::json::pretty(&envelope));
        return Ok(());
    }

    println!("{}", tr("command.connected", &[&target.port]));
    println!("{}", tr("command.project", &[&target.project_path.display()]));
    if let Some(server) = &catalog.server {
        let version = server.get("version").and_then(|v| v.as_str()).unwrap_or("?");
        let port = server.get("port").and_then(|v| v.as_u64()).unwrap_or(target.port as u64);
        println!("{}", tr("command.server_version", &[&version, &port]));
    }
    let cmds = &catalog.commands;
    if cmds.is_empty() {
        println!("{}", t("command.no_commands"));
        return Ok(());
    }
    println!("{}", tr("command.available_commands", &[&cmds.len()]));
    let width = cmds.len().to_string().len();
    for (idx, cmd) in cmds.iter().enumerate() {
        let name = tool_name(cmd).unwrap_or_else(|| "<unnamed>".to_owned());
        let summary = cmd
            .description
            .as_deref()
            .and_then(|d| d.lines().next())
            .filter(|l| !l.is_empty())
            .unwrap_or("");
        println!("  {:>width$}. {name}  {summary}", idx + 1, width = width);
    }
    println!("{}", t("command.usage"));
    println!("{}", t("command.example"));
    Ok(())
}

fn run_command(
    target: &Target,
    name: &str,
    params: &Value,
    timeout_seconds: u64,
    as_json: bool,
) -> Result<()> {
    // `--timeout` bounds the command itself, not just this process's wait: the server aborts a
    // main-thread command once the budget in the request is exceeded (60s if none is sent), so
    // passing it through is what makes an explicitly raised timeout mean anything.
    let timeout = Duration::from_secs(timeout_seconds.max(1));
    let result = match execute(target, name, params, ExecBudget::shared(timeout)) {
        Ok(v) => v,
        Err(err) => {
            if !as_json {
                eprintln!("{}", tr("command.execute_failed", &[&name]));
                eprintln!("   {err:#}");
                if let Some(note) = crate::pipeline::host_note(target, &err) {
                    eprintln!("⚠️  {note}");
                }
            }
            return Err(err);
        }
    };

    if as_json {
        let envelope = json!({
            "command": format!("command {name}"),
            "success": true,
            "target": {
                "host": "127.0.0.1",
                "port": target.port,
                "projectPath": target.project_path.to_string_lossy(),
            },
            "parameters": params,
            "result": result,
        });
        println!("{}", crate::json::pretty(&envelope));
        return Ok(());
    }

    println!("{}", tr("command.executed", &[&name]));
    println!("🔗 127.0.0.1:{}", target.port);
    if let Some(obj) = params.as_object() {
        if !obj.is_empty() {
            let pretty = crate::json::compact(params);
            println!("{}", tr("command.parameters_line", &[&pretty]));
        }
    }
    println!("{}", t("command.success"));
    match &result {
        Value::Null => println!("{}", t("command.no_return_value")),
        Value::String(s) => println!("{}", tr("command.result_line", &[&s])),
        other => {
            println!("{}", t("command.result_label"));
            println!("{}", crate::json::pretty(other));
        }
    }
    Ok(())
}

/// One catalog entry for the `--json` listing: the tool name plus everything the server
/// reported about the command.
fn command_to_json(cmd: &crate::pipeline::PipelineCommand) -> Value {
    let params: Vec<Value> = cmd
        .parameters
        .iter()
        .map(|p| {
            let mut obj = serde_json::Map::new();
            if let Some(n) = &p.name {
                obj.insert("name".to_owned(), Value::String(n.clone()));
            }
            if let Some(ty) = &p.ty {
                obj.insert("type".to_owned(), Value::String(ty.clone()));
                obj.insert(
                    "jsonType".to_owned(),
                    Value::String(
                        crate::pipeline::pipeline_type_to_json_schema(Some(ty)).to_owned(),
                    ),
                );
            }
            if let Some(d) = &p.description {
                obj.insert("description".to_owned(), Value::String(d.clone()));
            }
            obj.insert("required".to_owned(), Value::Bool(p.required));
            if let Some(dv) = &p.default_value {
                obj.insert("defaultValue".to_owned(), dv.clone());
            }
            Value::Object(obj)
        })
        .collect();
    let mut obj = serde_json::Map::new();
    if let Some(n) = tool_name(cmd) {
        obj.insert("name".to_owned(), Value::String(n));
    }
    if let Some(full) = &cmd.command {
        obj.insert("command".to_owned(), Value::String(full.clone()));
    }
    if let Some(d) = &cmd.description {
        obj.insert("description".to_owned(), Value::String(d.clone()));
    }
    obj.insert("parameters".to_owned(), Value::Array(params));
    Value::Object(obj)
}

#[cfg(test)]
#[path = "../tests/command.rs"]
mod tests;
