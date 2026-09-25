use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};

use crate::i18n::{t, tr};
use crate::pipeline::{
    self, ensure_reachable, execute, fetch_commands, find_target, is_failed_eval_envelope,
    ExecBudget, PipelineCommand, Target,
};

pub struct Options {
    pub name: Option<String>,
    pub args: Vec<String>,
    pub project_path: Option<String>,
    pub timeout_seconds: u64,
    pub as_json: bool,
}

const CATALOG_TIMEOUT: Duration = Duration::from_secs(10);

pub fn run(opts: Options) -> Result<()> {
    let Options {
        name,
        args,
        mut project_path,
        mut timeout_seconds,
        mut as_json,
    } = opts;

    // Mirror the JS `separateCliOptions`: if a user passed --project-path /
    // --timeout / --json AFTER the command name (which clap treats as trailing
    // args), pull them back out here so both invocation styles work.
    let args = separate_cli_options(
        args,
        &mut project_path,
        &mut timeout_seconds,
        &mut as_json,
    );

    let target = find_target(project_path.as_deref())?;
    ensure_reachable(&target)?;

    match &name {
        None => list_commands(&target, as_json),
        Some(name) => run_command(&target, name, &args, timeout_seconds, as_json),
    }
}

fn separate_cli_options(
    input: Vec<String>,
    project_path: &mut Option<String>,
    timeout_seconds: &mut u64,
    as_json: &mut bool,
) -> Vec<String> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        let arg = &input[i];
        let next = input.get(i + 1);
        let take_next = || -> Option<String> { next.cloned() };
        if arg == "--project-path" {
            if let Some(v) = take_next() {
                *project_path = Some(v);
                i += 2;
                continue;
            }
        } else if arg == "--timeout" {
            if let Some(v) = take_next() {
                if let Ok(n) = v.parse::<u64>() {
                    *timeout_seconds = n;
                }
                i += 2;
                continue;
            }
        } else if arg == "--json" {
            *as_json = true;
            i += 1;
            continue;
        } else if let Some(rest) = arg.strip_prefix("--project-path=") {
            *project_path = Some(rest.to_owned());
            i += 1;
            continue;
        } else if let Some(rest) = arg.strip_prefix("--timeout=") {
            if let Ok(n) = rest.parse::<u64>() {
                *timeout_seconds = n;
            }
            i += 1;
            continue;
        }
        out.push(arg.clone());
        i += 1;
    }
    out
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
        let name = cmd.name.as_deref().unwrap_or("<unnamed>");
        let inline = cmd
            .parameters
            .iter()
            .filter_map(|p| p.name.as_deref().map(|n| format!("--{n}{}", if p.required { "" } else { "?" })))
            .collect::<Vec<_>>()
            .join(" ");
        let inline = if inline.is_empty() { String::new() } else { format!("  {inline}") };
        println!("  {:>width$}. {name}{inline}", idx + 1, width = width);
        if let Some(desc) = cmd.description.as_deref() {
            if !desc.is_empty() {
                println!("     {desc}");
            }
        }
        for param in &cmd.parameters {
            let Some(pname) = param.name.as_deref() else { continue };
            let ty = param.ty.as_deref().unwrap_or("string");
            let req = if param.required { " (required)" } else { "" };
            let default = param
                .default_value
                .as_ref()
                .map(|v| format!(" [default: {}]", stringify_value(v)))
                .unwrap_or_default();
            let desc = param.description.as_deref().unwrap_or(ty);
            println!("       --{pname}: {desc}{req}{default}");
        }
    }
    println!("{}", t("command.usage"));
    println!("{}", t("command.example"));
    if let Some(sample) = cmds.iter().find(|c| !c.parameters.is_empty()) {
        if let (Some(cname), Some(pname)) = (
            sample.name.as_deref(),
            sample.parameters.first().and_then(|p| p.name.as_deref()),
        ) {
            println!("{}", tr("command.example_with_param", &[&cname, &pname]));
        }
    }
    Ok(())
}

fn run_command(
    target: &Target,
    name: &str,
    args: &[String],
    timeout_seconds: u64,
    as_json: bool,
) -> Result<()> {
    let schema = fetch_commands(target, CATALOG_TIMEOUT)
        .ok()
        .and_then(|c| c.commands.into_iter().find(|c| c.name.as_deref() == Some(name)));
    let parameters = parse_command_arguments(args, schema.as_ref());

    if let Some(cmd) = schema.as_ref() {
        if let Some(unknown) = unknown_parameter(cmd, &parameters) {
            let msg = tr("command.invalid_parameter", &[&name, &unknown]);
            if !as_json {
                eprintln!("❌ {msg}");
                eprintln!("{}", t("command.parameter_docs"));
                eprintln!("{}", format_schema_hint(cmd));
            }
            return Err(anyhow!("{}", msg));
        }
    }

    let params_value = Value::Object(parameters.clone());

    // `--timeout` bounds the command itself, not just this process's wait: the server aborts a
    // main-thread command once the budget in the request is exceeded (60s if none is sent), so
    // passing it through is what makes an explicitly raised timeout mean anything.
    let timeout = Duration::from_secs(timeout_seconds.max(1));
    let result = match execute(target, name, &params_value, ExecBudget::shared(timeout)) {
        Ok(v) => v,
        Err(err) => {
            if !as_json {
                eprintln!("{}", tr("command.execute_failed", &[&name]));
                eprintln!("   {err:#}");
                if let Some(note) = pipeline::host_note(target, &err) {
                    eprintln!("⚠️  {note}");
                }
                if let Some(cmd) = schema.as_ref() {
                    let msg = err.to_string();
                    if msg.contains("Parameter") || msg.contains("argument") {
                        eprintln!("{}", t("command.parameter_docs"));
                        eprintln!("{}", format_schema_hint(cmd));
                    }
                }
            }
            return Err(err);
        }
    };

    if matches!(name, "eval" | "eval_file") && is_failed_eval_envelope(&result) {
        return Err(anyhow!(
            "eval failed:\n{}",
            crate::json::pretty(&result)
        ));
    }

    if as_json {
        let envelope = json!({
            "command": format!("command {name}"),
            "success": true,
            "target": {
                "host": "127.0.0.1",
                "port": target.port,
                "projectPath": target.project_path.to_string_lossy(),
            },
            "parameters": params_value,
            "result": result,
        });
        println!("{}", crate::json::pretty(&envelope));
        return Ok(());
    }

    println!("{}", tr("command.executed", &[&name]));
    println!("🔗 127.0.0.1:{}", target.port);
    if !parameters.is_empty() {
        let pretty = crate::json::compact(&Value::Object(parameters.clone()));
        println!("{}", tr("command.parameters_line", &[&pretty]));
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

/// Port of the JS `parseCommandArguments` — accepts `--flag value`, `--flag=value`,
/// bare `--flag` booleans, and remaps positional args onto the schema's required
/// parameters (ordered by `order`).
///
/// Values stay strings: the command's real parameter types live on the server, which converts
/// against them. Guessing here would silently change what the caller typed (`--name 007` must
/// stay "007", not become the number 7).
fn parse_command_arguments(args: &[String], schema: Option<&PipelineCommand>) -> Map<String, Value> {
    let mut parameters: Map<String, Value> = Map::new();
    let mut positionals: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some(rest) = arg.strip_prefix("--") {
            if let Some((k, v)) = rest.split_once('=') {
                if !k.is_empty() {
                    parameters.insert(k.to_owned(), Value::String(v.to_owned()));
                }
            } else {
                let next = args.get(i + 1);
                match next {
                    Some(n) if !n.starts_with("--") => {
                        parameters.insert(rest.to_owned(), Value::String(n.clone()));
                        i += 1;
                    }
                    _ => {
                        // A bare flag means "true" — the server's own command-line binder
                        // reads it the same way.
                        parameters.insert(rest.to_owned(), Value::Bool(true));
                    }
                }
            }
        } else {
            positionals.push(arg.clone());
        }
        i += 1;
    }

    if !positionals.is_empty() {
        map_positionals(&positionals, &mut parameters, schema);
    }
    parameters
}

/// The first supplied parameter the command does not declare, or None when every name is known.
/// Catching this locally turns a typo into a precise message instead of the server's
/// "required parameter missing".
fn unknown_parameter(cmd: &PipelineCommand, parameters: &Map<String, Value>) -> Option<String> {
    let known: Vec<&str> = cmd
        .parameters
        .iter()
        .filter_map(|p| p.name.as_deref())
        .collect();
    parameters
        .keys()
        .find(|k| !known.contains(&k.as_str()))
        .cloned()
}

fn map_positionals(
    positionals: &[String],
    parameters: &mut Map<String, Value>,
    schema: Option<&PipelineCommand>,
) {
    if let Some(schema) = schema {
        let mut ordered: Vec<&crate::pipeline::PipelineParam> = schema
            .parameters
            .iter()
            .filter(|p| p.required && p.name.is_some())
            .collect();
        ordered.sort_by_key(|p| p.order.unwrap_or(0));
        for (i, p) in ordered.iter().enumerate() {
            if i >= positionals.len() {
                break;
            }
            if let Some(name) = &p.name {
                parameters.insert(name.clone(), parse_value(&positionals[i]));
            }
        }
        return;
    }

    // No schema: bind first positional to the best-guess primary field.
    let first_name = first_parameter_name(parameters);
    parameters.insert(first_name, parse_value(&positionals[0]));
    for (i, val) in positionals.iter().enumerate().skip(1) {
        parameters.insert(format!("arg{i}"), parse_value(val));
    }
}

fn first_parameter_name(existing: &Map<String, Value>) -> String {
    for name in ["message", "text", "content", "value", "input", "data"] {
        if !existing.contains_key(name) {
            return name.to_owned();
        }
    }
    "arg0".to_owned()
}

/// Positional arguments are forwarded as-is; the server converts them against the command's
/// declared parameter types.
fn parse_value(v: &str) -> Value {
    Value::String(v.to_owned())
}

fn stringify_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => crate::json::compact(other),
    }
}

fn command_to_json(cmd: &PipelineCommand) -> Value {
    let params: Vec<Value> = cmd
        .parameters
        .iter()
        .map(|p| {
            let mut obj = Map::new();
            if let Some(n) = &p.name {
                obj.insert("name".to_owned(), Value::String(n.clone()));
            }
            if let Some(ty) = &p.ty {
                obj.insert("type".to_owned(), Value::String(ty.clone()));
                obj.insert(
                    "jsonType".to_owned(),
                    Value::String(pipeline::pipeline_type_to_json_schema(Some(ty)).to_owned()),
                );
            }
            if let Some(d) = &p.description {
                obj.insert("description".to_owned(), Value::String(d.clone()));
            }
            obj.insert("required".to_owned(), Value::Bool(p.required));
            if let Some(dv) = &p.default_value {
                obj.insert("defaultValue".to_owned(), dv.clone());
            }
            if let Some(o) = p.order {
                obj.insert("order".to_owned(), Value::from(o));
            }
            Value::Object(obj)
        })
        .collect();
    let mut obj = Map::new();
    if let Some(n) = &cmd.name {
        obj.insert("name".to_owned(), Value::String(n.clone()));
    }
    if let Some(d) = &cmd.description {
        obj.insert("description".to_owned(), Value::String(d.clone()));
    }
    obj.insert("parameters".to_owned(), Value::Array(params));
    Value::Object(obj)
}

fn format_schema_hint(cmd: &PipelineCommand) -> String {
    let mut out = String::new();
    let name = cmd.name.as_deref().unwrap_or("<unnamed>");
    out.push_str(&format!("  {name}\n"));
    if let Some(desc) = cmd.description.as_deref() {
        if !desc.is_empty() {
            out.push_str(&format!("    {desc}\n"));
        }
    }
    for p in &cmd.parameters {
        let Some(pname) = p.name.as_deref() else { continue };
        let ty = p.ty.as_deref().unwrap_or("string");
        let req = if p.required { " (required)" } else { "" };
        let default = p
            .default_value
            .as_ref()
            .map(|v| format!(" [default: {}]", stringify_value(v)))
            .unwrap_or_default();
        let desc = p.description.as_deref().unwrap_or(ty);
        out.push_str(&format!("      --{pname}: {desc}{req}{default}\n"));
    }
    out
}
