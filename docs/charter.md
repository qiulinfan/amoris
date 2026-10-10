# Amoris 纲领：统一技术栈

状态：Accepted（本轮方向与技术栈）<br>
版本：1.0<br>
日期：2026-10-04<br>
仓库：公开仓库 `qiulinfan/amoris`（本地 `~/Desktop/amoris`）<br>
来源：所有者 2026-10-03/04 的指示；Amoris 前序 `rebuild` 分支纲领 v0.11；`Amoris Pioneer` 的
`rebuild` 线（纲领 0.4，提交 `e9e53545`）与 `master` 线；Amoris 前序 `feature/rust-core` 与
`feature/agent-native-rust` 原型

## 1. 这份文档是什么

所有者在 2026-10-03 决定将前序实验统一到 Rust 宿主与 TypeScript 脚本栈：Lua 暂时搁置，
放弃 C++ 宿主。本轮实现已迁入 Amoris；本仓库承载完整引擎及其技术栈，继续按可运行的
端到端成果推进。Amoris Pioneer 保留前序实验与测量历史。

本文固定 `Amoris` 的定位、技术栈及理由、架构、证明目标与来源。子系统的细节规格在 `docs/spec/`
（多数来自 `Amoris Pioneer` rebuild 线，随代码一起导入），实施排期在 [schedule.md](schedule.md)。

## 2. 定位

Amoris 是一个 **agent-native** 的 3D 游戏引擎。agent 在三个角色上都是一等公民，并且与人类使用
同一套接口：

| 角色 | 含义 | 引擎提供什么 |
|---|---|---|
| agent 当开发者 | 读写场景、脚本、资源，构建并验证游戏 | 一张命令目录（类型化、带 JSON Schema、拒绝未知字段并给建议），编辑器也只用它 |
| agent 当玩家 | 在运行中的世界里，以受限感知和意图式动作玩游戏 | 按 observer 投影的感知层、意图与 affordance、显式时间模型、决策时暂停 |
| agent 当调试者 | 定位"为什么"：断点、单步、数据断点、因果链、时间回溯 | 与人类共用的调试核心：人用 Chrome DevTools / VS Code / 编辑器，agent 用 MCP 工具 |

所有者 2026-10-04 给出的目标：

- **最强的 agent 原生能力**：agent 玩游戏、开发游戏、调试游戏。CLI 与架构比 MCP 更重要：agent
  通过 shell 使用工具最顺手、最省 token；MCP 只是同一命令目录的薄投影。
- **渲染与物理性能比肩 Unity 与 UE5**。
- **web 渲染性能比肩 three.js**。

本轮必须证明五件事（所有者 2026-10-03 的要求）：

1. **性能**：渲染与物理比肩 Unity/UE5，web 比肩 three.js。同机对照测量：Bevy 0.19（Rust/wgpu 的前
   沿）、three.js WebGPURenderer（web）、Jolt（物理，Godot 4.6 起新项目的默认 3D 物理）（第 6 节）。
2. **agent-native gameplay**：LLM agent 通过 MCP、在受限感知下玩通游戏；脚本里的决策模型每秒做
   上百次决策；fork 前瞻。
3. **web 渲染与神经渲染的先进性**：同一渲染器在浏览器 WebGPU 上运行；3D Gaussian Splatting 与神经
   纹理（着色器内 MLP 解码）作为渲染器的一等特性。
4. **编辑器的现代性与完整性**：可停靠的多面板 Web 编辑器，视口即引擎本身，层级、检视器、Gizmo、
   资源、脚本编辑与调试、时间线、性能分析、agent 面板齐备。
5. **断点调试与 agent 原生调试的流畅性**：TypeScript 源码级断点（含条件断点、单步、局部变量、帧上
   求值）在真实的 Chrome DevTools 与 VS Code 中可用；同一调试核心以 MCP 工具暴露给 agent，并有
   引擎级的数据断点、因果追溯与时间回溯。

## 3. 原则

1. **同一接口**。编辑器、agent、脚本、测试使用同一张命令目录；编辑器没有特权通道，所以人能做的
   agent 都能做，反之亦然（`Amoris Pioneer` master 验证过的做法）。
2. **感知是游戏定义的一部分**。每个 observer 经同一感知层取信息（有范围、遮挡、注意力）；全知视图
   必须显式标记，只给调试与评测。观察以拉取为主，事件推送差量，每次调用支持 token 预算。
3. **状态全在世界里，由运行时强制**。游戏状态只存在 ECS 组件里；脚本是无状态系统，冻结的全局、
   lint 与重载等价检查共同保证。
4. **确定性、fork、replay 是一等能力**。固定步长，同种子同输入逐 tick 同哈希；fork 是按需能力
   （前瞻、测试、编辑器 Play、调试回溯），不进入每 tick 的执行路径；replay 能定位首个分叉 tick。
5. **意图式动作与结构化错误**。错误是 `{code, message, detail}`；未知参数整调用拒绝并给出"是不是
   想写……"。
6. **LLM 不在 tick 内**；决策模型（脚本或原生策略）可以在 tick 内，以每秒上百次决策运行。
7. **数据导向的性能**。脚本按列批量处理（typed array），重活在 Rust 系统里；渲染以 GPU 驱动为
   目标，没有静默容量上限。性能测量随提交记录，用来发现退化与指导优化，不作为通过门槛。
8. **结构整洁**。按职责划分模块与 crate，依赖单向并由检查命令核对；不设文件行数上限。
9. **不维护文件核对哈希清单**。2026-10-10，所有者要求移除共享文档、素材、展示与评测的
   文件哈希记录，减少重复维护。来源保留仓库、提交、日期与授权信息；`shared/SYNC.toml`
   只记录可选来源提交。生成文件和 vendored 源码仍可重建后逐字节比较，不另存核对基线。
   运行时的内容身份、世界哈希、snapshot/fork/replay 与依赖锁的供应链完整性机制保持不变。
10. **运行产物不入库**。常规日志、逐帧数据、原始测量与一次运行的报告放在忽略目录 `out/` 或
    CI artifacts；经隐私与凭据清理的公开产物归档到独立的
    [amoris-benchmarks-results](https://github.com/qiulinfan/amoris-benchmarks-results) 仓库。
    引擎仓库保留测试源码、固定夹具、benchmark 源码及简明结果表和方法。

## 4. 技术栈

### 4.1 总表

| 层 | 决策 | 版本 | 主要理由 |
|---|---|---|---|
| 宿主语言 | **Rust**（edition 2024） | 1.98.1 | 所有权让世界状态自包含，fork 不会有隐藏共享；serde/schemars 让序列化与 schema 来自类型；同一编译器覆盖原生与 `wasm32` |
| 构建 | **Cargo workspace + `cargo xtask`**；TS 包用 bun；web 用 wasm-bindgen + wasm-opt | — | 不自研构建系统（见 4.6） |
| ECS | `bevy_ecs`（只用 ECS） | 0.19.1 | 实体加组件本身就是语义 schema；单线程确定调度，遍历按 EntityId 排序 |
| 模拟数学 | `pocket_sim::math`（基于 `libm`，禁用平台超越函数，`-ffp-contract=off`） | libm 0.2.16 | 跨平台、跨原生/web 位级一致 |
| 物理 | **Rapier 3D**，`enhanced-determinism`；与 Jolt 同机对照后定终选（见 4.7） | 0.36.0 | 原生与 wasm 3001/3001 tick 一致，fork 后逐位延续（Amoris Pioneer 物理 spike）；性能需对标 PhysX/Chaos |
| 持久化 | PCE 规范编码、分节哈希树（XXH3-128 / BLAKE3）、snapshot/restore/fork、replay 与首个分叉 | 自研 | 哈希定义在规范编码上而非内存布局 |
| 脚本语言 | **TypeScript** | TS 7（类型检查） | AI 时代的第一语言：模型语料最多、类型反馈最好 |
| 脚本转译 | **oxc**，进程内转译并产出 source map | 0.152 | 无需 Node；毫秒级；错误与断点都映射回 TS 行 |
| 脚本 VM | **QuickJS-ng**，经 `rquickjs`，vendored 并打补丁 | 0.16.2 / rquickjs 0.14.0 | 小型解释器，所有平台同一份，确定；补丁见 4.2 |
| 脚本类型 | Rust 组件注册表 → `.d.ts` 与 JSON Schema | ts-rs 12、schemars 1.2 | 改了引擎类型，agent 与脚本看到的接口自动同步 |
| 渲染 | **wgpu + WGSL** | 30.0.1 | 一套渲染器覆盖 Metal、Vulkan、Direct3D 12 与浏览器 WebGPU |
| 原生图形后端 | **Metal**（macOS）、**Vulkan**（Linux/Windows；macOS 上经 MoltenVK 验证）、**Direct3D 12**（Windows，Pioneer 2026-10-09） | — | 所有者 2026-10-04 指定只做 Metal 与 Vulkan；Pioneer 2026-10-09 应所有者的探索要求加入 Direct3D 12；所有者 2026-10-09 指定 Windows 默认后端为 Direct3D 12（见 4.4） |
| Web 图形 | WebGPU（wgpu 的浏览器后端，同一份代码） | — | 证明 web 渲染；不做 WebGL 回退 |
| 窗口与输入 | winit；gilrs（手柄） | 0.30 | 事实标准 |
| 资源 | glTF 2.0（`gltf`）、PNG/JPEG（`image`）、网格处理（`meshopt`：LOD、顶点缓存）；导入在工作线程异步进行；内容哈希作为 ID | gltf 1.4、meshopt 0.6 | Amoris Pioneer 的 OBJ 导入冻结 14–27 秒，本线只走 glTF 且异步 |
| 神经渲染 | 3D Gaussian Splatting（GPU 排序、与网格深度混合）；神经纹理压缩（潜变量网格 + 小型 MLP，在片元着色器里解码） | 自研 | 见 4.4 |
| 音频 | kira（cpal；web 上为 WebAudio） | 0.12 | 混音、空间音频、补间 |
| agent 接口 | **CLI 优先**：一张命令目录 → `pocket` CLI（连接运行中的宿主，紧凑文本输出，`--json` 精确输出，帮助文本来自目录）、编辑器 WebSocket、脚本 API、测试；MCP（`rmcp`）是同一目录的薄投影 | rmcp 3.5 | agent 在 shell 里最顺手、最省 token；目录唯一，所有前端不漂移 |
| 服务端 | axum + tokio（只在 presenter 一侧） | axum 0.8 | 游戏侧 crate 禁止依赖 tokio |
| 编辑器 | **TypeScript + React + Vite**；Electron 桌面窗口与 Web 共用 dockview、Monaco 和 wasm/WebGPU 视口，经 WebSocket 与宿主同步 | React 19、Vite 8、Electron 44 | 见 4.5 |
| 调试 | 调试核心在 `pocket-debug`（QuickJS-ng PR #1421 的 trace 钩子）；前端：CDP 端点（Chrome DevTools、VS Code js-debug）、编辑器、MCP 工具 | 自研 | 见 4.3 |
| 性能分析 | 内建分析器：每个系统的 CPU 区段 + 每个渲染通道的 GPU 时间戳，推送到编辑器；Tracy 为可选特性 | tracy-client 0.19 | 测量为依据 |
| 离线工具 | **Python 3.14 + uv**：神经资源训练（PyTorch，Apple MPS；神经纹理例外，2026-10-09 Pioneer 起在引擎的 wgpu 计算着色器里训练，见 4.4）、评测 harness、分析脚本 | — | 只在工具层，不嵌入运行时 |

### 4.2 脚本：TypeScript on QuickJS-ng

- 脚本是数据导向的**无状态系统**：声明查询，按列（typed array）批量读写组件；组件可以在 TS 里
  声明（字段类型、默认值、文档），与引擎组件同为一等公民。写入在 tick 内暂存，整体校验后提交。
- 转译：oxc 在进程内去类型并产出 source map；类型检查交给 TypeScript 7（`tsc --noEmit`），只用于
  反馈，从不在运行时执行。
- QuickJS-ng 0.16.2 以 vendored 的 `rquickjs-sys` 0.14.0 构建，带：
  - **PR #1421**（`JS_SetDebugTraceHandler`、`OP_debug`、局部变量与帧上求值）：断点调试的基础。
  - **P1–P8**：确定的中断计数预算、不可捕获的栈溢出与 OOM、常量哈希种子、`-ffp-contract=off`、
    每上下文调用深度上限、typed array 的规范 NaN、构建修正、失败调用丢弃挂起的 promise 任务。
  - **P9–P10**（调试器，2026-10-04）：被跟踪函数的语句位置缓存（插桩后每条语句的开销从约 16 ns
    降到约 4 ns）；异常进入 trace 回调并附带是否会被捕获的预测，以及任意栈帧的位置查询。
  - **P11**（2026-10-10，采用 Amoris Pioneer 的调试修复）：帧上求值按暂停语句所在的
    lexical scope 解析名字。编译器记录插桩语句的作用域，不改变保留在字节码中的操作码编号；
    解决块内后声明的 `let`/`const` 无法求值以及 sibling block 局部变量外露的问题。
- 预算：每系统 100 万步、每 tick 200 万步、64 MiB 内存；步数不进入世界哈希。
- 性能定位：解释执行比同等 Rust 规则慢 21–27 倍（Amoris Pioneer 的 script-native spike）。对策是让脚
  本做编排、让引擎做重活：物理、动画、寻路、感知、渲染都在 Rust 里，脚本经批量列接口调用。与 Godot
  的 GDScript 处在同一量级，与 Unity C# 有差距；JIT 后端作为测量实验列入排期，不进入本轮默认路径。
- 被否决：V8（Amoris Pioneer master：预编译库与 Chromium libc++ 冲突、源码构建数小时）、
  JavaScriptCore（Windows 上要封装 46 MB 的 Bun DLL）、Lua 5.4（所有者搁置）、Luau、WASM 脚本。

### 4.3 调试

一个调试核心，三个前端：

| 前端 | 使用者 | 协议 |
|---|---|---|
| Chrome DevTools、VS Code（js-debug attach） | 人类 | CDP（Chrome DevTools Protocol）端点，127.0.0.1；TS 源码映射由客户端按 source map 完成 |
| 编辑器（Monaco 的断点栏、调用栈、变量、监视、控制台求值） | 人类 | 同一个 CDP，经 WebSocket |
| MCP 工具：`debug.*` | agent | 结构化 JSON：暂停时的调用栈、局部变量、组件、事件 |

- 断点在游戏线程的 trace 回调里暂停；渲染与服务线程继续工作，暂停期间编辑器仍可观察世界。
- 只有调试器连接时才插桩：连接与断开时在 tick 边界重编译脚本（脚本无状态，热更新使之廉价）；
  未连接时零开销。
- 引擎级调试（agent 原生）：组件字段的**数据断点**（某字段被哪个系统改写时暂停）、`events.why`
  因果链、tick 环形快照与**时间回溯**（恢复到任意 tick 重放到断点）、两次运行的**首个分叉 tick**。

### 4.4 渲染

2026-10-10，所有者授权 Amoris 采用以下 Pioneer 最终实现与测量后的默认值，源版本为
`9a3e4b0814959629202629845b85205c42086505`。历史 Windows 数字保持原平台与日期标注；
当前 Amoris 在 Mac/Metal 上单独验证。mesh-shader 独立原型未导入；源码和结果的来源索引随整合保存。


- 2026-10-05，所有者接受 Metal 分支的 GI 实施顺序：先交付世界空间探针烘焙，再实现
  SHaRC 辐亮度缓存并测量优化，随后探索光追与 ReSTIR GI/PT，最后训练小网络验证神经光照缓存。
  每阶段保留可运行入口、效果对照、资源与耗时记录；GPU 缓存为渲染派生数据，不进入模拟状态。
  探针烘焙先使用 Rust 离线三角形追踪；Metal 光追以 wgpu 的实验 ray query 做独立验证与后续接入。
  先完成同一场景的端到端闭环，再扩大内容和材质覆盖；尚未证明的能力明确记录。
- 同日，所有者要求下一轮完成并优化标准表面路径追踪：覆盖引擎的材质、贴图、光源、环境与
  多次反弹采样；在此基线上实现小网络在线 NRC。NRC 明确是单台 M5 上的 toy，要求真实样本、
  持续 GPU 训练、渲染时查询及光照变化响应，不要求生产级实时预算。既有烘焙和研究工具继续可用。
  已交付独立的静态 Metal 表面 PT 与在线 NRC 实验入口；统一读回减少了测得的批次耗时，
  NRC 学习链路成立但仍有偏差、尚未加速渲染。契约见
  [表面 PT 与在线 NRC](spec/path-tracing-nrc.md)，数据见 [测量记录](bench/path-tracing-nrc.md)。
- 2026-10-09（Pioneer）：所有者要求 Pioneer 自主探索 DirectX、WebGPU、3DGS 等方向。Direct3D 12
  加入为 Windows 上与 Metal、Vulkan 并列的原生后端：打开 wgpu 的 `dx12` 特性，DXC 在运行时从
  Windows SDK 加载（不用会在构建时下载二进制的 `static-dxc`），`POCKET_BACKEND=dx12` 选择它，
  `POCKET_ADAPTER` 在双 GPU 的笔记本上选适配器。理由：同一份 wgpu 代码只需开一个特性；Chrome
  的 Dawn 在 Windows 上也走 Direct3D 12，原生与浏览器同一底层便于对照；本机早先的渲染 spike
  （`docs/spikes/render.md`）在 Radeon 780M 上测得 Vulkan 比 Direct3D 12 慢 25–53%。Windows 的
  `Auto` 后端由测量决定：Direct3D 12 至少同样快且画面一致才取代 Vulkan。测量与结论见
  [bench/dx12.md](bench/dx12.md)。光线追踪与浏览器不在此决定内。
- 2026-10-09（所有者决定，取代上一条"由测量决定"的规则）：Windows 的 `Auto` 后端改为 Direct3D
  12，Vulkan 仍可用 `POCKET_BACKEND=vulkan` 选择，macOS 仍为 Metal，其他平台仍为 Vulkan。决定前的安
  静环境重测（分支 `explore/quiet`，`docs/bench/quiet-2026-10-09.md`）：两种后端画面一致；RTX 5060
  上 Direct3D 12 在 GPU 密集的 dense 场景慢约 24%，渲染器启动多约 0.9 s （DXC 编译管线），其余场景持
  平；Radeon 780M 上 Direct3D 12 快 12%–30%。这些代价由 Pioneer 继续优化（启动、dense 场景的 GPU 时
  间、每帧 CPU 编码），测量记入 [bench/dx12.md](bench/dx12.md)。同日进展
  （Pioneer，`explore/d3d12perf`，单代理测量）：渲染器的管线改为多线程并发创建（浏览器中仍按顺
  序），RTX 5060 上 Direct3D 12 的 `Renderer::new` 从约 0.93 s 降到约 0.18 s（Vulkan 从约 0.10 s 降
  到约 0.05 s）；前向着色器只在管线拥有运动与法线目标时才计算这两项（间接光占比始终计算：省去它时
  NVIDIA 的 Direct3D 12 编译器对颜色本身的舍入不同），离屏 dense 场景的 GPU 时间从 18.2 ms 降到 16.8
  ms（比 Vulkan 多 14%，原为 24%），窗口模式下未见变化；每帧 CPU 编码在两种后端上都降低（RTX 5060 的
  Direct3D 12 上 28% 至 39%）。画面与改动前逐像素比较（六个场景、四种抗锯齿与 GTAO 模式、两种后端、
  两块 GPU，共 72 对）全部完全相同。设备创建约 1.1 s 来自驱动，未改变。见
  [bench/dx12.md](bench/dx12.md) 第 10 节。2026-10-10 安静环境在最终构建上重测（Pioneer，只记录，不
  改变所有者的决定）：RTX 5060 上 Direct3D 12 在离屏轻场景的 GPU 时间比 Vulkan 多 2%–16%（球体 2%，
  点云花园 9%，蒙皮人群 16%，人群的帧时间多 45%），窗口模式下球体的帧时间少 10%；离屏 dense 场景 GPU
  时间多 14%，窗口模式多 44%（Vulkan 的窗口帧在最终构建上便宜了 11%，Direct3D 12 的没有变化），每帧
  CPU 编码是 Vulkan 的 1.6–2.3 倍，`Renderer::new` 约 0.18 s；Radeon 780M 上 Direct3D 12 在
  many_cubes 与点云花园上快 4%–30%，蒙皮人群则慢（GPU 时间多 9%，离屏帧时间多 19%）。见
  [bench/dx12.md](bench/dx12.md) 第 11 节。
- 2026-10-09（Pioneer）：光追研究从 Metal 扩展到所有暴露 ray query 的后端。2026-10-05 的 GI
  决定以 Metal ray query 表述，表面 PT、SHaRC/ReSTIR 原型与在线 NRC 也只在 Metal 上运行；
  现在它们在 Vulkan（`VK_KHR_ray_query`）与 Direct3D 12（DXR 1.1、Shader Model 6.5）上同样运行，
  门槛是适配器暴露 wgpu 的 `EXPERIMENTAL_RAY_QUERY`，而不是后端名称；没有该特性的适配器与
  浏览器 WebGPU 明确拒绝。研究器材仍是独立设备上的独立工具，不改变交互渲染器与 WebGPU 基线。
  理由：所有者要求自主探索 DirectX；wgpu 30 在这两个后端上提供同一个 ray query 接口，同一份
  WGSL 经 naga 生成 MSL、SPIR-V 与 HLSL，跨后端运行能暴露着色器翻译与驱动的差异；本机的
  RTX 5060 支持两者，能在同一块 GPU 上对照 Vulkan 与 Direct3D 12 的光追开销与画面，M5 的 Metal
  结果保留为参考。规格与测量见 [表面 PT 与在线 NRC](spec/path-tracing-nrc.md)、
  [GI 实现](spec/metal-gi.md) 及其测量记录的 Windows 章节。
- 2026-10-09（Pioneer）：交互渲染器加入第一个光追效果，默认关闭：光追太阳阴影，
  `POCKET_RT_SHADOWS=1` 开启。只有开启且原生适配器暴露 ray query 时，主设备才请求
  `EXPERIMENTAL_RAY_QUERY`；渲染器为网格建 BLAS（蒙皮网格每帧重建），按插值后的位姿重建 TLAS，前向通
  道的片元着色器向太阳发一条 inline ray query 阴影射线，代替级联阴影贴图（开启时不再渲染级联）。
  WebGPU、基线绘制路径与默认的原生路径不变。理由：前向渲染器没有 G-buffer 与深度预通道，计算通道无法
  在着色之前给出可见性，inline ray query 是改动最小的接入方式；先测量它相对级联阴影的开销与画面差
  异，再决定是否扩展到软阴影、AO 或 GI。规格见 [光追阴影](spec/rt-shadows.md)。暂定测量（2026-10-09
  用提交的构建重测；最初的数字来自掩码投影体尚未单独建 BLAS 的中间构建，已作废）：小场景里与四个级联
  通道的开销相当，Vulkan 上差别在轮次波动之内（RTX 5060 −5% 至 +12%，Radeon 780M −7% 至
  +7%），Direct3D 12 上两块 GPU 都便宜 14% 至 20%；可见实例多时更贵（1 万立方体 +22% 至 +37%，20 万
  立方体 +50% 至 +57%；wgpu 每次提交逐实例校验 TLAS 依赖，静态场景也要约 2.8 ms CPU）；与级联阴影的
  阴影区域交并比 0.91 至 0.94，差别在边缘，集成测试另外核对掩码镂空透光、移动实例与蒙皮姿态变化后阴
  影随之变化。见 [测量记录](bench/rt-shadows.md)。2026-10-09 安静环境重测（`ef35e1af`，各适配器的默
  认设置，即 GTAO 与遮挡剔除的自动模式都已开启；只有 mixed 场景跑了三轮，其余配置各只有一次）：
  Direct3D 12 上小场景里光追阴影仍便宜（RTX 5060 7%–9%，780M 18%），实例多时贵 13%–35%；780M 的
  Vulkan 上便宜 22%–26%；RTX 5060 的 Vulkan 上则贵 33%–72%（20 万个移动立方体的单次运行贵 155%），部
  分来自不追踪光线的通道（GTAO、Hi-Z）在带 ray query 的设备上变慢。2026-10-10 在最终构建
  `0871920d` 上每种配置各测五轮（启动后等待 5 秒，每轮前 RTX 5060 冷却到 70 C 以下），取代上述数字：
  Direct3D 12 上小场景便宜 8%–10%（RTX 5060）与 19%（780M），1 万与 20 万立方体贵 15% 与 43%；
  RTX 5060 的 Vulkan 上小场景只贵 3%–6%，1 万与 20 万立方体贵 22% 与 46%，20 万个移动立方体贵 18%；
  780M 的 Vulkan 上便宜 25%；不追踪光线的通道不再变慢。RTX 5060 Vulkan 上的变化来自两个构建之间的差
  别（最终构建按第一次的方式不等待启动时也是 −5% 至 +4%），具体是哪项改动未查。默认仍关闭。

- 一套 wgpu 渲染器，WGSL 着色器放在独立的 `.wgsl` 文件里（不放进 Rust 字符串），按通道组织模块。
- **渲染图**：每帧由通道组成的有向无环图，资源（瞬态纹理与缓冲）由图分配与复用。
- **GPU 驱动**：全部实例数据在存储缓冲里；计算着色器做视锥剔除（与上一帧 Hi-Z 遮挡剔除），把
  可见实例压缩并写出间接绘制参数；每个网格一次 `draw_indexed_indirect`，绘制调用数与实例数无关。
  2026-10-09（Pioneer）：遮挡剔除定为两阶段 Hi-Z：第一阶段只画上一帧可见且在视锥内的实例，用这些
  实例的深度建 Hi-Z 金字塔（计算着色器，只用 WebGPU 核心特性：读多重采样深度、r32float 各级、
  非 2 的幂尺寸、reversed-Z 取最小值），第二阶段以本帧金字塔测试其余实例并补画新可见者，同时记下
  下一帧的可见集；阴影级联仍只做视锥剔除。理由：直接拿上一帧金字塔重投影，在镜头移动与去遮挡时
  会漏画可见物体；两阶段只用本帧真实画出的深度作保守判断，首帧、改尺寸、镜头切换都无需特判，
  可见集只影响快慢、不影响画面。第二阶段多一次剔除派发、建金字塔与一次渲染通道，场景几乎无遮挡时
  得不偿失，故另有自动模式：GPU 统计第二阶段剔除掉的三角形数，与第二阶段的代价比较，代价也按
  三角形计，为固定的 200,000 加视锥内每个实例 0.5（在 RTX 5060 上用 many_cubes 标定）；试探须
  超出代价的 1.5 倍才开启，开启后连续三次读数抵不上代价即关闭；关闭后定期试探，间隔从 120 帧起，
  每次试探失败加倍，至多 960 帧；首次试探保持两阶段直到读数返回，此后的试探只跑两帧。
  `POCKET_OCCLUSION`（`on`/`off`/`auto`）与渲染器选项可强制开关。进度表所记的 `feat/hzb` 分支已
  遗失，此为重新实现。规格见 [occlusion.md](spec/occlusion.md)。同日更正：自动模式起初按被遮挡
  三角形的数量与占视锥内三角形的比例决定开关，在测量之前改为上述代价模型。理由：比例与收益无关，
  一千万三角形的场景遮挡掉 10% 已远超第二阶段的代价，比例规则却会关掉它；而且第二阶段的剔除代价
  随视锥内的实例数增长，比例不反映这一点。
  2026-10-09（Pioneer）：网格 LOD。生成：导入 glTF 与生成内置几何体时，用 meshopt 从原网格逐级简化
  （每级目标约为上一级三角形数的一半，记录网格单位下的绝对几何误差），并做顶点缓存与顶点读取优化；
  各级是同一顶点缓冲上的索引列表（`MeshData.lods`）。原生与浏览器走同一份导入代码：meshopt 的 C++
  原生由 clang-cl（或 MSVC）编译，wasm32 由固定版本的 clang 配 meshopt 自带的极简头文件编译，C++ 的
  `operator new`/`delete` 由 Rust 提供。理由：浏览器视口本来就在 wasm 里导入 `.glb`，引擎还没有
  烘焙资源格式；打包时生成再随包分发，需要新增一种资源格式，并改动宿主的资源服务、打包工具与视口的
  取资源流程；在导入时生成，代价只是一次导入耗时，在浏览器里也可测量和接受（见测量记录）。这改变了
  `docs/spec/architecture.md` 4.2"导入器只在原生"的写法，该节随之改写。选择：剔除着色器按实例、按
  视图选级。相机视图取投影屏幕误差不超过阈值（默认 1 像素；误差乘以实例最大缩放、每米像素数，除以
  到包围球近点的距离）的最粗级别，带迟滞（变粗须再留 20% 余量，变细立即），上一帧的级别记在每实例
  状态字里；阴影级联按级联纹素的世界尺寸选级（误差不超过一个纹素），与相机距离无关、通常更粗。绘制：
  每个层级是网格表中的一行，批次仍是（变体、网格行），所以三条绘制路径、两阶段 Hi-Z、ID 通道与级联
  的发射方式都不变；代价是带层级的网格在每个视图的列表里每一级都预留全部实例的区域（`drawn` 与
  `visible` 按层级数成倍）。理由：这是对现有批次结构改动最小、在 WebGPU 基线路径上也成立的做法；
  消除成倍内存需要 GPU 前缀和压缩与顶点阶段从存储缓冲读基址，列为后续，先测量。`POCKET_LOD=off`、
  渲染器选项与视口的 `?lod=off` 强制 0 级。规格见 [lod.md](spec/lod.md)。
  2026-10-10（Pioneer）：不透明前向通道加入深度预通道（Z-prepass）。先只画深度：所有不透明实例按同
  一批间接绘制写入 reversed-Z 深度（alpha 测试的材质做与着色时相同的 alpha 测试，双面材质不剔背面，
  神经纹理变体按其基础变体，LOD 级别与蒙皮部件照常）；再画颜色：深度测试为相等、不写深度，只为最终可
  见的样本着色，alpha 测试的材质在这一遍不再丢弃片元（可见与否已由深度决定，两遍的 alpha 测试不可能
  不一致而留下空洞）。两遍的顶点位置由 WGSL 的 `@invariant` 保证逐位相同（naga 输出为 HLSL 的
  `precise`、SPIR-V 的 `Invariant` 修饰、MSL 的 `invariant`），两遍用同一个抖动后的矩阵，三条绘制路
  径与 MSAA 都适用。两阶段 Hi-Z 下每个阶段各自先画深度再画颜色：金字塔仍由第一阶段的深度建成，剔除结
  果与可见集不变。模式为 off/on/auto，由 `POCKET_PREPASS`、渲染器选项与视口页的 `?prepass=` 设
  定；auto 按测量的代价决定：用 GPU 时间戳逐帧交替测量有无预通道时不透明通道（含预通道）的耗时，另一
  种便宜 5% 以上才切换，之后定期复测（结论不变则间隔加倍），单阶段与两阶段的帧分别决定；没有时间戳时
  不开。默认模式由测量决定，记入 [bench/prepass.md](bench/prepass.md)。理由：与 UE 5.7 的对照（分支
  `explore/ue5`）中，dense 场景关闭遮挡剔除时我们的帧时间是 UE 的 2 到 3 倍，差别正是 UE 前向渲染器
  的深度预通道（只画深度约 12 ms，着色约 2.7 ms）：我们按绘制顺序着色，为视锥内约 48 万个立方体（可
  见的只有 443 个）的过度绘制付费。遮挡剔除按实例剔除，覆盖不到 auto 关闭、首帧、去遮挡以及可见物体
  之间的过度绘制；预通道补上这些情形，代价是多一遍几何与深度的读写，所以何时开启由测量决定。规格见
  [prepass.md](spec/prepass.md)。
  同日测量后（Pioneer，暂定：另一个代理同时使用机器，每次计时持 GPU 锁，三轮交错）：默认取
  auto。RTX 5060 上 2560x1440、4x MSAA 的 dense 场景关闭遮挡剔除时，开启预通道使离屏 GPU 时间
  Direct3D 12 由 51.3 ms 降到 16.2 ms，Vulkan 由 42.5 ms 降到 16.6 ms（窗口帧时间比 UE 5.7 多 4% 与
  2%，此前是其 3 倍与 2 倍），Radeon 780M 由约 298 ms 降到约 96 ms；开启遮挡剔除（auto）时第一阶段画
  的可见方块彼此重叠，预通道同样省下约三分之一（Direct3D 12 由 2.33 ms 降到 1.44 ms），窗口帧时间为
  UE 的 0.73 倍（Direct3D 12）与 0.53 倍（Vulkan）；球体场景的方块几乎不重叠，强制开启多花 19% 至
  35%，auto 不开。auto 与较好的固定模式相差 0 到 1.2 ms（测量窗口内的复测帧）。`@invariant` 的代价在
  轮次误差之内。画面在两块 GPU、两种后端与 WebGPU 基线设备上与关闭预通道时逐像素相同，只有球体场景有
  1 到 2 个孤立像素差 1 级（两个面在该样本的深度恰好相等，预通道下后画者胜出）。见
  [bench/prepass.md](bench/prepass.md)。
  同日复审后（Pioneer）：auto 的探测只采用它自己的帧的读数。每个读数带帧序号，探测只计入它开始之后、
  有实例的同类帧（单阶段或两阶段），没有实例的帧（加载时只画天空）和尺寸或格式改变之前的帧都不参与决
  定；超时按所有帧计（600 帧），探测用到的读数相隔不超过它。探测先画当前的方式（首次探测先画预通道：
  它至多多一遍几何，另一种方式的代价没有上限）直到有 4 个读数，再一帧一帧地画另一种方式：只在会被计
  时的帧里画，等上一帧的读数到了才画下一帧，每次探测至多 8 帧。仍是挑战者便宜 5% 才切换（首次探测的
  在位者是不开）；读数越多，后画方式的最小值只会更低，所以它一旦胜出即为定论，而它有一个读数比先画方
  式的最小值高出 50% 以上即落败，否则 4 个读数后落败。尺寸或格式改变时保留当前选择，立即重新探测。理
  由：复审发现读数不带帧号，加载时的空帧（0.1 ms）被当作不开预通道的样本，dense 场景的首次决定因此为
  关，此后 240 帧 GPU 时间约为三倍；每次复测连续 5 帧画较慢的方式（读回延迟 2 到 3 帧期间仍按读数少
  的一方画），dense 场景约每 4 秒出现一次三倍帧时间的卡顿，780M 上约 1.5 秒。
  修正后测量（Pioneer，暂定：三轮交错，每轮持 GPU 锁，与修正前的构建同轮对照）：dense 场景每次复测只
  有 1 帧不开预通道（RTX 5060 上 Direct3D 12 该帧 44 至 49 ms，修正前连续 5 帧、每帧 54 至 61
  ms；780M 上 1 帧约 0.25 s，修正前共约 1.2 s），离屏帧时间的 p99 在 RTX 5060 上减半，auto 的 GPU 时
  间与常开预通道相差约 0.1 ms（Direct3D 12 与 780M 按中位数，Vulkan 按最小值；修正前 Direct3D 12 上
  1.1 ms，780M 上 3.6 ms）；球体场景的复测一帧一帧地画预通道，至多 4 帧，每帧约多 2 ms。见
  [bench/prepass.md](bench/prepass.md) 3.1。
- **光照与画质**：reversed-Z、PBR（GGX）、聚簇前向光照、级联阴影、程序天空与 IBL、HDR、bloom、
  TAA、AgX 色调映射、实体 ID 缓冲（拾取与 agent 的"画面上是什么"）。 2026-10-09（Pioneer）：TAA 与
  GTAO 在帧内的位置。开启 TAA 时不透明通道用逐帧亚像素抖动的投影绘制；前向着色器另写两张附加目标：间
  接光在像素颜色中的占比（GTAO 用）与物体自身的运动（TAA 用，静止处为零；摄像机运动由深度与前后两帧
  的矩阵重建，蒙皮网格取上一帧的蒙皮顶点）。解析之后依次是：GTAO 在半分辨率上由深度计算，经深度感知
  上采样后只乘到间接光上（天空 IBL、球谐与烘焙 GI 的漫反射和镜面），直接光与自发光不变；TAA 按运动向
  量重投影历史、在 YCoCg 中做方差裁剪、按深度判断去遮挡，输出 HDR 图像；之后才绘制 splat（不抖动，它
  们没有运动向量，本身已平滑），再做 bloom、AgX 与可选的锐化。实体 ID 通道、编辑器覆盖层、Gizmo、地
  面网格与游戏 UI 一律用不抖动的矩阵；改尺寸、场景重置与镜头切换清空历史。TAA 不预设取代 MSAA：多重
  采样数（1 或 4）、 TAA 与 GTAO 是三个独立的渲染器选项（`POCKET_AA`、`POCKET_GTAO`），所有组合都可
  运行；默认值由测量决定：在 RTX 5060 与 Radeon 780M 的 Vulkan、Direct3D 12 及 Chrome（只用 WebGPU
  核心特性）上测各组合的开销，以静态场景对超采样参考的 PSNR 随帧数的收敛与运动场景的拖影检查测画质，
  画质相当时取便宜者。理由：前向渲染器没有 G-buffer 与深度预通道，间接光占比只需一张 8 位附加目标，
  就能让 AO 只压暗间接光而不多画一遍几何；摄像机运动可由深度得到，附加目标只存物体运动，静止物体处恰
  为零，多重采样解析不会把它混坏；MSAA 已经在帧内，TAA 是取代还是叠加应由画质与开销的数据决定。进度
  表所记的 `feat/gtao` 分支已遗失，此为重新实现。规格见 [TAA 与 GTAO](spec/taa-gtao.md)。同日补充
  （Pioneer，测量之后）：默认值按适配器类别取测量结果（`post::defaults_for`）。Vulkan 与 Direct3D 12
  上的独立显卡保持 4x MSAA，并默认开启 GTAO；集成显卡默认单采样加 TAA，不开 GTAO；未测量的 Apple
  GPU（tile 架构，MSAA 本就便宜）与浏览器（无法得知适配器类别，Chrome 在 Radeon 780M 上的时间戳不可
  用）保持 MSAA、不开 GTAO。`POCKET_AA`、`POCKET_GTAO` 与渲染器选项覆盖默认，跨适配器比较画面的工具
  固定这两项。理由：没有一种模式处处更好。对 256 帧超采样参考，静止画面、 alpha 测试的边缘与镜面高光
  上 TAA 高 5 到 7 dB；摄像机平移时 MSAA 略好（PSNR 高零点几 dB，帧间稳定度高 1 到 2 dB），亚像素细
  线也只有 MSAA 每帧都画得出。成本则因适配器而异：RTX 5060 上单采样加 TAA 与 4x MSAA 相差在 0.45 ms
  以内（轻场景 TAA 贵 0.09 到 0.43 ms，160 万立方体便宜 0.06 到 0.17 ms），故保留在运动中更好的
  MSAA，GTAO 在 1600x900 只要约 0.15 ms；Radeon 780M 上 4x MSAA 在 Vulkan 下比 TAA 贵 1.6 到 4.2
  ms，在 Direct3D 12 下轻场景相当、160 万立方体贵 2 ms，在画质相当时取便宜者，而 GTAO 在那里要 0.5
  到 0.9 ms。测量与数据见 [bench/taa-gtao.md](bench/taa-gtao.md)。同日再补（Pioneer，评审之后）：三
  处修正。其一，开启 TAA 时 splat 不再对抖动后的深度做测试：渲染器为它们另画一遍不抖动的深度（只写深
  度，含 alpha 测试的材质与海面），splat 的两种光栅化都对这张深度测试。理由：splat 画在 TAA 之后、不
  进历史，对抖动深度测试时网格遮住 splat 的边缘随抖动逐帧跳动（静止画面上轮廓两像素内每帧有数百像素
  闪烁），而单帧抖动深度里取不出不抖动的边缘；这遍深度只在 TAA 与 splat 同时开启时绘制。其二，一次性
  的离屏截图（MCP 与 CLI 的 `capture`）在 TAA 下从清空的历史起连画 16 帧（一个抖动周期）再读回：TAA
  的第一帧只是一个抖动样本，PSNR 比 MSAA 低 4 到 5 dB，而集成显卡默认用 TAA。其三，默认值另按厂商与
  驱动认出 Apple：Apple GPU 与经 MoltenVK 的 Vulkan（任何 GPU，未测量）保持 MSAA、不开 GTAO，与上文
  对未测量的 Apple GPU 的规定一致；此前它们按适配器类别取到 TAA 或 GTAO（Apple Silicon 报告为集成显
  卡）。
- **神经渲染**：
  - 3D Gaussian Splatting：预处理计算着色器（投影、剔除、球谐取色）、GPU 基数排序、实例化四边形混
    合，与网格深度缓冲合成，即混合网格与 splat 的场景。 2026-10-09（Pioneer）：另有可选的计算着色器
    分块光栅化，与实例化四边形并存，默认仍为四边形。它把可见 splat 按 16x16 像素分块分箱，稳定排序后
    每个分块保持由前到后的深度序，逐块由前向后混合，像素饱和或到达网格深度即停止，只用 WebGPU 核心特
    性。理由：四边形路径受填充率限制（M5 上 3M splat 的绘制 21–30 ms），分块是参考实现的做法；在 RTX
    5060 上两者互有胜负（3M 与 2560x1440、近景时分块快 12–20%，1M/2560x1440 时快 3–13%，3M/1600x900
    时持平， 1M/1600x900 时四边形快约 10%，抗锯齿模式与远景时四边形快 20–90%；除远景外分块的光栅本身
    都比四边形绘制快，差距来自分箱与第二次排序。同日更正：先前一组测量误用了关闭子块剔除的着色器），
    默认值待参考机测量后再定（见 `docs/spec/splats.md` 4.2 与 `docs/bench/splats.md`）。
    2026-10-10（Pioneer，安静环境重测之后）：默认光栅化由测量决定，所有适配器都用实例化四边形
    （`SplatRaster::DEFAULT`），分块光栅化保留为可选（`POCKET_SPLAT_RASTER=tile`）。理由：在 RTX
    5060 与 Radeon 780M、Direct3D 12 与 Vulkan 上五轮交错测量 1M 与 3M 花园（1600x900 与
    2560x1440）、球谐近景、抗锯齿模式与 2.5 倍距离：花园的默认视图（1M、1600x900）上分块的 GPU 时间
    是四边形的 1.09–1.47 倍，每种适配器与后端都更慢；抗锯齿与远景 1.24–2.06 倍；分块只在 3M 或
    2560x1440 与近景时更快（0.79–0.97，多在独立显卡的 Vulkan 上）。集成显卡上分箱与第二次排序很贵
    （1M 时约 7.6 ms），除 Direct3D 12 上 3M、2560x1440 一处（0.95）外处处更慢。没有一种适配器类别上
    分块明显占优，故不按适配器区分。测量中还发现分块的逐像素遍历在 NVIDIA 的 Direct3D 12 上慢 8 倍
    （循环里的 break/continue；1M 时 11.4 对 1.45 ms），改为由循环条件结束后与 Vulkan 相当，画面逐字
    节不变（Radeon 780M 上这一改动使 Direct3D 12 快 1.5 倍、Vulkan 慢 7%；同日复核的证据见
    `docs/evidence/quiet/splats/walk/`）。 Apple GPU（四边形的混合在其上最贵）未测量；若 M5 上分块明
    显更快，再为 Apple 另定默认值。见 [bench/splats.md](bench/splats.md) 的安静环境重测一节。
  - 神经纹理压缩：材质的多通道纹理压缩为低分辨率潜变量网格加一个小 MLP（Python/PyTorch 训练），
    在片元着色器里逐像素推理解码；对比未压缩与传统压缩的体积和质量。
    2026-10-09（Pioneer）：神经纹理的训练改在引擎自己的 wgpu 计算着色器里进行，不用 Python/PyTorch：
    离线编码器是 `pocket-render` 的一个 cargo 示例，手写前向与反向传播（与在线 NRC 同样不用浮点原子
    操作：潜变量梯度以定点整数原子累加，与顺序无关，MLP 梯度按固定顺序分块归约），同一种子的结果
    可复现。编码结果是带版本号、严格校验的二进制文件（`.ntex`），类型在 `pocket-assets`（游戏侧，
    可编译到 wasm）；`Model.material` 写 `.ntex` 路径即为神经材质，渲染输入流的格式不变。神经材质
    走自己的管线变体，其他材质的着色器与绘制不变；有 `SHADER_F16` 时以 f16 解码，否则 f32，
    WebGPU 基线与浏览器同样运行。理由：本机没有 PyTorch（CUDA 版约 3 GB 下载）；训练与运行时解码
    共用同一套 WGSL 约定，编码器在 Metal、Vulkan、Direct3D 12 上都能跑，不再需要另一套语言与
    运行环境；网络很小，手写反向传播可行（在线 NRC 已验证这种做法）。BCn 对照用 crates.io 的
    CPU 编码器（只作工具层的开发依赖）。规格见 [神经纹理](spec/neural-textures.md)。
    暂定测量（2026-10-09，机器上同时有其他工作）：8 至 12 通道的材质 5.37 bit/texel（含全部
    mip，为 BC1/BC5/BC4 的 1/4 至 1/6）；四种程序材质的 mip 0 为 40 至 52 dB，七种材质中六种的
    每个通道组都高于半分辨率的 BCn，mip 2 低 5 至 11 dB；2560x1440 全屏解码在 RTX 5060 上 f16
    约 3 ms，Radeon 780M（Direct3D 12）上约 10 ms，Chrome 中与原生相当。见
    [测量记录](bench/neural-textures.md)。
- 浏览器 WebGPU 与原生 Metal/Vulkan/Direct3D 12 走同一份代码；只使用 WebGPU 默认可用的特性作为基线，
  原生可选特性（如 multi-draw indirect、时间戳查询）作为加速路径。

### 4.5 编辑器

- 形态：Web 应用（TypeScript + React + Vite），由宿主在 127.0.0.1 上提供；桌面版使用 Electron
  打包相同界面。所有者 2026-10-05 授权桌面 GUI：选用随应用分发的 Chromium，固定 WebGPU
  运行环境；原生菜单、目录选择器和宿主生命周期由桌面主进程负责。此项替换此前待实施的 Tauri
  包装计划，不更改模拟、渲染器或命令接口。实现与验证见 [desktop.md](spec/desktop.md)。
- 视口：引擎渲染器编译到 wasm，在页面里用 WebGPU 绘制；场景由宿主经 WebSocket 推送快照与差量，
  资源经 HTTP 获取。原生窗口（Metal/Vulkan）用于独立运行与 Play。
- 权威：唯一权威是宿主进程里的世界。编辑器的每次修改都是一条命令，进入可撤销的事务历史；agent
  经 MCP 的修改进入同一历史。
- 面板：层级树、检视器（由 JSON Schema 生成）、视口（拾取、平移/旋转/缩放 Gizmo、吸附）、资源
  浏览器、脚本编辑器（Monaco，诊断与断点）、调试（调用栈、变量、监视）、控制台、事件与因果、
  时间线（tick 拨动、快照、回溯）、性能分析、agent 面板（agent 的操作流与对话）。
- 被否决：egui（所有者要求 TS + React）；把编辑器 UI 做成引擎自己的 UI 系统（Amoris Pioneer master
  的 Pocket UI，效果与工作量不划算）；视口按 PNG 轮询推帧（Amoris 原型，4 Hz）。

### 4.6 构建链

- Cargo workspace 加 `cargo xtask`（检查、代码生成、vendoring、web 打包）。不自研构建系统：
  Amoris Pioneer master 的 `pocket` 工具（Rust 写的第二套构建系统，嵌入 n2 后又换回 Ninja）在 Cargo
  之上重复了增量构建、特性与目标管理。
- TypeScript 包（`sdk/`、`editor/`）用 bun 安装与运行脚本，Vite 打包编辑器。
- web：`wasm32-unknown-unknown` + wasm-bindgen + wasm-opt；QuickJS-ng 的 C 代码用 wasi-libc 编译。

### 4.7 物理性能与后端

所有者要求物理性能比肩 Unity（PhysX）与 UE5（Chaos）。Rapier 在确定性配置下关闭了 SIMD 与并行，
吞吐量是风险点。做法：在同一台机器上用同一组场景（箱子金字塔、大量球体、布娃娃、射线查询）测量
Rapier（确定性配置、SIMD+并行配置）与 Jolt（原生 C++ 构建，其跨平台确定性选项开与关），据此决定：

- Rapier 足够：保持单一后端；
- Jolt 显著更快：`pocket-physics` 抽象出后端接口，原生默认 Jolt（经 C 封装），web 与确定性检查
  继续用 Rapier，或以 Jolt 的 wasm 构建统一。

### 4.8 2026-10-10 的 Amoris 平台复核

本次整合保留 Pioneer 的 Windows 测量及其原时间，但在 Amoris 的 M5/Metal 上重新验证实现。
LOD 的径向距离不能保证离轴透视投影的像素误差；整合时改为包围球的最近轴向深度与投影导数
上界，保持 1 像素设定与 20% 误差迟滞，不放宽轮廓测试。splat tile 的共享向量分量写入存在
数据竞争，改为同布局的独立标量字段，消除 Metal 上的闪烁。当前 wgpu 30.0.1 的 Metal
加速结构 barrier 没有实现（[wgpu #9215](https://github.com/gfx-rs/wgpu/issues/9215)），
光追阴影在该后端按已完成的构建提交排序；光追片元着色器与 4 倍 MSAA 的组合在 M5 上持续
黑帧，因此该路径使用单采样，可保留 TAA。普通 Metal 渲染仍使用原采样设定。仅在修复后的
wgpu 通过本机冷启动、移动、蒙皮与多采样回归后移除这些约束。平台修复和实际验证另记于导入说明。

## 5. 架构

### 5.1 crate

2026-10-10，Amoris 采用 Pioneer 的 CPU/runtime 先行成果（源快照
`qiulinfan/amoris-pioneer` 的 `9a3e4b0814959629202629845b85205c42086505`）：

- 原生玩家层由 `pocket-interface` 执行感知、投影、意图与时间控制，`pocket-runtime` 用
  `PlayerSpec` 把声明作为 persisted world state 保存，恢复后重建派生定义。观察者的 team vision
  分享当前可见集，每个 observer 的记忆、事件和测量仍独立；executor 只看该 seat 的感知，
  只写自己的控制 channel。session 的决策点、pacing 和 thinking clocks 不进入世界 hash。
- `Source::Player` 在声明玩家的游戏中只允许 `player.*`，由 runtime、thread、worker 和 host
  一同检查。CLI、MCP 与 HTTP 都可绑定 seat；这是可信本地客户端的角色限制，尚不包含 HTTP
  token grants 或远程身份鉴权。新原生玩家层尚无 LLM course 实测；旧 Python gateway 的模型
  结果不作为原生 seat 层的成绩。规格见 [player.md](spec/player.md)。
- 调试器暂停时的读取来自上一个已发布边界并标明 `paused_at`；step 在断点答复，Stop 可结束
  被断点暂停的 Play；snapshot restore 默认保留 applied scripts，并将 restore/swap 记录进
  replay。构建注入 source-based EngineVersion；Windows host 的启动、compiler 与 check 工具
  修复一同采用。此阶段不改变图形后端、渲染算法或 LOD/neural assets；采用后的检查独立记录。

游戏侧（无头、可编译到 `wasm32`，禁止依赖 wgpu、winit、tokio、rmcp）：

| crate | 职责 |
|---|---|
| `pocket-contract` | 错误协议 `Problem`、严格解码与"是不是想写"建议 |
| `pocket-sim` | ECS 世界、tick、调度、EntityId、事件、RNG、确定性数学、组件注册表、`Transform` |
| `pocket-assets` | 资源 ID、可视组件（模型、材质、灯光、相机、环境、splat）、glTF 导入与网格烘焙 |
| `pocket-persist` | 规范编码、哈希、snapshot/fork/replay、分叉定位、版本迁移 |
| `pocket-physics` | Rapier、浮力、风与帆、水动力 |
| `pocket-interface` | 感知、动作与意图、affordance、时间模型 |
| `pocket-script` | TypeScript on QuickJS-ng、沙箱、预算、热更新、调试钩子 |
| `pocket-link` | 游戏与 presenter 之间：命令信封、队列、发布的快照、事件环 |
| `pocket-runtime` | 游戏组合、命令目录、场景、游戏线程 |
| `pocket-check` | 确定性、fork、replay、重载等价检查 |

presenter 与工具侧：

| crate | 职责 |
|---|---|
| `pocket-render` | wgpu 渲染器（原生与 web），含 splat 与神经纹理 |
| `pocket-debug` | 调试核心与 CDP 端点 |
| `pocket-mcp` | MCP 服务器（rmcp） |
| `pocket-server` | axum：编辑器 WebSocket、资源 HTTP、MCP HTTP |
| `pocket-app` | 原生可执行文件 `pocket`：窗口、线程、`run`/`editor`/`mcp`/`check` |
| `pocket-web` | 浏览器入口：worker 里的游戏、WebGPU 渲染、编辑器视口 |

TypeScript：`sdk/`（脚本的 `pocket` 模块与生成的类型）、`editor/`（React 编辑器）。
Python：`tools/neural/`（神经资源训练）、`tools/eval/`（评测）。

### 5.2 线程（原生）

- **游戏线程**：在 tick 边界按规范顺序应用命令，运行系统（Rust 与 TS），发布快照与事件。
- **主线程（渲染）**：winit 事件循环；读取最新快照，插值，渲染。
- **服务线程**（tokio）：MCP、编辑器 WebSocket、CDP。
- **资源工作线程**：导入与烘焙。
- 断点暂停只阻塞游戏线程；其余线程照常。

### 5.3 web

- 游戏在 Web Worker 里跑同一个 wasm；页面主线程跑 WebGPU 渲染；快照以 transferable buffer 传递
  （Amoris Pioneer threads spike：10⁵ 实体 0.39 ms）。

## 6. 证明目标与测量

| 目标 | 产物 | 测量 |
|---|---|---|
| 性能 | `many_cubes` 等价场景、海岛展示场景 | 与 Bevy 0.19 `many_cubes`（同机）对照的帧时间、GPU 通道时间、每 tick 模拟时间、脚本系统时间 |
| agent-native gameplay | 帆船与竞技场两个示例游戏；MCP 玩家工具 | LLM 通过 MCP 完成任务；脚本决策模型的每秒决策数；fork 前瞻的收益 |
| web 与神经渲染 | 浏览器运行的同一场景；splat 场景；神经纹理材质 | 浏览器帧时间；splat 数量与帧时间；神经纹理的压缩比、PSNR、着色开销 |
| 编辑器 | 完整的 Web 编辑器 | 每个面板的可用性证据（截图），编辑-运行-调试的完整回路 |
| 调试 | CDP 端点、编辑器调试面板、MCP `debug.*` | 在真实 Chrome DevTools 与 VS Code 中命中 TS 断点；agent 用 MCP 定位一个预埋的缺陷 |

基线（2026-10-04，Apple M5，Bevy 0.19.0 `many_cubes --benchmark`，160 万立方体，release）：sphere
布局约 10.4 ms/帧，dense 布局约 13.0 ms/帧。

## 7. 来源与取舍

| 来源 | 取用 | 不取 |
|---|---|---|
| `Amoris Pioneer` rebuild（`e9e53545`） | 游戏侧 crate、共享契约、vendored QuickJS-ng 与补丁、xtask、`docs/spec/` 与 `docs/spikes/` | egui 编辑器决定 |
| `Amoris Pioneer` master | 编辑器的面板与交互清单、命令目录做法、感知工具（`world.tree`、`events.why`、`step until/watch`）、评测 harness 设计、渲染技术清单 | C++ 宿主、JSC、`pocket` 构建工具、13.8k 行的单文件渲染器 |
| Amoris 前序 `feature/rust-core` 与 `feature/agent-native-rust` | wgpu 渲染要素（海面、尾迹、级联阴影、reversed-Z）、MCP 工具形状、React 编辑器的布局与协议驱动做法 | Lua 脚本层、每步 fork 的核心、PNG 轮询视口 |

## 8. 待决事项

1. 脚本 JIT 后端（原生 V8 或浏览器自身的 JS 引擎）是否进入路线：先做同负载的测量。
2. 真实拍摄的 splat 场景与 CC0 材质集的来源（需要所有者同意下载）。
3. 桌面编辑器外壳（Tauri 2）的时机。
