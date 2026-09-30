//! 单元测试本体，由 `src/upgrade.rs` 末尾的
//! `#[cfg(test)] #[path = "../tests/upgrade.rs"] mod tests;` 挂载为子模块
//! （子模块因此能访问父模块私有项）。Cargo.toml 已设 `autotests = false`，
//! cargo 不会把本文件误当独立集成测试编译。

use super::*;

#[test]
fn is_newer_compares_semver() {
    assert!(is_newer("v0.2.0", "0.1.2").unwrap());
    assert!(is_newer("v0.2.0-beta.1", "0.1.2").unwrap());
    // 开发版（CI 手动触发的 0.0.0-dev.N）对正式版总是可更新。
    assert!(is_newer("v0.1.2", "0.0.0-dev.7").unwrap());
    assert!(!is_newer("v0.1.2", "0.1.2").unwrap());
    assert!(!is_newer("v0.1.1", "0.1.2").unwrap());
    assert!(is_newer("not-a-version", "0.1.2").is_err());
}

#[test]
fn pick_asset_matches_platform_suffix() {
    let assets: Vec<Asset> = [
        "unity_pipeline_cli-0.1.2-windows-x86_64.tar.gz",
        "unity_pipeline_cli-0.1.2-macos-x86_64.tar.gz",
        "unity_pipeline_cli-0.1.2-macos-arm64.tar.gz",
        "unity_pipeline_cli-0.1.2-macos-universal.tar.gz",
    ]
    .into_iter()
    .map(|name| Asset {
        name: name.to_owned(),
        browser_download_url: String::new(),
    })
    .collect();
    let name = |a: Option<&Asset>| a.map(|a| a.name.clone());
    assert_eq!(
        name(pick_asset(&assets, "windows-x86_64")),
        Some("unity_pipeline_cli-0.1.2-windows-x86_64.tar.gz".to_owned())
    );
    assert_eq!(
        name(pick_asset(&assets, "macos-arm64")),
        Some("unity_pipeline_cli-0.1.2-macos-arm64.tar.gz".to_owned())
    );
    assert!(pick_asset(&assets, "linux-x86_64").is_none());
}
