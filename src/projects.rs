use anyhow::Result;
use serde_json::json;

use crate::editor::{discover_editor_instances, EditorInstance};

pub fn run(as_json: bool) -> Result<()> {
    let instances = discover_editor_instances()?;

    if as_json {
        print_json(&instances);
    } else {
        print_human(&instances);
    }
    Ok(())
}

/// Base name of the currently-running executable, minus a `.exe` suffix. Falls
/// back to `"unity"` when Node/Bun/Deno wrappers report their own runtime name
/// (mirrors the JS `cliInvocationName`).
fn cli_invocation_name() -> String {
    let exe = std::env::current_exe().ok();
    let base = exe
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = base.trim_end_matches(".exe").trim_end_matches(".EXE");
    if stem.is_empty() {
        return "unity".to_owned();
    }
    let lower = stem.to_ascii_lowercase();
    if matches!(lower.as_str(), "node" | "bun" | "deno") {
        return "unity".to_owned();
    }
    stem.to_owned()
}

fn print_json(instances: &[EditorInstance]) {
    let data = instances
        .iter()
        .map(|inst| {
            let port = inst.descriptor.as_ref().map(|d| d.port);
            let api_url = port.map(|p| format!("http://127.0.0.1:{p}/api/editor_status"));
            json!({
                "projectName": inst.project_name,
                "projectPath": inst.project_path.to_string_lossy(),
                "pid": inst.pid,
                "isRunning": true,
                "unityVersion": inst.unity_version,
                "hasPipelinePackage": inst.has_pipeline,
                "pipelineVersion": inst.pipeline_version,
                "pipelineServer": {
                    "port": port,
                    "isReachable": inst.is_reachable,
                    "apiUrl": api_url,
                }
            })
        })
        .collect::<Vec<_>>();

    let running = instances.len();
    let with_pipeline = instances.iter().filter(|i| i.has_pipeline).count();
    let reachable = instances.iter().filter(|i| i.is_reachable).count();

    let envelope = json!({
        "command": "projects",
        "instances": data,
        "summary": {
            "totalInstances": instances.len(),
            "runningInstances": running,
            "instancesWithPipeline": with_pipeline,
            "reachableServers": reachable,
        }
    });
    println!("{}", crate::json::pretty(&envelope));
}

fn print_human(instances: &[EditorInstance]) {
    if instances.is_empty() {
        println!("没有打开的 Unity 项目");
        println!("提示：请先在 Unity Hub 或 Unity Editor 中打开一个项目，再运行该命令。");
        return;
    }

    println!("发现 {} 个正在运行的 Unity Editor：", instances.len());
    println!();
    for (i, inst) in instances.iter().enumerate() {
        let version = inst.unity_version.as_deref().unwrap_or("未知");
        println!(
            "{}. {} (PID {}, Unity {})",
            i + 1,
            inst.project_name,
            inst.pid,
            version
        );
        println!("   路径 : {}", inst.project_path.display());
        if inst.has_pipeline {
            let ver = inst.pipeline_version.as_deref().unwrap_or("");
            let suffix = if ver.is_empty() {
                String::new()
            } else {
                format!(" ({ver})")
            };
            println!("   Pipeline : 已安装{suffix}");
            match &inst.descriptor {
                Some(d) => {
                    let state = if inst.is_reachable {
                        "可连接"
                    } else {
                        "不可达"
                    };
                    // A batchmode Editor cannot show modal dialogs, so the distinction decides
                    // whether the guidance below can apply at all.
                    let mode = if d.mode.as_deref() == Some("batchmode") {
                        " · batchmode"
                    } else {
                        ""
                    };
                    println!("   服务 : {state}  (127.0.0.1:{}){mode}", d.port);
                    if let Some(info) = d.info.as_deref() {
                        println!("   提示 : {info}");
                    }
                }
                None => {
                    println!("   服务 : 未启动（Editor 尚未创建 .unity-pipeline-port 描述符）");
                }
            }
        } else {
            println!("   Pipeline : 未安装");
            let cli = cli_invocation_name();
            println!(
                "   提示 : 尚未安装 Pipeline 包，先运行 `{cli} install`（会列出正在运行的 Editor 供选择）"
            );
        }
        println!();
    }
}
