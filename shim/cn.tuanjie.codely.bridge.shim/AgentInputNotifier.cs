using System;
using System.Collections.Generic;
using System.Text;
using UnityEditor;
using UnityEngine;

namespace UnityTcp.Editor.Notifier
{
    /// <summary>
    /// 垫片桩：签名与真桥 1.0.80 一致。真桥经 TCP 推送到 CodelyWindow 前端；
    /// 本垫片不连接 Codely，改为把消息复制到系统剪贴板（text + 附件路径），
    /// 供用户粘贴到其他 agent 运行，并在当前窗口 toast 提示复制完成。
    /// 返回值保持真桥语义（false = 未送达 Codely），如实证伪。
    /// </summary>
    public static class AgentInputNotifier
    {
        private static bool _warned;

        public static bool Send(
            string text,
            string source = "editor_window",
            string type = "user_message",
            bool autoSubmit = false,
            DateTime? timestamp = null,
            IReadOnlyList<string> attachments = null)
        {
            // 与真桥守卫一致：text 与 attachments 都空视为无效消息，不复制不提示。
            var paths = new List<string>();
            if (attachments != null)
            {
                for (int i = 0; i < attachments.Count; i++)
                {
                    string p = attachments[i] != null ? attachments[i].Trim() : null;
                    if (!string.IsNullOrEmpty(p)) paths.Add(p);
                }
            }

            bool hasText = !string.IsNullOrWhiteSpace(text);
            if (!hasText && paths.Count == 0) return false;

            var sb = new StringBuilder();
            if (hasText) sb.Append(text.TrimEnd());
            if (paths.Count > 0)
            {
                if (sb.Length > 0) sb.Append("\n\n");
                sb.Append("附件：");
                for (int i = 0; i < paths.Count; i++) sb.Append('\n').Append(paths[i]);
            }
            string payload = sb.ToString();

            EditorGUIUtility.systemCopyBuffer = payload;

            if (!_warned)
            {
                _warned = true;
                Debug.LogWarning(
                    "[cn.tuanjie.codely.bridge.shim] AgentInputNotifier.Send is a shim stub: the message was " +
                    "copied to the system clipboard but NOT delivered to Codely. Editor buttons such as " +
                    "\"Ask Assistant\" work as a copy-and-paste flow under this installation.");
            }

            string toast = paths.Count > 0
                ? "Prompt copied to clipboard (" + paths.Count + " attachments) — paste into your agent"
                : "Prompt copied to clipboard — paste into your agent";

            // 通知落在用户刚点击的窗口上；无焦点窗口时退到最近活跃的 SceneView。
            EditorWindow target = EditorWindow.focusedWindow;
            if (target == null) target = SceneView.lastActiveSceneView;
            if (target != null) target.ShowNotification(new GUIContent(toast));

            return false;
        }
    }
}
