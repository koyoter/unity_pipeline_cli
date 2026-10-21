//! 单元测试本体，由 `src/install.rs` 末尾的
//! `#[cfg(test)] #[path = "../tests/install.rs"] mod tests;` 挂载为子模块
//! （子模块因此能访问父模块私有项）。Cargo.toml 已设 `autotests = false`，
//! cargo 不会把本文件误当独立集成测试编译。

use super::*;

/// `normalize_crlf` 只吃掉紧跟 `\n` 的 `\r`：整文件 CRLF 归一成 LF，孤立
/// 的 `\r`（内容本身的 CR）原样保留。
#[test]
fn normalize_crlf_strips_only_cr_before_lf() {
    assert_eq!(normalize_crlf(b"a\r\nb\n".to_vec()), b"a\nb\n".to_vec());
    assert_eq!(normalize_crlf(b"a\rb".to_vec()), b"a\rb".to_vec());
    assert_eq!(normalize_crlf(b"a\n\r".to_vec()), b"a\n\r".to_vec());
}

/// 回归：CRLF 补丁里无 tail 的 hunk header（`@@ -1,2 +1,2 @@\r\n`）会让
/// patchkit 报 MalformedHunkHeader，load_patch 归一后必须可正常解析。
/// 夹具用 `\n` 转义拼接而非字面换行，避免测试源码被 autocrlf 转换后失真。
#[test]
fn load_patch_accepts_crlf_patch_file() {
    let lf = concat!(
        "diff --git a/Editor/Foo.cs b/Editor/Foo.cs\n",
        "--- a/Editor/Foo.cs\n",
        "+++ b/Editor/Foo.cs\n",
        "@@ -1,2 +1,2 @@\n",
        " one\n",
        "-two\n",
        "+TWO\n",
    );
    let tmp = std::env::temp_dir().join(format!(
        "unity_pipeline_crlf_patch_test_{}.patch",
        std::process::id()
    ));
    std::fs::write(&tmp, lf.replace('\n', "\r\n")).unwrap();
    let patches = load_patch("custom/crlf.patch", &tmp).unwrap();
    let _ = std::fs::remove_file(&tmp);
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0].rel_path, PathBuf::from("Editor").join("Foo.cs"));
    assert_eq!(patches[0].hunks.len(), 1);
    assert_eq!(patches[0].hunks[0].lines.len(), 3);
}

/// 所有内嵌补丁必须能被 load_patch 完整解析。守住「重新生成补丁时混入
/// CRLF 或坏段」这类回归，不依赖本机有无包缓存。
#[test]
fn embedded_patches_all_parse() {
    assert!(!EMBEDDED_PATCHES.is_empty());
    let tmp = std::env::temp_dir().join(format!(
        "unity_pipeline_embedded_parse_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    for (name, bytes) in EMBEDDED_PATCHES {
        let path = tmp.join(name);
        std::fs::write(&path, bytes).unwrap();
        let patches = load_patch(&format!("system/{name}"), &path)
            .unwrap_or_else(|e| panic!("解析 {name} 失败：{e}"));
        assert!(!patches.is_empty(), "{name} 没有可用的文本段");
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Verify the pure-Rust sha1 matches a known digest so regressions in the
/// streaming buffer show up here rather than via a mismatched registry SHA.
#[test]
fn sha1_hex_matches_known_digest() {
    let tmp = std::env::temp_dir().join(format!(
        "unity_pipeline_sha1_test_{}.bin",
        std::process::id()
    ));
    std::fs::write(&tmp, b"abc").unwrap();
    let got = sha1_hex(&tmp).unwrap();
    let _ = std::fs::remove_file(&tmp);
    // sha1("abc") — the RFC 3174 canonical test vector.
    assert_eq!(got, "a9993e364706816aba3e25717850c26c9cd0d89d");
}

/// Verify `collect_patch_sources` ordering: system files follow the
/// manifest list order verbatim, then custom by numeric N (so `_10` comes
/// after `_2`), gaps allowed, non-conforming custom names skipped.
#[test]
fn collect_patch_sources_orders_system_then_custom() {
    let tmp = std::env::temp_dir().join(format!(
        "unity_pipeline_collect_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let custom = tmp.join(CUSTOM_DIR_NAME);
    std::fs::create_dir_all(&custom).unwrap();
    std::fs::write(custom.join("modify_unity_local_2.patch"), "").unwrap();
    std::fs::write(custom.join("modify_unity_local_10.patch"), "").unwrap();
    std::fs::write(custom.join("modify_unity_local_1.patch"), "").unwrap();
    std::fs::write(custom.join("ignored.patch"), "").unwrap();

    let sources =
        collect_patch_sources(&tmp, &["b.patch".to_owned(), "a.patch".to_owned()]).unwrap();
    let names: Vec<&str> = sources.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "system/b.patch",
            "system/a.patch",
            "custom/modify_unity_local_1.patch",
            "custom/modify_unity_local_2.patch",
            "custom/modify_unity_local_10.patch",
        ]
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Version → patch-set selection against the real embedded manifest:
/// cutoffs are prerelease-aware (`0.7.0-exp.1 < 0.7.0`), highest matching
/// `from` wins, older versions fall to `legacy` (from `0.0.0`), and
/// anything newer than the last cutoff sticks to it (0.10.0 → 0.9).
#[test]
fn select_patch_set_matches_version_cutoffs() {
    let manifest = parse_manifest().unwrap();
    let set = |v: &str| select_patch_set(&manifest, v).unwrap().0;
    assert_eq!(set("0.6.9"), "legacy");
    assert_eq!(set("0.6.10"), "legacy");
    // prerelease of the cutoff itself sorts below `from`
    assert_eq!(set("0.7.0-exp.0"), "legacy");
    assert_eq!(set("0.7.0-exp.1"), "0.7");
    assert_eq!(set("0.7.0"), "0.7");
    assert_eq!(set("0.8.0-exp.0"), "0.7");
    assert_eq!(set("0.8.0-exp.1"), "0.8");
    assert_eq!(set("0.8.0"), "0.8");
    assert_eq!(set("0.9.0-exp.0"), "0.8");
    assert_eq!(set("0.9.0-exp.1"), "0.9");
    assert_eq!(set("0.9.0"), "0.9");
    assert_eq!(set("0.10.0"), "0.9");
}

/// 最低版本门槛：0.6.0-exp.1 及以上保留，更低的（含 0.6.0-exp.0）和解析
/// 不了的版本全部过滤；保持传入顺序。
#[test]
fn filter_supported_versions_enforces_floor() {
    let got = filter_supported_versions(vec![
        "0.9.0".to_owned(),
        "0.6.0".to_owned(),
        "0.6.0-exp.1".to_owned(),
        "0.6.0-exp.0".to_owned(),
        "0.5.9".to_owned(),
        "garbage".to_owned(),
    ]);
    assert_eq!(got, vec!["0.9.0", "0.6.0", "0.6.0-exp.1"]);
}

/// Unparseable package versions abort instead of guessing a patch set.
#[test]
fn select_patch_set_rejects_unparseable_versions() {
    let manifest = parse_manifest().unwrap();
    assert!(select_patch_set(&manifest, "not-a-version").is_err());
    assert!(select_patch_set(&manifest, "").is_err());
}

#[test]
fn read_package_version_reads_package_json() {
    let tmp = std::env::temp_dir().join(format!(
        "unity_pipeline_pkgver_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(
        tmp.join("package.json"),
        r#"{"name":"com.unity.pipeline","version":"0.7.0-exp.1"}"#,
    )
    .unwrap();
    assert_eq!(read_package_version(&tmp).unwrap(), "0.7.0-exp.1");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// End-to-end sanity check: for every pristine package extraction cached
/// under `releases/downloads/` (or the tarball in `UNITY_TARBALL`), select
/// the patch set for its version, then parse + dry-run + apply those
/// embedded patches in-memory. Extractions already patched by a previous
/// install run are skipped (their old-state no longer matches any set);
/// the copy-out keeps the cache untouched either way. Skips silently when
/// no pristine package is available.
#[test]
fn embedded_patch_applies_to_cached_packages() {
    let tmp = std::env::temp_dir().join(format!(
        "unity_pipeline_patch_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // Candidate package roots: tarball extraction first, then every
    // already-extracted version in the project-local download cache.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(v) = std::env::var_os("UNITY_TARBALL") {
        let tarball = PathBuf::from(v);
        assert!(tarball.exists(), "tarball missing at {}", tarball.display());
        let status = Command::new("tar")
            .args(["-xzf", tarball.to_str().unwrap(), "-C", tmp.to_str().unwrap()])
            .status()
            .expect("tar 未启动");
        assert!(status.success(), "tar 解压失败");
        candidates.push(tmp.join("package"));
    } else {
        let downloads =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("releases/downloads");
        if let Ok(entries) = std::fs::read_dir(&downloads) {
            candidates.extend(
                entries
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.is_dir() && p.join("package").join("package.json").exists())
                    .map(|p| p.join("package")),
            );
        }
        candidates.sort();
    }
    if candidates.is_empty() {
        eprintln!("skip: no UNITY_TARBALL and no extracted package under releases/downloads");
        return;
    }
    let manifest = parse_manifest().unwrap();

    for src in candidates {
        // A previous install run patches the extraction in place, so a
        // dirty cache no longer matches any patch set's old-state.
        let asset_cs = src.join("Editor/Commands/Assets/AssetCommands.cs");
        if std::fs::read_to_string(&asset_cs)
            .map(|s| s.contains("UNITY_2022_3"))
            .unwrap_or(false)
        {
            eprintln!("skip dirty cache: {}", src.display());
            continue;
        }
        // 低于最低支持版本的缓存包不在补丁保障范围内（如 0.5.0-exp.1 连
        // PipelineAnalytics.cs 都没有），安装流程也已禁止选择，直接跳过。
        let package_version = read_package_version(&src).expect("读取包版本失败");
        if !is_supported_version(&package_version) {
            eprintln!(
                "skip below {MIN_PACKAGE_VERSION}: {package_version} ({})",
                src.display()
            );
            continue;
        }

        let dest = tmp.join(format!("case-{}", src.parent().unwrap().file_name().unwrap().to_string_lossy()));
        std::fs::create_dir_all(&dest).unwrap();
        copy_dir_recursive(&src, &dest).expect("复制已解压包失败");

        let (patch_set, patch_list) =
            select_patch_set(&manifest, &package_version).expect("选择补丁集失败");
        eprintln!("end-to-end against {package_version} → patch set {patch_set}");

        let mut patches = Vec::new();
        for name in patch_list {
            let bytes = EMBEDDED_PATCHES
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, b)| *b)
                .expect("manifest 引用的补丁应已内嵌");
            let patch_path = tmp.join(name);
            std::fs::write(&patch_path, bytes).unwrap();
            patches.extend(
                load_patch(&format!("system/{name}"), &patch_path)
                    .expect("load_patch 应成功"),
            );
        }
        // 扁平补丁一个文件一个目标段，manifest 清单长度即解析出的段数。
        assert_eq!(
            patches.len(),
            patch_list.len(),
            "预期每个补丁文件恰好解析出一段"
        );

        dry_run_patches(&patches, &dest).expect("dry_run 应通过");
        apply_patches(&patches, &dest).expect("apply 应成功");

        // Sanity: the applied patch introduced a `UNITY_2022_3` guard.
        let cs =
            std::fs::read_to_string(dest.join("Editor/Commands/Assets/AssetCommands.cs"))
                .unwrap();
        assert!(
            cs.contains("UNITY_2022_3"),
            "patch 后的文件应包含 UNITY_2022_3 宏"
        );
        // Sanity (v0.7+ sets): serializedVersion downgraded, and the
        // Unsafe .meta's platformData block must be byte-identical to
        // CSharp's (Editor-only). Guards against malformed block
        // reconstruction — a missing `settings:` line once made Unity
        // uncheck every platform for the Unsafe plugin.
        if patch_set == "0.7" || patch_set == "0.8" || patch_set == "0.9" {
            let meta_dir = dest.join("Runtime/Plugins/CodeAnalysis");
            let platform_block = |name: &str| -> Vec<String> {
                let text =
                    std::fs::read_to_string(meta_dir.join(name)).unwrap();
                let mut out = Vec::new();
                let mut in_block = false;
                for line in text.lines() {
                    if line == "  platformData:" {
                        in_block = true;
                        continue;
                    }
                    if line.starts_with("  userData:") {
                        in_block = false;
                    }
                    if in_block {
                        out.push(line.to_owned());
                    }
                }
                out
            };
            let unsafe_meta = std::fs::read_to_string(
                meta_dir.join(
                    "UnityPipeline.System.Runtime.CompilerServices.Unsafe.dll.meta",
                ),
            )
            .unwrap();
            assert!(
                unsafe_meta.contains("serializedVersion: 2"),
                "patch 后的 .meta 应包含 serializedVersion: 2"
            );
            assert_eq!(
                platform_block(
                    "UnityPipeline.System.Runtime.CompilerServices.Unsafe.dll.meta"
                ),
                platform_block(
                    "UnityPipeline.Microsoft.CodeAnalysis.CSharp.dll.meta"
                ),
                "Unsafe 的 platformData 块应与其余 Roslyn .meta 一致（Editor-only）"
            );
        }

        // 0.9+ analytics guard: the real implementation is wrapped in
        // `#if ENABLE_ANALYTICS` and a no-op stand-in follows in `#else`. The
        // stand-in must keep the two entry points the package calls, or the
        // Editor assembly stops compiling on 2022.3 (where the guard is off).
        if patch_set == "0.9" {
            let analytics =
                std::fs::read_to_string(dest.join("Editor/PipelineAnalytics.cs")).unwrap();
            assert!(
                analytics.starts_with("#if ENABLE_ANALYTICS\n"),
                "analytics 补丁应把原实现整体括进 #if ENABLE_ANALYTICS"
            );
            let (real, stub) = analytics
                .split_once("#else\n")
                .expect("analytics 补丁应提供 #else 空实现");
            assert!(
                real.contains("EditorAnalytics.SendAnalytic"),
                "#if 分支应保留原 analytics 实现"
            );
            assert!(
                !stub.contains("EditorAnalytics.SendAnalytic"),
                "#else 空实现不得再触碰 analytics API"
            );
            assert!(
                stub.contains("class PipelineAnalytics")
                    && stub.contains("RecordCommandExecuted(in CommandExecutionInfo info)")
                    && stub.contains("SendSessionStoppedIfStarted()")
                    && stub.ends_with("#endif\n"),
                "#else 空实现应保留两个入口点并以 #endif 收尾"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
