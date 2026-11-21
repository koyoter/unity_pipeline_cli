using System;

namespace UnityTcp.Editor.Tools
{
    /// <summary>
    /// 垫片（cn.tuanjie.codely.bridge.shim）：只保留真桥 1.0.80 中助手包编译所需的
    /// <c>CustomToolAttribute</c>。运行期工具的发现与执行由 unity_pipeline_cli 的
    /// pipeline 补丁反射本属性完成，不经过本类任何成员。
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
    }
}
