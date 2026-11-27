using System;

namespace UnityTcp.Editor.Tools
{
    /// <summary>
    /// 垫片（cn.tuanjie.codely.bridge.shim）：保留真桥 1.0.8x 中助手包编译所需的
    /// <c>CustomToolAttribute</c> 与 <c>GetCustomToolsInfo</c>。运行期工具的发现与
    /// 执行由 unity_pipeline_cli 的 pipeline 补丁反射本属性完成。
    /// </summary>
    public static class ExecuteCustomTool
    {
        /// <summary>逐字保真自真桥 1.0.80（含 description = null 默认值）。</summary>
        [AttributeUsage(AttributeTargets.Method, AllowMultiple = false)]
        public class CustomToolAttribute : Attribute
        {
            public string Name { get; }
            public string Description { get; }

            public CustomToolAttribute(string name, string description = null)
            {
                Name = name;
                Description = description;
            }
        }

        /// <summary>
        /// 真桥为 perf.list_tools 提供的工具清单接口（perf-assistant 0.8.3 新增依赖）。
        /// 垫片版返回空清单：工具发现本就由 pipeline 补丁吸收进 /api/commands
        /// （MCP tools 列表），list_tools 只是无 tools/list 客户端的兜底，这里不给假数据。
        /// ponytail: 需要垫片端完整清单时，把补丁的反射扫描搬进来。
        /// </summary>
        public static object GetCustomToolsInfo() => new { tools = Array.Empty<object>() };
    }
}
