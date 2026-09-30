//! 单元测试本体，由 `src/i18n.rs` 末尾的
//! `#[cfg(test)] #[path = "../tests/i18n.rs"] mod tests;` 挂载为子模块
//! （子模块因此能访问父模块私有项）。Cargo.toml 已设 `autotests = false`，
//! cargo 不会把本文件误当独立集成测试编译。

use super::*;

#[test]
fn locales_share_the_same_key_set() {
    let tb = tables();
    let mut en: Vec<_> = tb.en.keys().collect();
    let mut zh: Vec<_> = tb.zh.keys().collect();
    en.sort_unstable();
    zh.sort_unstable();
    assert_eq!(en, zh, "en/zh 键集合不一致");
}

#[test]
fn placeholders_are_substituted() {
    let out = tr("language.invalid", &[&"xx", &"en, zh"]);
    assert!(!out.contains('{'), "占位符未替换: {out}");
    assert!(out.contains("xx"));
}
