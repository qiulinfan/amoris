# aipocket2 纲领：Pocket3D 统一技术栈

状态：Accepted（本轮方向与技术栈）<br>
版本：1.0<br>
日期：2026-10-04<br>
仓库：私有仓库 `qiulinfan/aipocket2`（本地 `~/Desktop/aipocket2`）<br>
来源：所有者 2026-10-03/04 的指示；PocketEngine `rebuild` 分支纲领 v0.11；`aipocket` 的 `rebuild`
线（纲领 0.4，提交 `e9e53545`）与 `master` 线；PocketEngine `feature/rust-core` 与
`feature/agent-native-rust` 原型

## 1. 这份文档是什么

所有者在 2026-10-03 推翻了"PocketEngine 走 Rust + Lua、aipocket 走 Rust + TypeScript"的双线方案：
两条线不再区分技术栈，TypeScript 重新成为第一语言（Lua 暂时搁置），宿主统一为 Rust，放弃 C++
宿主。本轮由 agent 独立在私有仓库 `aipocket2` 里做出一个完整的引擎与完整的技术栈；所有者之后会把
产品拆开复盘，再按更顺的实现顺序构建到 PocketEngine。

本文固定 `aipocket2` 的定位、技术栈及理由、架构、证明目标与来源。子系统的细节规格在 `docs/spec/`
（多数来自 `aipocket` rebuild 线，随代码一起导入），实施排期在 [schedule.md](schedule.md)。

## 2. 定位

Pocket3D 是一个 **agent-native** 的 3D 游戏引擎。agent 在三个角色上都是一等公民，并且与人类使用
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

1. **性能**：渲染与物理比肩 Unity/UE5，web 比肩 three.js。同机对照测量：Bevy 0.19（Rust/wgpu 的
   前沿）、three.js WebGPURenderer（web）、Jolt（物理，Godot 4.6 起新项目的默认 3D 物理）（第 6 节）。
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
   agent 都能做，反之亦然（`aipocket` master 验证过的做法）。
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

## 4. 技术栈

### 4.1 总表

| 层 | 决策 | 版本 | 主要理由 |
|---|---|---|---|
| 宿主语言 | **Rust**（edition 2024） | 1.98.1 | 所有权让世界状态自包含，fork 不会有隐藏共享；serde/schemars 让序列化与 schema 来自类型；同一编译器覆盖原生与 `wasm32` |
| 构建 | **Cargo workspace + `cargo xtask`**；TS 包用 bun；web 用 wasm-bindgen + wasm-opt | — | 不自研构建系统（见 4.6） |
| ECS | `bevy_ecs`（只用 ECS） | 0.19.1 | 实体加组件本身就是语义 schema；单线程确定调度，遍历按 EntityId 排序 |
| 模拟数学 | `pocket_sim::math`（基于 `libm`，禁用平台超越函数，`-ffp-contract=off`） | libm 0.2.16 | 跨平台、跨原生/web 位级一致 |
| 物理 | **Rapier 3D**，`enhanced-determinism`；与 Jolt 同机对照后定终选（见 4.7） | 0.36.0 | 原生与 wasm 3001/3001 tick 一致，fork 后逐位延续（aipocket 物理 spike）；性能需对标 PhysX/Chaos |
| 持久化 | PCE 规范编码、分节哈希树（XXH3-128 / BLAKE3）、snapshot/restore/fork、replay 与首个分叉 | 自研 | 哈希定义在规范编码上而非内存布局 |
| 脚本语言 | **TypeScript** | TS 7（类型检查） | AI 时代的第一语言：模型语料最多、类型反馈最好 |
| 脚本转译 | **oxc**，进程内转译并产出 source map | 0.152 | 无需 Node；毫秒级；错误与断点都映射回 TS 行 |
| 脚本 VM | **QuickJS-ng**，经 `rquickjs`，vendored 并打补丁 | 0.16.2 / rquickjs 0.14.0 | 小型解释器，所有平台同一份，确定；补丁见 4.2 |
| 脚本类型 | Rust 组件注册表 → `.d.ts` 与 JSON Schema | ts-rs 12、schemars 1.2 | 改了引擎类型，agent 与脚本看到的接口自动同步 |
| 渲染 | **wgpu + WGSL** | 30.0.1 | 一套渲染器覆盖 Metal、Vulkan 与浏览器 WebGPU |
| 原生图形后端 | **Metal**（macOS）、**Vulkan**（Linux/Windows；macOS 上经 MoltenVK 验证） | — | 所有者 2026-10-04 指定只做这两个原生后端；不做 Direct3D 12 |
| Web 图形 | WebGPU（wgpu 的浏览器后端，同一份代码） | — | 证明 web 渲染；不做 WebGL 回退 |
| 窗口与输入 | winit；gilrs（手柄） | 0.30 | 事实标准 |
| 资源 | glTF 2.0（`gltf`）、PNG/JPEG（`image`）、网格处理（`meshopt`：LOD、顶点缓存、meshlet）；导入在工作线程异步进行；内容哈希作为 ID | gltf 1.4、meshopt 0.6 | aipocket 的 OBJ 导入冻结 14–27 秒，本线只走 glTF 且异步 |
| 神经渲染 | 3D Gaussian Splatting（GPU 排序、与网格深度混合）；神经纹理压缩（潜变量网格 + 小型 MLP，在片元着色器里解码） | 自研 | 见 4.4 |
| 音频 | kira（cpal；web 上为 WebAudio） | 0.12 | 混音、空间音频、补间 |
| agent 接口 | **CLI 优先**：一张命令目录 → `pocket` CLI（连接运行中的宿主，紧凑文本输出，`--json` 精确输出，帮助文本来自目录）、编辑器 WebSocket、脚本 API、测试；MCP（`rmcp`）是同一目录的薄投影 | rmcp 3.5 | agent 在 shell 里最顺手、最省 token；目录唯一，所有前端不漂移 |
| 服务端 | axum + tokio（只在 presenter 一侧） | axum 0.8 | 游戏侧 crate 禁止依赖 tokio |
| 编辑器 | **TypeScript + React + Vite**；Electron 桌面窗口与 Web 共用 dockview、Monaco 和 wasm/WebGPU 视口，经 WebSocket 与宿主同步 | React 19、Vite 8、Electron 44 | 见 4.5 |
| 调试 | 调试核心在 `pocket-debug`（QuickJS-ng PR #1421 的 trace 钩子）；前端：CDP 端点（Chrome DevTools、VS Code js-debug）、编辑器、MCP 工具 | 自研 | 见 4.3 |
| 性能分析 | 内建分析器：每个系统的 CPU 区段 + 每个渲染通道的 GPU 时间戳，推送到编辑器；Tracy 为可选特性 | tracy-client 0.19 | 测量为依据 |
| 离线工具 | **Python 3.14 + uv**：神经资源训练（PyTorch，Apple MPS）、评测 harness、分析脚本 | — | 只在工具层，不嵌入运行时 |

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
- 预算：每系统 100 万步、每 tick 200 万步、64 MiB 内存；步数不进入世界哈希。
- 性能定位：解释执行比同等 Rust 规则慢 21–27 倍（aipocket 的 script-native spike）。对策是让脚本
  做编排、让引擎做重活：物理、动画、寻路、感知、渲染都在 Rust 里，脚本经批量列接口调用。
  与 Godot 的 GDScript 处在同一量级，与 Unity C# 有差距；JIT 后端作为测量实验列入排期，不进入
  本轮默认路径。
- 被否决：V8（aipocket master：预编译库与 Chromium libc++ 冲突、源码构建数小时）、
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

- 一套 wgpu 渲染器，WGSL 着色器放在独立的 `.wgsl` 文件里（不放进 Rust 字符串），按通道组织模块。
- **渲染图**：每帧由通道组成的有向无环图，资源（瞬态纹理与缓冲）由图分配与复用。
- **GPU 驱动**：全部实例数据在存储缓冲里；计算着色器做视锥剔除（与上一帧 Hi-Z 遮挡剔除），把
  可见实例压缩并写出间接绘制参数；每个网格一次 `draw_indexed_indirect`，绘制调用数与实例数无关。
- **光照与画质**：reversed-Z、PBR（GGX）、聚簇前向光照、级联阴影、程序天空与 IBL、HDR、bloom、
  TAA、AgX 色调映射、实体 ID 缓冲（拾取与 agent 的"画面上是什么"）。
- **神经渲染**：
  - 3D Gaussian Splatting：预处理计算着色器（投影、剔除、球谐取色）、GPU 基数排序、实例化四边形
    混合，与网格深度缓冲合成，即混合网格与 splat 的场景。
  - 神经纹理压缩：材质的多通道纹理压缩为低分辨率潜变量网格加一个小 MLP（Python/PyTorch 训练），
    在片元着色器里逐像素推理解码；对比未压缩与传统压缩的体积和质量。
- 浏览器 WebGPU 与原生 Metal/Vulkan 走同一份代码；只使用 WebGPU 默认可用的特性作为基线，
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
- 被否决：egui（所有者要求 TS + React）；把编辑器 UI 做成引擎自己的 UI 系统（aipocket master 的
  Pocket UI，效果与工作量不划算）；视口按 PNG 轮询推帧（PocketEngine 原型，4 Hz）。

### 4.6 构建链

- Cargo workspace 加 `cargo xtask`（检查、代码生成、vendoring、web 打包）。不自研构建系统：
  aipocket master 的 `pocket` 工具（Rust 写的第二套构建系统，嵌入 n2 后又换回 Ninja）在 Cargo
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

## 5. 架构

### 5.1 crate

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
  （aipocket threads spike：10⁵ 实体 0.39 ms）。

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
| `aipocket` rebuild（`e9e53545`） | 游戏侧 crate、共享契约、vendored QuickJS-ng 与补丁、xtask、`docs/spec/` 与 `docs/spikes/` | egui 编辑器决定 |
| `aipocket` master | 编辑器的面板与交互清单、命令目录做法、感知工具（`world.tree`、`events.why`、`step until/watch`）、评测 harness 设计、渲染技术清单 | C++ 宿主、JSC、`pocket` 构建工具、13.8k 行的单文件渲染器 |
| PocketEngine `feature/rust-core` 与 `feature/agent-native-rust` | wgpu 渲染要素（海面、尾迹、级联阴影、reversed-Z）、MCP 工具形状、React 编辑器的布局与协议驱动做法 | Lua 脚本层、每步 fork 的核心、PNG 轮询视口 |

## 8. 待决事项

1. 脚本 JIT 后端（原生 V8 或浏览器自身的 JS 引擎）是否进入路线：先做同负载的测量。
2. 真实拍摄的 splat 场景与 CC0 材质集的来源（需要所有者同意下载）。
3. 桌面编辑器外壳（Tauri 2）的时机。
4. 拆分回 PocketEngine 的顺序（所有者复盘时决定）。
