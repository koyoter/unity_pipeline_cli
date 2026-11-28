//! 单元测试本体，由 `src/configure.rs` 末尾的
//! `#[cfg(test)] #[path = "../tests/configure.rs"] mod tests;` 挂载为子模块
//! （子模块因此能访问父模块私有项）。Cargo.toml 已设 `autotests = false`，
//! cargo 不会把本文件误当独立集成测试编译。

use super::*;

/// 真实资产形状：PlayerSettings 根下两格缩进的字段、四格缩进的
/// applicationIdentifier 子键；Android 键命中，后置 buildNumber 块不可达。
#[test]
fn android_package_hits_within_block() {
    let yaml = "%YAML 1.1\n--- !u!129 &1\nPlayerSettings:\n  productName: My Game\n  applicationIdentifier:\n    Standalone: com.acme.desktop\n    Android: com.acme.mygame\n  buildNumber:\n    Standalone: 0\n    Android: 101\n";
    assert_eq!(
        android_package_from_asset(yaml).as_deref(),
        Some("com.acme.mygame")
    );
}

/// 回归：块被同级字段（如 buildNumber）终结后，其内层的 `Android:` 构建号
/// 不得被误认成包名——靠缩进回退判块结束，而非扫到文件尾。
#[test]
fn android_build_number_after_block_is_not_a_package() {
    let yaml = "PlayerSettings:\n  applicationIdentifier: {}\n  buildNumber:\n    Android: 101\n";
    assert_eq!(android_package_from_asset(yaml), None);
}

/// 从未切到 Android 平台（无 applicationIdentifier 块）或键值为空，诚实返回 None。
#[test]
fn no_android_target_returns_none() {
    assert_eq!(
        android_package_from_asset("PlayerSettings:\n  productName: Game\n"),
        None
    );
    assert_eq!(
        android_package_from_asset("applicationIdentifier:\n  Android:\n"),
        None
    );
}

/// 带引号的值剥引号；CRLF 行尾不影响解析。
#[test]
fn quoted_value_and_crlf_lines() {
    let yaml = "PlayerSettings:\r\n  applicationIdentifier:\r\n    Android: \"com.acme.mygame\"\r\n";
    assert_eq!(
        android_package_from_asset(yaml).as_deref(),
        Some("com.acme.mygame")
    );
}
