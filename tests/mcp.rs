//! 单元测试本体，由 `src/mcp.rs` 末尾的
//! `#[cfg(test)] #[path = "../tests/mcp.rs"] mod tests;` 挂载为子模块
//! （子模块因此能访问父模块私有项）。Cargo.toml 已设 `autotests = false`，
//! cargo 不会把本文件误当独立集成测试编译。

use super::*;

/// 已经成功下发过工具表后，刷新失败（Editor 重启、token 轮换、暂时不可达）必须继续
/// 回最后一份工具表：回空表会让宿主注销全部工具，模型侧表现为 "MCP tool is not found"，
/// 且宿主不会自己重新 tools/list。
#[test]
fn failed_refresh_serves_last_known_catalog() {
    let missing_project = std::env::temp_dir().join("unity-pipeline-mcp-test-missing");
    let opts = Options {
        project_path: Some(missing_project.to_string_lossy().into_owned()),
    };
    let state = Mutex::new(None);
    let known = Mutex::new(None);
    let last = Mutex::new(Some(vec![json!({
        "name": "eval",
        "description": "cached",
        "inputSchema": { "type": "object" }
    })]));

    let (response, changed) = handle_tools_list(json!(7), &state, &opts, &known, &last);

    assert!(!changed);
    assert_eq!(response["result"]["tools"][0]["name"], "eval");
}

/// 从未成功拿到过工具表时（Unity 还没起来）保持原有握手语义：回空表让客户端完成
/// initialize + tools/list，而不是报协议错误。
#[test]
fn failed_refresh_without_cache_stays_empty() {
    let missing_project = std::env::temp_dir().join("unity-pipeline-mcp-test-missing");
    let opts = Options {
        project_path: Some(missing_project.to_string_lossy().into_owned()),
    };
    let state = Mutex::new(None);
    let known = Mutex::new(None);
    let last = Mutex::new(None);

    let (response, changed) = handle_tools_list(json!(8), &state, &opts, &known, &last);

    assert!(!changed);
    assert_eq!(response["result"]["tools"].as_array().unwrap().len(), 0);
}

/// 日志时间戳必须能和 Unity / 宿主日志直接对齐；用两个公认的 epoch 值钉住换算。
#[test]
fn iso8601_matches_known_epochs() {
    assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(iso8601_utc(1_700_000_000_000), "2023-11-14T22:13:20.000Z");
    assert_eq!(iso8601_utc(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
}

/// token 原文不能进日志，但不同 token 的指纹必须可区分。
#[test]
fn token_fingerprint_is_short_and_not_the_token() {
    let token = "super-secret-token";
    let fingerprint = token_fingerprint(Some(token));
    assert_eq!(fingerprint.len(), 8);
    assert_ne!(fingerprint, token);
    assert_ne!(
        fingerprint,
        token_fingerprint(Some("another-token"))
    );
    assert_eq!(token_fingerprint(None), "none");
    assert_eq!(token_fingerprint(Some("")), "none");
}

/// 0.9 分组目录：`command` 是带空格的全名，工具名取它并把空格换成点号——点号形式
/// 服务端接受（live 0.9 实测过）、跨组唯一。裸 `name`（"create"）会跨组撞车，绝不能用。
#[test]
fn tool_name_dots_the_grouped_full_name() {
    let cmd = pipeline::PipelineCommand {
        command: Some("asset create".to_owned()),
        name: Some("create".to_owned()),
        ..empty_cmd()
    };
    assert_eq!(pipeline::tool_name(&cmd).as_deref(), Some("asset.create"));

    // 多级组：点号本来就存在，只有组分隔空格被替换。
    let nested = pipeline::PipelineCommand {
        command: Some("animation.animator add_state".to_owned()),
        name: Some("add_state".to_owned()),
        ..empty_cmd()
    };
    assert_eq!(
        pipeline::tool_name(&nested).as_deref(),
        Some("animation.animator.add_state")
    );

    // 无组命令的全名没有空格，原样即点号形式。
    let bare = pipeline::PipelineCommand {
        command: Some("ping".to_owned()),
        name: Some("ping".to_owned()),
        ..empty_cmd()
    };
    assert_eq!(pipeline::tool_name(&bare).as_deref(), Some("ping"));
}

/// 0.8 及更老的目录没有 `command` 字段，`name` 就是完整扁平名（如 run_tests），
/// 原样转发、不做任何转换。
#[test]
fn tool_name_falls_back_to_flat_name_for_pre_group_catalogs() {
    let cmd = pipeline::PipelineCommand {
        command: None,
        name: Some("run_tests".to_owned()),
        ..empty_cmd()
    };
    assert_eq!(pipeline::tool_name(&cmd).as_deref(), Some("run_tests"));

    let unnamed = pipeline::PipelineCommand {
        command: None,
        name: None,
        ..empty_cmd()
    };
    assert!(pipeline::tool_name(&unnamed).is_none());
}

fn empty_cmd() -> pipeline::PipelineCommand {
    pipeline::PipelineCommand {
        command: None,
        name: None,
        description: None,
        parameters: Vec::new(),
        schema: None,
    }
}

/// 预算只看参数形状：显式 `timeout`（秒）优先，`timeout_s`（0.9 前的拼写）兜底，
/// 都没有时用 schema 里声明的 default，最后才是 60s 客户端默认。不认识任何命令名。
#[test]
fn tool_budget_reads_timeout_shape_only() {
    // 显式 timeout。
    let budget = tool_budget(&json!({"timeout": 300}), None);
    assert_eq!(budget.server_ms, Some(300_000));

    // 0.9 前的拼写 timeout_s（wait_for）。
    let budget = tool_budget(&json!({"timeout_s": 120}), None);
    assert_eq!(budget.server_ms, Some(120_000));

    // 调用方没传，但命令 schema 声明了 default（test run 默认 600）。
    let schema = json!({
        "type": "object",
        "properties": { "timeout": { "type": "number", "default": 600 } }
    });
    let budget = tool_budget(&json!({}), Some(&schema));
    assert_eq!(budget.server_ms, Some(600_000));

    // 什么都没有：60s 默认。
    let budget = tool_budget(&json!({}), None);
    assert_eq!(budget.server_ms, Some(60_000));

    // 非正数忽略，避免把垃圾值放大成无限等待。
    let budget = tool_budget(&json!({"timeout": 0}), None);
    assert_eq!(budget.server_ms, Some(60_000));
}

/// 成图与失败标记都按结果形状触发，与命令名无关：任何命令返回 encoding=png+base64
/// 都要变成 image content（宿主不会渲染 JSON 文本里的 base64），任何 success:false
/// 信封都要标 isError。
#[test]
fn shape_tool_result_triggers_on_shape_alone() {
    // PNG 形状 → image content。
    let shaped = shape_tool_result(
        &json!({
            "encoding": "png",
            "base64": "aW1n",
            "savedPath": "Screenshots/foo.png",
        }),
        None,
    );
    assert_eq!(shaped["content"][0]["type"], "image");
    assert_eq!(shaped["content"][0]["data"], "aW1n");
    assert_eq!(shaped["content"][0]["mimeType"], "image/png");
    assert_eq!(shaped["content"][1]["type"], "text");

    // success:false 信封 → isError。
    let failed = shape_tool_result(&json!({"success": false, "error": "boom"}), None);
    assert_eq!(failed["isError"], true);

    // 普通结果原样文本，不带 isError。
    let plain = shape_tool_result(&json!({"answer": 42}), None);
    assert_eq!(plain["content"][0]["type"], "text");
    assert!(plain.get("isError").is_none());
}
