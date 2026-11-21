using System;
using System.Collections.Generic;
using UnityEngine;
using UnityEditor;

namespace UnityTcp.Editor.Notifier
{
    /// <summary>
    /// 垫片桩：签名与真桥 1.0.80 一致。真桥经 TCP 推送到 CodelyWindow 前端；
    /// 本垫片不连接 Codely，仅保助手包编译兼容并如实披露。
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
            if (!_warned)
            {
                _warned = true;
                Debug.LogWarning(
                    "[cn.tuanjie.codely.bridge.shim] AgentInputNotifier.Send 为垫片桩，消息不会送达 Codely。" +
                    "（「Ask Assistant」等编辑器按钮在此安装形态下不可用）");
            }
            return false;
        }
    }
}
