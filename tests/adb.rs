//! 单元测试本体，由 `src/adb.rs` 末尾的
//! `#[cfg(test)] #[path = "../tests/adb.rs"] mod tests;` 挂载为子模块
//! （子模块因此能访问父模块私有项）。Cargo.toml 已设 `autotests = false`，
//! cargo 不会把本文件误当独立集成测试编译。

use super::*;

/// `adb devices` 解析只收 "device" 状态的行：offline / unauthorized / 空行 / 表头都跳过。
#[test]
fn parses_adb_devices_only_ready_state() {
    let out = "List of devices attached\n\
               R58MB1234\tdevice\n\
               emulator-5554\tdevice\n\
               ABC123\toffline\n\
               DEF456 unauthorized\n\
               \n";
    assert_eq!(parse_adb_devices(out), vec!["R58MB1234", "emulator-5554"]);
}

/// 空输出（无设备）返回空表，不 panic。
#[test]
fn parses_empty_adb_devices() {
    assert!(parse_adb_devices("List of devices attached\n").is_empty());
    assert!(parse_adb_devices("").is_empty());
}

/// `adb forward --list` 行形如 `<serial> tcp:<local> tcp:<device>`；
/// 复用已有 forward 的逻辑要按设备端口（第 3 列）匹配、取本地端口（第 2 列）。
#[test]
fn existing_forward_reuse_is_by_device_port() {
    // 逻辑内联在 ensure_forward（需真实 adb），此处钉住它依赖的字符串约定。
    let line = "R58MB1234 tcp:7921 tcp:7903";
    let tokens: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(tokens[2], "tcp:7903"); // 设备端口列
    assert_eq!(tokens[1].strip_prefix("tcp:").unwrap(), "7921"); // 本地端口列
}
