//! 单元测试本体，由 `src/command.rs` 末尾的
//! `#[cfg(test)] #[path = "../tests/command.rs"] mod tests;` 挂载为子模块
//! （子模块因此能访问父模块私有项）。Cargo.toml 已设 `autotests = false`，
//! cargo 不会把本文件误当独立集成测试编译。

use super::*;

/// None / 空串 / "null" 都是「无参数」，统一成空对象原样转发。
#[test]
fn parse_params_accepts_absent_empty_and_null() {
    assert_eq!(parse_params(None).unwrap(), json!({}));
    assert_eq!(parse_params(Some("")).unwrap(), json!({}));
    assert_eq!(parse_params(Some("   ")).unwrap(), json!({}));
    assert_eq!(parse_params(Some("null")).unwrap(), json!({}));
}

/// JSON 对象原样转发，不做任何本地改写——服务端才是绑定参数类型的地方。
#[test]
fn parse_params_forwards_object_verbatim() {
    let raw = r#"{"path":"Assets/New.mat","confirm":true,"n":3}"#;
    assert_eq!(
        parse_params(Some(raw)).unwrap(),
        json!({"path":"Assets/New.mat","confirm":true,"n":3})
    );
}

/// 非对象（数组 / 字符串 / 数字）与坏 JSON 都是明确报错，不是静默吞掉。
#[test]
fn parse_params_rejects_non_object_and_bad_json() {
    assert!(parse_params(Some("[1,2]")).is_err());
    assert!(parse_params(Some("\"x\"")).is_err());
    assert!(parse_params(Some("42")).is_err());
    assert!(parse_params(Some("{oops}")).is_err());
}
