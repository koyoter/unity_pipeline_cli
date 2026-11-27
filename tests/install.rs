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
/// anything newer than the last cutoff sticks to it (0.11.0 → 0.10).
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
    assert_eq!(set("0.9.1-exp.0"), "0.9");
    assert_eq!(set("0.9.1-exp.1"), "0.9.1");
    assert_eq!(set("0.9.1"), "0.9.1");
    assert_eq!(set("0.10.0-exp.0"), "0.9.1");
    assert_eq!(set("0.10.0-exp.1"), "0.10");
    assert_eq!(set("0.10.0"), "0.10");
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

/// 团结引擎项目的 ProjectVersion.txt 带 `m_TuanjieEditorVersion` 行：此时
/// package.json 的 `unity` 字段改写为项目引擎版本的 major.minor（团结 UPM
/// 对该字段做 <年>.<版本> 硬校验，Unity 6 风格的 `6000.0` 直接判非法）。
/// 非团结项目一律不动。
#[test]
fn adjust_package_for_tuanjie_rewrites_unity_field_only_for_tuanjie() {
    let tmp = std::env::temp_dir().join(format!(
        "unity_pipeline_tuanjie_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);

    let pkg_dir = tmp.join("package");
    let project_dir = tmp.join("project");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    std::fs::create_dir_all(project_dir.join("ProjectSettings")).unwrap();

    // Non-Tuanjie project: untouched even with a Unity 6 style unity field.
    std::fs::write(
        project_dir.join("ProjectSettings").join("ProjectVersion.txt"),
        "m_EditorVersion: 6000.0.23f1\n",
    )
    .unwrap();
    let pkg_json = r#"{"name":"com.unity.pipeline","version":"1.0.1","unity":"6000.0"}"#;
    std::fs::write(pkg_dir.join("package.json"), pkg_json).unwrap();
    adjust_package_for_tuanjie(&pkg_dir, &project_dir).unwrap();
    assert_eq!(
        std::fs::read_to_string(pkg_dir.join("package.json")).unwrap(),
        pkg_json
    );

    // Tuanjie project (2022.3.62t18): rewritten to the editor's major.minor.
    std::fs::write(
        project_dir.join("ProjectSettings").join("ProjectVersion.txt"),
        "m_EditorVersion: 2022.3.62t18\nm_EditorVersionWithRevision: 2022.3.62t18 (89460cc452f5)\nm_TuanjieEditorVersion: 1.11.1\n",
    )
    .unwrap();
    adjust_package_for_tuanjie(&pkg_dir, &project_dir).unwrap();
    let after = std::fs::read_to_string(pkg_dir.join("package.json")).unwrap();
    assert!(after.contains(r#""unity": "2022.3""#), "{after}");
    assert!(!after.contains("6000.0"));

    // Already matching: second run is a no-op (still valid JSON, same value).
    adjust_package_for_tuanjie(&pkg_dir, &project_dir).unwrap();
    assert!(std::fs::read_to_string(pkg_dir.join("package.json"))
        .unwrap()
        .contains(r#""unity": "2022.3""#));

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
        // dirty cache no longer matches any patch set's old-state. 旧档补
        // AssetCommands 时引入 UNITY_2022_3 宏；0.10 档不再碰它，改以
        // RoslynCompilationService 的 UNITY_ANDROID（pristine 包零出现）
        // 作为补丁痕迹。
        let asset_cs = src.join("Editor/Commands/Assets/AssetCommands.cs");
        let roslyn_cs = src.join("Runtime/Compilation/RoslynCompilationService.cs");
        let dirty = std::fs::read_to_string(&asset_cs)
            .map(|s| s.contains("UNITY_2022_3"))
            .unwrap_or(false)
            || std::fs::read_to_string(&roslyn_cs)
                .map(|s| s.contains("UNITY_ANDROID"))
                .unwrap_or(false);
        if dirty {
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

        // Sanity (≤0.9.1 sets): the applied patch introduced a
        // `UNITY_2022_3` guard. 0.10 档不再带 2022 垫片，AssetCommands 不动。
        if patch_set != "0.10" && patch_set != "1.0" {
            let cs =
                std::fs::read_to_string(dest.join("Editor/Commands/Assets/AssetCommands.cs"))
                    .unwrap();
            assert!(
                cs.contains("UNITY_2022_3"),
                "patch 后的文件应包含 UNITY_2022_3 宏"
            );
        }
        // Sanity (v0.7+ sets): serializedVersion downgraded, and the
        // Unsafe .meta's platformData block must be byte-identical to
        // CSharp's (Editor-only). Guards against malformed block
        // reconstruction — a missing `settings:` line once made Unity
        // uncheck every platform for the Unsafe plugin.
        if patch_set == "0.7"
            || patch_set == "0.8"
            || patch_set == "0.9"
            || patch_set == "0.10"
            || patch_set == "1.0"
        {
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
        // 0.10 档弃掉该垫片（上游不再 guard，min Unity 6000.0）。
        if patch_set == "0.9" || patch_set == "0.9.1" {
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

        // 0.10 档：Android Mono eval 移植（gate + APK 引用）须落在重写后的
        // script 版 eval 上；2022 垫片不得再出现。
        if patch_set == "0.10" {
            let roslyn =
                std::fs::read_to_string(dest.join("Runtime/Compilation/RoslynCompilationService.cs"))
                    .unwrap();
            assert_eq!(
                roslyn.matches("#if UNITY_EDITOR || UNITY_STANDALONE || UNITY_ANDROID").count(),
                2,
                "外层与 SnapshotProjectDefines 两处 gate 应带 UNITY_ANDROID"
            );
            assert!(
                roslyn
                    .contains("#if UNITY_EDITOR || (UNITY_STANDALONE && DEBUG) || UNITY_ANDROID"),
                "内层 gate 应带 UNITY_ANDROID"
            );
            assert!(
                roslyn.contains("class AndroidApkAssemblies")
                    && roslyn.contains("AndroidApkAssemblies.Open(")
                    && roslyn.contains(r#""ROSLYN002""#),
                "APK 内读 DLL 的 Android 分支与 Assembly.Load 失败兜底应就位"
            );
            let eval =
                std::fs::read_to_string(dest.join("Runtime/Compilation/EvalCodeCompiler.cs"))
                    .unwrap();
            assert_eq!(
                eval.matches("|| UNITY_ANDROID").count(),
                2,
                "EvalCodeCompiler 的 using-guard 与类 gate 应各带一处 UNITY_ANDROID"
            );

            // UnityApiCollector 是 0.10 新增类型，上游只在 Editor/桌面 debug 下
            // 编译；RoslynCompilationService 的实现区（已扩到 UNITY_ANDROID）第
            // 112 行无条件引用它，Android 打包报 CS0103。guard 必须同步。
            let collector =
                std::fs::read_to_string(dest.join("Runtime/Compilation/UnityApiCollector.cs"))
                    .unwrap();
            assert!(
                collector.starts_with(
                    "#if UNITY_EDITOR || (UNITY_STANDALONE && DEBUG) || UNITY_ANDROID\n"
                ),
                "UnityApiCollector 的类 gate 应带 UNITY_ANDROID，否则 Android 构建 CS0103"
            );

            // 0.10 的 PipelineAnalytics 上游无 guard，直接绑
            // UnityEngine.Analytics（IAnalytic 等）——Analytics 模块被裁掉的工程
            // 编不过。@4 垫片整体包进 #if ENABLE_ANALYTICS，#else 空实现须覆盖
            // 0.10 全部外部调用面（diagnostics record_analytics/recent_analytics
            // 的 IsRecording/RecentEvents/SentEvent + 两个上报入口）。
            let analytics =
                std::fs::read_to_string(dest.join("Editor/PipelineAnalytics.cs")).unwrap();
            assert!(
                analytics.starts_with("#if ENABLE_ANALYTICS\n"),
                "0.10 analytics 垫片应把原实现整体括进 #if ENABLE_ANALYTICS"
            );
            let (real, stub) = analytics
                .split_once("#else\n")
                .expect("0.10 analytics 垫片应提供 #else 空实现");
            assert!(
                real.contains("EditorAnalytics.SendAnalytic"),
                "#if 分支应保留上游 analytics 实现"
            );
            assert!(
                !stub.contains("EditorAnalytics.SendAnalytic")
                    && !stub.contains("IAnalytic"),
                "#else 空实现不得再触碰 analytics API"
            );
            assert!(
                stub.contains("class PipelineAnalytics")
                    && stub.contains("IsRecording")
                    && stub.contains("RecentEvents")
                    && stub.contains("class SentEvent")
                    && stub.contains("RecordCommandExecuted(in CommandExecutionInfo info)")
                    && stub.contains("SendSessionStoppedIfStarted()")
                    && stub.ends_with("#endif\n"),
                "#else 空实现应覆盖 0.10 的五成员调用面并以 #endif 收尾"
            );
        }

        // 1.0 档：1.0.0-pre.1 的 Scene Preview 用 Unity 6 专属的
        // UnityEditor.Build.Profile 编译期引用，2022.3 编译炸（build 命令链
        // 上游已自带反射垫片，不受影响）。补丁方案：五个文件把 using 换成
        // `#if UNITY_6000_0_OR_NEWER` 原样 / `#else` 别名到 2022 专用 stub，
        // Engine 文件追加 stub 类型本体——上游代码对各处 profile 为 null
        // 已有降级路径，2022.3 上 Scene Preview 因此整体走"无 profile"分支。
        if patch_set == "1.0" {
            let alias =
                "using BuildProfile = Unity.Pipeline.Editor.Previews.ScenePreviewBuildProfileStub;";
            for rel in [
                "Editor/EditorPipelineConfigEditor.cs",
                "Editor/Previews/ScenePreview/PreviewBuildCheck.cs",
                "Editor/Previews/ScenePreview/PreviewSceneUsage.cs",
                "Editor/Previews/ScenePreview/ScenePreviewEngine.cs",
                "Editor/Previews/ScenePreview/ScenePreviewSettings.cs",
            ] {
                let cs = std::fs::read_to_string(dest.join(rel)).unwrap();
                assert!(
                    cs.contains(
                        "#if UNITY_6000_0_OR_NEWER\nusing UnityEditor.Build.Profile;\n#else\n"
                    ),
                    "{rel} 的 using 应在 UNITY_6000_0_OR_NEWER 下保留上游原样"
                );
                assert!(cs.contains(alias), "{rel} 应把 BuildProfile 别名到 2022 stub");
            }
            let engine = std::fs::read_to_string(
                dest.join("Editor/Previews/ScenePreview/ScenePreviewEngine.cs"),
            )
            .unwrap();
            assert!(
                engine.contains(
                    "internal static BuildProfile GetActiveBuildProfile() => BuildProfile.GetActiveBuildProfile();"
                ),
                "Engine 的真实 GetActiveBuildProfile 不得被 stub hunk 误删"
            );
            assert!(
                engine.contains("#if !UNITY_6000_0_OR_NEWER")
                    && engine
                        .contains("internal class ScenePreviewBuildProfileStub : ScriptableObject")
                    && engine.contains(
                        "internal static ScenePreviewBuildProfileStub GetActiveBuildProfile() => null;"
                    )
                    && engine.contains("EditorBuildSettingsScene[] scenes"),
                "Engine 文件应追加 2022 stub（含 scriptingDefines/scenes 字段面）且仅限 Unity 6 以下"
            );
            let config = std::fs::read_to_string(dest.join("Editor/EditorPipelineConfigEditor.cs"))
                .unwrap();
            assert!(
                config.contains(
                    "#else\n            EditorGUILayout.Space();\n            EditorGUILayout.HelpBox(\n                \"Scene Preview requires Unity 6"
                ),
                "2022 上 Scene Preview 设置段应降级为提示框，不得显示死按钮"
            );

            // 1.0.0-pre.1 的 PipelineTestRunner 新增编译期 TestRunnerApi.CancelTestRun
            // （TF 1.4 API，2022.3 的 TF 1.1.33 没有，CS0117）。@2 延续 @1 的反射修：
            // 运行期解析，解析不到时 cancel 报"无法停止"（pre-1.4 行为）。@1 的
            // UnregisterTestCallback 反射修也必须随 @2 带上（1.0 档不再引用 @1）。
            let runner = std::fs::read_to_string(dest.join("Editor/Testing/PipelineTestRunner.cs"))
                .unwrap();
            assert!(
                !runner.contains("TestRunnerApi.CancelTestRun")
                    && !runner.contains("TestRunnerApi.UnregisterTestCallback"),
                "不得再有编译期 CancelTestRun / UnregisterTestCallback 调用"
            );
            assert!(
                runner.contains("\"CancelTestRun\"")
                    && runner.contains("internal static Func<string, bool> s_CancelTestRun = id =>")
                    && runner.contains("s_UnregisterTestCallback?.Invoke(null, new object[] { collector })"),
                "s_CancelTestRun 应改为反射解析 CancelTestRun，且保留 @1 的 UnregisterTestCallback 反射修"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// 新文件补丁（/dev/null 源）语义：空树创建；重装遇非空遗留产物幂等跳过。
/// 用真实内嵌的 AssistantToolBridge 新文件补丁做夹具，防止「解析成功但静默
/// 跳过」或「重装撞 hunk 不匹配」两类回归。
#[test]
fn new_file_patch_creates_and_skips_idempotently() {
    let (name, bytes) = EMBEDDED_PATCHES
        .iter()
        .find(|(n, _)| n.contains("AssistantToolBridge"))
        .expect("AssistantToolBridge 新文件补丁应已内嵌")
        .to_owned();
    let tmp = std::env::temp_dir().join(format!(
        "unity_pipeline_new_file_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let tree = tmp.join("tree");
    std::fs::create_dir_all(&tree).unwrap();
    let patch_path = tmp.join(name);
    std::fs::write(&patch_path, bytes).unwrap();
    let patches = load_patch(&format!("system/{name}"), &patch_path).unwrap();
    assert_eq!(patches.len(), 1);
    assert!(patches[0].is_new_file, "夹具应是 /dev/null 新文件补丁");

    dry_run_patches(&patches, &tree).expect("空树 dry_run 应通过");
    apply_patches(&patches, &tree).expect("空树 apply 应创建文件");
    let target = tree.join("Editor").join("Commands").join("AssistantToolBridge.cs");
    let first = std::fs::read(&target).expect("新文件应已创建");
    assert!(String::from_utf8_lossy(&first).contains("AssistantToolBridge"));

    // 重装：非空遗留产物必须幂等跳过，而不是撞 hunk 不匹配
    dry_run_patches(&patches, &tree).expect("重装 dry_run 应跳过遗留产物");
    apply_patches(&patches, &tree).expect("重装 apply 应跳过遗留产物");
    let second = std::fs::read(&target).unwrap();
    assert_eq!(first, second, "遗留产物不应被改写");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// 垫片嵌入完整性：asmdef 名逐字校验、DLL 体量、属性类源码在场。
#[test]
fn embedded_shim_has_asmdef_dll_and_package_json() {
    let get = |n: &str| {
        EMBEDDED_SHIM
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, b)| *b)
    };
    assert!(get("cn.tuanjie.codely.bridge.shim/package.json").is_some());
    let asmdef = get("cn.tuanjie.codely.bridge.shim/UnityTcp.Editor.asmdef").unwrap();
    assert!(
        String::from_utf8_lossy(asmdef).contains("\"name\": \"UnityTcp.Editor\""),
        "asmdef 名漂移会让助手包整批编译失败"
    );
    let dll = get("cn.tuanjie.codely.bridge.shim/Plugins/Codely.Newtonsoft.Json.dll").unwrap();
    assert!(dll.len() > 500_000, "Codely Newtonsoft DLL 体量异常");
    let src = get("cn.tuanjie.codely.bridge.shim/ExecuteCustomTool.cs").unwrap();
    assert!(String::from_utf8_lossy(src).contains("CustomToolAttribute"));
    let notifier = get("cn.tuanjie.codely.bridge.shim/AgentInputNotifier.cs").unwrap();
    let notifier_src = String::from_utf8_lossy(notifier);
    assert!(notifier_src.contains("AgentInputNotifier"));
    // 垫片形态的 Ask Assistant 走「复制到剪贴板 + toast」：两者都得在桩里。
    assert!(notifier_src.contains("systemCopyBuffer"), "垫片桩必须写系统剪贴板");
    assert!(notifier_src.contains("ShowNotification"), "垫片桩必须有复制成功 toast");
    assert!(notifier_src.contains("附件"), "剪贴板内容须含附件路径段");
}

/// 助手包安装离线链路：合成 zip → extract_zip（含 zip-slip 拒绝）→
/// copy_dir_recursive（任意深度 Tests/ 跳过）。联网下载与真桥冲突提示由
/// 编辑器验收人工覆盖。
#[test]
fn extract_zip_and_copy_assistant_packages_skips_tests() {
    use std::io::Write as _;
    let tmp = std::env::temp_dir().join(format!(
        "unity_pipeline_assistants_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let make_zip = |name: &str, entries: &[(&str, &[u8])]| {
        let path = tmp.join(name);
        let file = std::fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        for (n, b) in entries {
            w.start_file(*n, opts).unwrap();
            w.write_all(b).unwrap();
        }
        w.finish().unwrap();
        path
    };

    let zip_path = make_zip(
        "ext.zip",
        &[
            (
                "perf-assistant/Packages/cn.tuanjie.profiler.assistant/package.json",
                br#"{"name":"cn.tuanjie.profiler.assistant"}"#.as_slice(),
            ),
            (
                "perf-assistant/Packages/cn.tuanjie.profiler.assistant/Editor/Tools/ProfilerTools.cs",
                b"// tool",
            ),
            (
                "perf-assistant/Packages/cn.tuanjie.profiler.assistant/Editor/Tests/ApiTests.cs",
                b"using NUnit.Framework;",
            ),
            (
                "perf-assistant/Packages/cn.tuanjie.assets.assistant/package.json",
                br#"{"name":"cn.tuanjie.assets.assistant"}"#.as_slice(),
            ),
        ],
    );

    let extract_root = tmp.join("ext");
    std::fs::create_dir_all(&extract_root).unwrap();
    extract_zip(&zip_path, &extract_root).unwrap();
    let src_packages = extract_root.join("perf-assistant").join("Packages");
    assert!(
        src_packages
            .join("cn.tuanjie.profiler.assistant")
            .join("package.json")
            .exists()
    );

    let dest = tmp.join("proj").join("Packages");
    std::fs::create_dir_all(&dest).unwrap();
    let pkg_dest = dest.join("cn.tuanjie.profiler.assistant");
    std::fs::create_dir_all(&pkg_dest).unwrap();
    copy_dir_recursive(
        &src_packages.join("cn.tuanjie.profiler.assistant"),
        &pkg_dest,
    )
    .unwrap();
    assert!(pkg_dest.join("Editor").join("Tools").join("ProfilerTools.cs").exists());
    assert!(
        !pkg_dest.join("Editor").join("Tests").exists(),
        "任意深度 Tests/ 应被跳过（NUnit 引用会炸助手包主 asmdef 编译）"
    );

    // zip-slip：../ 条目必须被拒
    let slip = make_zip("slip.zip", &[("perf-assistant/../evil.txt", b"x" as &[u8])]);
    let err = extract_zip(&slip, &tmp.join("slip-out"));
    assert!(err.is_err(), "../ 条目必须被 zip-slip 防护拒绝");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// 真实扩展 zip 的解压冒烟（存在才跑）：7 个助手包目录齐全。防真实产物
/// 的布局/压缩方法偏离合成夹具。文件名随版本走（元数据接口动态解析），
/// 因此扫 downloads/ 下任意 perf-assistant-ext-v*.zip。
#[test]
fn real_assistants_zip_extracts_seven_packages() {
    let downloads = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("downloads");
    let mut zip_path: Option<std::path::PathBuf> = None;
    if let Ok(entries) = std::fs::read_dir(&downloads) {
        for e in entries.flatten() {
            let name = e.file_name();
            let name = name.to_str().unwrap_or_default();
            if name.starts_with("perf-assistant-ext-v") && name.ends_with(".zip") {
                zip_path = Some(e.path());
            }
        }
    }
    let Some(zip_path) = zip_path else {
        eprintln!("skip: downloads/ 下没有 perf-assistant-ext-v*.zip");
        return;
    };
    let tmp = std::env::temp_dir().join(format!(
        "unity_pipeline_real_zip_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    extract_zip(&zip_path, &tmp).unwrap();
    for id in ASSISTANT_PACKAGE_IDS {
        assert!(
            tmp.join("perf-assistant")
                .join("Packages")
                .join(id)
                .join("package.json")
                .exists(),
            "zip 里缺助手包 {id}"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// 真桥在场检测：目录 / manifest.json dependencies / packages-lock.json
/// dependencies 三种形态都算——registry 解析的真桥不落 Packages/ 目录。
#[test]
fn bridge_present_detects_dir_manifest_and_lock() {
    let tmp = std::env::temp_dir().join(format!(
        "unity_pipeline_bridge_detect_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let pkgs = tmp.join("Packages");
    std::fs::create_dir_all(&pkgs).unwrap();
    assert!(!bridge_present(&pkgs), "空项目不应检测到真桥");

    // manifest.json 声明（registry 解析形态）
    std::fs::write(
        pkgs.join("manifest.json"),
        r#"{"dependencies": {"com.unity.pipeline": "1.0.1", "cn.tuanjie.codely.bridge": "1.0.93"}}"#,
    )
    .unwrap();
    assert!(bridge_present(&pkgs), "manifest 声明应检测到");
    std::fs::remove_file(pkgs.join("manifest.json")).unwrap();

    // packages-lock.json 声明（含助手包 dep 干扰项，顶层键才算）
    std::fs::write(
        pkgs.join("packages-lock.json"),
        r#"{"dependencies": {"cn.tuanjie.profiler.assistant": {"version": "file:cn.tuanjie.profiler.assistant", "dependencies": {"cn.tuanjie.codely.bridge": "1.0.80"}}, "cn.tuanjie.codely.bridge": {"version": "1.0.93"}}}"#,
    )
    .unwrap();
    assert!(bridge_present(&pkgs), "lock 顶层声明应检测到");
    std::fs::remove_file(pkgs.join("packages-lock.json")).unwrap();

    // 目录形态
    std::fs::create_dir_all(pkgs.join("cn.tuanjie.codely.bridge")).unwrap();
    assert!(bridge_present(&pkgs), "目录形态应检测到");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// 助手包 package.json 的真桥依赖剥离：只删真桥键，其余依赖保留；
/// 无 dependencies / 坏 JSON 原样不动（真桥不在场时 UPM 会整包忽略助手包）。
#[test]
fn strip_bridge_dependency_removes_only_bridge_key() {
    let tmp = std::env::temp_dir().join(format!(
        "unity_pipeline_strip_dep_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let p = tmp.join("package.json");
    std::fs::write(
        &p,
        r#"{"name":"cn.tuanjie.assets.assistant","dependencies":{"cn.tuanjie.codely.bridge":"1.0.80","com.unity.nuget.mono-cecil":"1.10.2"}}"#,
    )
    .unwrap();
    strip_bridge_dependency(&p);
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    let deps = v["dependencies"].as_object().unwrap();
    assert!(!deps.contains_key("cn.tuanjie.codely.bridge"));
    assert_eq!(deps.get("com.unity.nuget.mono-cecil").unwrap(), "1.10.2");

    std::fs::write(&p, r#"{"name":"x"}"#).unwrap();
    strip_bridge_dependency(&p);
    std::fs::write(&p, "not json").unwrap();
    strip_bridge_dependency(&p);

    let _ = std::fs::remove_dir_all(&tmp);
}

/// skills 同步语义：同名子目录先删后复制（旧内容被整目录替换）、
/// 异名子目录不碰、根级文件覆盖。exe 目录定位在调用方，此处测纯复制逻辑。
#[test]
fn copy_skills_into_replaces_same_name_only() {
    let tmp = std::env::temp_dir().join(format!(
        "unity_pipeline_skills_test_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let src = tmp.join("skills-src");
    let dest = tmp.join("skills-out");
    std::fs::create_dir_all(src.join("skill-a")).unwrap();
    std::fs::create_dir_all(src.join("skill-b")).unwrap();
    std::fs::write(src.join("skill-a").join("SKILL.md"), b"new").unwrap();
    std::fs::write(src.join("skill-b").join("SKILL.md"), b"b").unwrap();
    std::fs::create_dir_all(dest.join("skill-a")).unwrap();
    std::fs::write(dest.join("skill-a").join("SKILL.md"), b"old").unwrap();
    std::fs::write(dest.join("skill-a").join("user-edit.txt"), b"custom").unwrap();
    std::fs::create_dir_all(dest.join("foreign")).unwrap();
    std::fs::write(dest.join("foreign").join("keep.txt"), b"keep").unwrap();

    let copied = copy_skills_into(&src, &dest).unwrap();
    assert_eq!(copied, 2, "只处理 src 顶层条目");
    assert_eq!(
        std::fs::read_to_string(dest.join("skill-a").join("SKILL.md")).unwrap(),
        "new",
        "同名 skill 应被整目录替换"
    );
    assert!(
        !dest.join("skill-a").join("user-edit.txt").exists(),
        "替换后的同名 skill 不应残留旧文件"
    );
    assert_eq!(
        std::fs::read_to_string(dest.join("foreign").join("keep.txt")).unwrap(),
        "keep",
        "非同名子目录不应被碰"
    );
    assert_eq!(
        std::fs::read_to_string(dest.join("skill-b").join("SKILL.md")).unwrap(),
        "b"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// parse_assistants_meta：按 item.attachmentId 取附件（图标附件在前也不选错）、
/// 相对 url 拼站点根、服务端文件名落盘前做基名清洗。
#[test]
fn parse_assistants_meta_resolves_by_attachment_id() {
    let json = concat!(
        r#"{"item":{"attachmentId":368},"attachment_map":{"#,
        r#""283":{"filename":"icon.png","url":"https://cdn.example/icon.png"},"#,
        r#""368":{"filename":"perf-assistant-ext-v0.8.3.zip","#,
        r#""url":"/skills/download/abc/perf-assistant-ext-v0.8.3.zip"}}}"#
    );
    let (url, name) = parse_assistants_meta(json).unwrap();
    assert_eq!(
        url,
        "https://codely.tuanjie.cn/skills/download/abc/perf-assistant-ext-v0.8.3.zip"
    );
    assert_eq!(name, "perf-assistant-ext-v0.8.3.zip");

    // 信任边界：带路径分隔符/非 .zip 的文件名一律拒绝。
    for evil in [
        r#"{"item":{"attachmentId":1},"attachment_map":{"1":{"filename":"..\\evil.zip","url":"https://x/e.zip"}}}"#,
        r#"{"item":{"attachmentId":1},"attachment_map":{"1":{"filename":"sub/evil.zip","url":"https://x/e.zip"}}}"#,
        r#"{"item":{"attachmentId":1},"attachment_map":{"1":{"filename":"evil.exe","url":"https://x/e.exe"}}}"#,
    ] {
        assert!(parse_assistants_meta(evil).is_err(), "应拒绝非法文件名 {evil}");
    }
    assert!(parse_assistants_meta("{}").is_err(), "缺字段应报错");
}
