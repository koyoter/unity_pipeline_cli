use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

/// `patches/manifest.json` 里一个补丁集（版本档）的定义。
#[derive(Deserialize)]
struct PatchSetDef {
    #[allow(dead_code)]
    from: String,
    patches: Vec<String>,
}

/// 内嵌补丁资源：patches/ 是扁平布局，一个上游目标文件对应一个补丁文件，
/// 命名 `<目标路径编码>@<自增长序号>.patch`——同一文件内容漂移则序号 +1，
/// 内容相同的补丁集共享同一文件。哪个补丁集从哪个包版本（`from`，semver
/// prerelease 语义）生效、应用哪些文件（数组顺序即应用顺序），全部记录在
/// patches/manifest.json，原样内嵌为 MANIFEST_JSON；选择逻辑在 install.rs。
/// 新增补丁 = 丢进 patches/ 并登记进 manifest；新增版本档 = manifest 加 key。
/// 编译期校验 manifest ↔ 补丁文件一一对应，防止两处漂移。
fn main() {
    println!("cargo:rerun-if-changed=patches");
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let patches_dir = PathBuf::from(&manifest_dir).join("patches");
    let manifest_path = patches_dir.join("manifest.json");
    let manifest_raw = fs::read_to_string(&manifest_path).expect("读取 patches/manifest.json 失败");
    let manifest: BTreeMap<String, PatchSetDef> =
        serde_json::from_str(&manifest_raw).expect("patches/manifest.json 不是合法的补丁集清单");

    let mut files: Vec<PathBuf> = fs::read_dir(&patches_dir)
        .expect("patches 目录不存在")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map_or(false, |e| e == "patch"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "patches/ 下没有任何 .patch 文件");
    let names: BTreeSet<&str> = files
        .iter()
        .map(|f| f.file_name().unwrap().to_str().unwrap())
        .collect();

    let mut referenced: BTreeSet<&str> = BTreeSet::new();
    for (set, def) in &manifest {
        let mut seen_in_set = BTreeSet::new();
        assert!(!def.patches.is_empty(), "补丁集 {set} 的补丁清单为空");
        for name in &def.patches {
            assert!(
                names.contains(name.as_str()),
                "补丁集 {set} 引用了不存在的补丁文件 `{name}`"
            );
            assert!(
                seen_in_set.insert(name.as_str()),
                "补丁集 {set} 重复登记 `{name}`——同一文件会被应用两次"
            );
            referenced.insert(name.as_str());
        }
    }
    for name in &names {
        assert!(
            referenced.contains(name),
            "孤儿补丁文件 `{name}`：未在 patches/manifest.json 任何补丁集里登记"
        );
    }

    let mut out = String::from("pub static EMBEDDED_PATCHES: &[(&str, &[u8])] = &[\n");
    for f in &files {
        let name = f.file_name().unwrap().to_str().unwrap();
        let path = f.to_str().unwrap().replace('\\', "/");
        out.push_str(&format!("    (\"{name}\", include_bytes!(\"{path}\")),\n"));
    }
    out.push_str("];\n");
    let manifest_include = manifest_path.to_str().unwrap().replace('\\', "/");
    out.push_str(&format!(
        "pub static MANIFEST_JSON: &str = include_str!(\"{manifest_include}\");\n"
    ));

    // shim 嵌入：shim/ 下包目录整树 include_bytes（键为 shim/ 相对路径）。
    // 垫片是我们自己的产物，离线可装、版本锁定；编译期校验包名与 asmdef 名。
    println!("cargo:rerun-if-changed=shim");
    let shim_root = PathBuf::from(&manifest_dir).join("shim");
    let mut shim_files: Vec<PathBuf> = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(dir).expect("shim 目录读取失败").flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    walk(&shim_root, &mut shim_files);
    shim_files.sort();
    assert!(!shim_files.is_empty(), "shim/ 下没有任何文件：垫片包缺失");
    let shim_pkg_dir = shim_root.join("cn.tuanjie.codely.bridge.shim");
    let shim_pkg: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(shim_pkg_dir.join("package.json"))
            .expect("读取 shim package.json 失败"),
    )
    .expect("shim package.json 不是合法 JSON");
    assert!(
        shim_pkg["name"] == "cn.tuanjie.codely.bridge.shim",
        "shim 包名漂移，安装侧按此名复制"
    );
    let shim_asmdef_raw =
        fs::read_to_string(shim_pkg_dir.join("UnityTcp.Editor.asmdef"))
            .expect("读取 shim asmdef 失败");
    assert!(
        shim_asmdef_raw.contains("\"name\": \"UnityTcp.Editor\""),
        "shim asmdef 名必须逐字为 UnityTcp.Editor（助手包按名解析）"
    );
    out.push_str("pub static EMBEDDED_SHIM: &[(&str, &[u8])] = &[\n");
    for f in &shim_files {
        let rel = f
            .strip_prefix(&shim_root)
            .unwrap()
            .to_str()
            .unwrap()
            .replace('\\', "/");
        let path = f.to_str().unwrap().replace('\\', "/");
        out.push_str(&format!("    (\"{rel}\", include_bytes!(\"{path}\")),\n"));
    }
    out.push_str("];\n");

    let out_path = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("embedded_patches.rs");
    fs::write(out_path, out).unwrap();
}
