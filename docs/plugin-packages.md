# 本机插件包：依赖与资源

这是实验性 Windows x64 插件包规范。清单版本与 C ABI 版本独立：包格式支持 `schema_version: 2`，C ABI 仍为 0.1。能力包括整段同步 `ag.whole_sync/1` 和新版宿主的可选实时处理器 `ag.realtime_audio/1`；每个节点声明其对应的一种，包声明必须等于节点能力集合，未知/重复能力拒绝。旧版 v1 包继续受理，不需要改写旧 Graph。实时接口和限制见[实时插件说明](plugin-realtime.md)。

## 包结构

```text
example/
  manifest.json
  nodes.json
  ag_example_plugin.dll
  org.audiograph.example.algorithms.dll
  resources/
    config/
      gain-scale.txt
```

manifest 的身份、ABI、平台、入口和节点字段保持原意。v2 的 `files` 为每个成员增加 `role`：

```json
{
  "files": [
    {"path": "ag_example_plugin.dll", "sha256": "<64位小写SHA256>", "role": "entry"},
    {"path": "nodes.json", "sha256": "<64位小写SHA256>", "role": "nodes"},
    {"path": "org.audiograph.example.algorithms.dll", "sha256": "<64位小写SHA256>", "role": "dependency"},
    {"path": "resources/config/gain-scale.txt", "sha256": "<64位小写SHA256>", "role": "resource"}
  ]
}
```

上例只是字段片段，不是可直接安装的完整清单；完整示例由独立插件项目构建生成。

- `entry` 和 `nodes` 各一个，与顶层 `entry`、`nodes` 完全对应；节点描述仍叫 `nodes.json`。
- `dependency` 为同包根目录下的辅助 DLL，文件名以 `plugin_id + "."` 开头，避免各开发者都发布名为 `utils.dll` 的依赖。
- `resource` 为 `resources/` 下的模型、配置等数据文件；宿主不会解释模型格式，也不会自动下载。
- 所有成员逐个记录 SHA256。manifest 自身由应用启动快照记录指纹，不在 `files` 中自引用。v2 目录中不得混入未声明文件或无关空目录。
- v1 仍使用没有 `role` 的两项 `files`，只描述入口 DLL 和节点描述；不要把 v2 字段塞进 v1 清单。宿主保留旧 v1 对额外成员不检查的行为，但安装器仍只接受其声明的两个文件和 manifest，旧包里的额外文件不获得校验保证。

## 路径与容量

包内路径统一使用 `/`，只允许 ASCII 字母、数字、`_`、`-`、`.` 的安全路径段；不允许绝对路径、反斜线、冒号、`.`/`..`、Windows 保留设备名或尾随点/空格。不区分大小写的同名路径也视为冲突。包安装目录本身可以位于中文用户目录。

- 路径最多 256 字节，每段最多 128 字节；资源最多四段，例如 `resources/a/b/model.bin`。
- 最多 64 个声明文件，整个目录最多 256 个文件与目录条目（含 manifest）。
- manifest 最大 64 KiB，nodes.json 最大 512 KiB，每个 DLL/资源最大 256 MiB，声明文件合计最大 1 GiB。
- 这些是本版明确容量，不代表支持任意大小的深度学习模型。大型外置模型绑定、延迟下载和缓存另行设计。

安装器和宿主都检查路径、清单、指纹和容量。拒绝链接、重解析点、硬链接及越界路径。复制和大文件校验使用分块读写，不把整个模型文件塞进 JSON 或节点目录。

## 节点怎样使用

`ag_instance_environment.resource_root` 仍是完整包的绝对 UTF-8 路径；插件自行拼接其声明的 `resources/...` 文件。它按约定只读，不是 Graph 的工作区文件参数。`data_root` 继续用于插件/工作区隔离的可写缓存或运行数据；卸载插件不会删除该目录。

插件通过自己的辅助 DLL 实现计算，无须链接 AudioGraph 内部核心。示例在 run 时读取 `resources/config/gain-scale.txt`，调用自己的算法 DLL 处理音频；值 `1.0` 保持原 Gain 行为，文件缺失或内容无效会明确失败。示例仍是 Gain 和模拟 ASR，不是真实识别模型。

依赖 DLL 与入口放在同一目录，加载使用包目录和 Windows 系统目录，不改进程 PATH 或当前目录。同一选用集合内的 DLL 基名冲突会拒绝；运行时还需防止使用已加载的其他路径同名模块。开发者应命名私有 DLL，并将所需的非系统依赖全部随包声明。

## 验证和边界

安装与节点发现只做静态检查，不加载 DLL。首次执行前再验证并保持声明成员的文件句柄；更换资源也属于包变化，需更新指纹并重启，不在运行中换模型或 DLL。在实际 AudioGraph 的插件管理界面安装生成的插件目录；启停或安装变更后重启以更新节点发现，卸载时保留备份并检查已有 Graph 的缺失节点提示。开发步骤见[节点开发约定](plugin-development.md)。

这不是原生代码沙箱，也不是任意第三方 DLL 的完全隔离机制。插件代码仍能自行联网、访问其他文件、加载其他模块或调用 GPU；清单校验不证明算法安全，也不能保证依赖 ABI 相容。实时回调禁止这些阻塞操作，必须遵守独立实时契约。尚不提供普通离线streaming/异步插件、插件商店、凭据系统或覆盖更新能力。

验证应覆盖：独立 SDK 构建；辅助 DLL 与资源确实参与计算；v1 兼容；成员缺失/篡改；大小写冲突；路径与容量拒绝；静态发现不加载代码；安装后重启、用户/AI 后台执行、卸载备份保留全部成员及原音频。
