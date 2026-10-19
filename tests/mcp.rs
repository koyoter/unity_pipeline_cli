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
