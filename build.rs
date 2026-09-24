use std::fs;
use std::path::PathBuf;

/// 补丁集选择规则：(集合目录名, 生效起始版本 `from`)。
///
/// 选择逻辑（install.rs `select_patch_set`）：取所有 `from <= 包版本` 的规则中
/// `from` 最大者；无一命中则用 [`DEFAULT_PATCH_SET`]。比较遵循 semver prerelease
/// 语义（`0.7.0-exp.1 < 0.7.0`），因此 `from = 0.7.0-exp.1` 表示 0.7.0 正式版
/// 及其后的所有版本（含 prerelease）都落入该集合。
///
/// 背景：0.7.0-exp.1 起上游把 Runtime/Plugins/CodeAnalysis 下的 Roslyn DLL 及其
/// .meta 全部加了 `UnityPipeline.` 前缀（RenameAsm 改写，避免与编辑器自带副本
/// 冲突），.meta 旧状态与旧版本不再互相匹配，故按包版本拆分补丁集；后续出现
/// 不兼容漂移时照此办理——新增一个集合目录 + 在此追加一行规则。
const PATCH_SET_RULES: &[(&str, &str)] = &[("v0.7", "0.7.0-exp.1")];

/// 没有任何规则命中时使用的补丁集（最老的集合）。
const DEFAULT_PATCH_SET: &str = "legacy";

/// Embed every `patches/<set>/*.patch` as `EMBEDDED_PATCHES: &[(&str, &str,
/// &[u8])]`（补丁集名, 文件名, 内容），连同选择规则一起内嵌，release exe 保持
/// 自包含。新增 patch = 丢进某个集合目录后重编；新增补丁集 = 新建目录 +
/// （按版本生效时）在 `PATCH_SET_RULES` 加一行。
fn main() {
    println!("cargo:rerun-if-changed=patches");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let patches_dir = PathBuf::from(&manifest).join("patches");

    // 收集 patches/<set>/*.patch → (set, file) 对。
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    let mut set_names: Vec<String> = Vec::new();
    for entry in fs::read_dir(&patches_dir).expect("patches 目录不存在") {
        let path = entry.expect("读取 patches 子项失败").path();
        if !path.is_dir() {
            continue;
        }
        let set = path.file_name().unwrap().to_str().unwrap().to_owned();
        let mut files: Vec<PathBuf> = fs::read_dir(&path)
            .unwrap_or_else(|e| panic!("读取补丁集 patches/{set} 失败：{e}"))
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map_or(false, |e| e == "patch"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "补丁集 patches/{set}/ 下没有任何 .patch 文件");
        set_names.push(set.clone());
        for f in files {
            entries.push((set.clone(), f));
        }
    }
    assert!(!set_names.is_empty(), "patches/ 下没有任何补丁集目录");
    set_names.sort();
    entries.sort();

    // 编译期一致性：规则与默认集合必须指向真实存在的目录。
    for (rule_set, from) in PATCH_SET_RULES {
        assert!(
            set_names.iter().any(|s| s.as_str() == *rule_set),
            "PATCH_SET_RULES 引用了不存在的补丁集 `{rule_set}`（from={from}，现有：{set_names:?}）"
        );
    }
    assert!(
        set_names.iter().any(|s| s.as_str() == DEFAULT_PATCH_SET),
        "默认补丁集 `{DEFAULT_PATCH_SET}` 目录不存在（现有：{set_names:?}）"
    );

    let mut out = String::from("pub static EMBEDDED_PATCHES: &[(&str, &str, &[u8])] = &[\n");
    for (set, f) in &entries {
        let name = f.file_name().unwrap().to_str().unwrap();
        if name.starts_with("modify_unity_local_") {
            println!(
                "cargo:warning=跳过 {set}/{name}：modify_unity_local_* 属于运行时 patchs/custom/，不内嵌"
            );
            continue;
        }
        let path = f.to_str().unwrap().replace('\\', "/");
        out.push_str(&format!("    (\"{set}\", \"{name}\", include_bytes!(\"{path}\")),\n"));
    }
    out.push_str("];\n");
    out.push_str("pub static PATCH_SET_RULES: &[(&str, &str)] = &[\n");
    for (set, from) in PATCH_SET_RULES {
        out.push_str(&format!("    (\"{set}\", \"{from}\"),\n"));
    }
    out.push_str("];\n");
    out.push_str(&format!(
        "pub static DEFAULT_PATCH_SET: &str = \"{DEFAULT_PATCH_SET}\";\n"
    ));

    let out_path = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("embedded_patches.rs");
    fs::write(out_path, out).unwrap();
}
