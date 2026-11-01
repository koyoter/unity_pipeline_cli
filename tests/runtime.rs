//! 单元测试本体，由 `src/runtime.rs` 末尾的
//! `#[cfg(test)] #[path = "../tests/runtime.rs"] mod tests;` 挂载为子模块
//! （子模块因此能访问父模块私有项）。Cargo.toml 已设 `autotests = false`，
//! cargo 不会把本文件误当独立集成测试编译。

use super::*;

/// 描述符样例（字段名来自包内 RuntimeInstanceDescriptor.cs 的 JsonProperty），
/// evalToken / workingDirectory 等 camelCase 键必须经 rename 正确落到 snake_case 字段。
#[test]
fn parses_descriptor_fields() {
    let raw = r#"{
        "pid": 4242,
        "port": 7901,
        "platform": "WindowsPlayer",
        "unityVersion": "6000.2.1f1",
        "buildGuid": "abc-123",
        "startedAt": "2026-11-01T10:00:00Z",
        "lastHeartbeat": "2026-11-01T10:00:05Z",
        "workingDirectory": "C:\\Builds\\MyGame",
        "evalToken": "SECRET=="
    }"#;
    let desc: RuntimeDescriptor = serde_json::from_str(raw).unwrap();
    assert_eq!(desc.port, 7901);
    assert_eq!(desc.eval_token.as_deref(), Some("SECRET=="));
    assert_eq!(desc.pid, Some(4242));
    assert_eq!(desc.working_directory.as_deref(), Some("C:\\Builds\\MyGame"));
}

/// 端口非法（<1）的描述符拒绝解析；坏 JSON 报错而不是 panic。
#[test]
fn rejects_invalid_descriptor() {
    let dir = std::env::temp_dir().join(format!("unity-pipeline-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(RUNTIME_DESCRIPTOR_FILE), r#"{"port":0}"#).unwrap();
    assert!(read_runtime_descriptor(&dir).is_err());

    fs::write(dir.join(RUNTIME_DESCRIPTOR_FILE), "{oops").unwrap();
    assert!(read_runtime_descriptor(&dir).is_err());

    fs::remove_dir_all(&dir).ok();
}

/// app.info 是 `Company\nProductName` 两行文本，容忍首尾空行与行内空白。
#[test]
fn parses_app_info() {
    assert_eq!(
        parse_app_info("Acme Corp\nMy Game\n"),
        Some(("Acme Corp".into(), "My Game".into()))
    );
    assert_eq!(
        parse_app_info("\n  Acme  \n  MyGame  \n\n"),
        Some(("Acme".into(), "MyGame".into()))
    );
    assert_eq!(parse_app_info("only-one-line"), None);
    assert_eq!(parse_app_info(""), None);
}
