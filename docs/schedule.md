# amoris 实施排期

日期：2026-10-04。本文是本轮任务的第三项交付物：技术栈见 [charter.md](charter.md)，探索实验的结果见
`docs/bench/`、`docs/spec/` 与 `docs/evidence/`，这里给出现状与把产品做到可交付的排期。

时间窗按所有者的要求取一周：2026-10-03（第 1 天）到 2026-10-09（第 7 天）。第 1、2 天已经过去，
第 3 到第 7 天是剩余的计划。

## 1. 可交付的定义

到第 7 天结束，`main` 上的引擎满足：

1. 一个仓库、一条命令构建（`cargo build --release`，浏览器端 `tools/build_web.sh`），Metal 与 Vulkan
   原生运行，同一份游戏在浏览器（WebGPU）里运行。
2. 下面第 3 节五个证明目标各有可复现的证据：命令、原始数据、截图或录屏，数字在安静的机器上测得。
3. `cargo xtask check` 在 M5 上完整跑过一次，日志入库。
4. 展示场景（`samples/harbor` 帆船港湾、`samples/arena` 对战）原生与浏览器都能跑。

不在本周范围：Windows 与 Direct3D 12、桌面编辑器外壳（Tauri）。本轮实现已迁入 Amoris。
Direct3D 12 已于 2026-10-09 作为 Pioneer 的探索方向重新纳入（纲领 4.1 与 4.4，测量见
`docs/bench/dx12.md`），不改变本周的交付定义。

## 2. 技术栈（定稿摘要）

| 层 | 选择 |
|---|---|
| 宿主 | Rust 1.98.1，cargo workspace + `cargo xtask`（不自研构建工具） |
| 世界 | bevy_ecs 0.19（只用 ECS），确定性 tick，PCE 规范编码，snapshot / fork / replay |
| 游戏逻辑 | TypeScript：oxc 转译，QuickJS-ng 执行（vendored，补丁 P1–P11），TypeScript 7 做类型检查 |
| 物理 | 现为 Rapier 0.36（确定性配置）；按 `docs/bench/physics.md` 的建议迁往 Jolt（第 4 天） |
| 渲染 | wgpu 30，WGSL；原生 Metal、Vulkan（macOS 上经 MoltenVK）、Direct3D 12（Windows，Pioneer 2026-10-09），浏览器 WebGPU；GPU 驱动 |
| 浏览器 | 游戏在 Web Worker 里跑同一份 wasm，页面用同一个渲染器（`pocket-viewport`）绘制 |
| agent 接口 | 一张命令目录投影出 CLI（第一位）、HTTP/WebSocket、MCP（薄投影）与 `.d.ts` |
| 调试 | CDP 端点（Chrome DevTools、VS Code）、`debug.*` 命令（agent 与编辑器共用） |
| 编辑器 | React 19 + dockview + Monaco，视口是编译到 wasm 的同一渲染器 |
| 音频 | kira 0.12 |
| 工具 | Python（uv）：神经资源训练、评测；Blender 4.5 无头脚本：程序化美术 |

## 3. 现状：五个证明目标

### 3.1 性能（对标前沿引擎）

| 项 | 结果 | 位置 |
|---|---|---|
| 渲染，原生 | many_cubes 160 万立方体：sphere 8.35 ms/帧（被 120 Hz 封顶），dense 11.7 ms；Bevy 0.19 同机 10.4 / 13.0 ms | main，`docs/bench/bevy-baseline.md` |
| 遮挡剔除（Hi-Z 两阶段） | many_cubes 160 万、1280x720、GPU 时间（暂定，机器有负载）：RTX 5060（Vulkan）dense 16.0 → 0.9 ms，镜头环绕 21.8 → 2.8 ms；Radeon 780M dense 91.6 → 5.1 ms，环绕 40.9 → 8.0 ms；无遮挡的 sphere 强制开启多 0.2–0.4 ms（780M 多 1.2–1.4 ms），auto 模式关掉它、与关闭持平；开关前后画面与 id 覆盖一致（原生四种配置与 Chrome） | 分支 `explore/hiz`，`docs/spec/occlusion.md`、`docs/bench/occlusion.md`。2026-10-09 Pioneer 更正：原记的 `feat/hzb`（M5 上 dense 13.4 → 6.3 ms、环绕 13.7 → 8.3 ms）已遗失，数字无从复核 |
| 渲染，浏览器 | 与 three.js r186 同场景，GPU 时间快 1.5–3.4 倍 | main，`docs/bench/web.md` |
| 物理 | Rapier 对 Jolt 同场景：箱堆 Rapier 快 1.5–2.4 倍；网格地形上的凸体 Jolt 快 1.6 倍，布娃娃 2.4 倍（4 线程 2.2 / 4.6 倍）；浏览器默认构建 Jolt 全部更快；两者原生与 wasm 哈希一致 | main，`docs/bench/physics.md` |

缺口：所有数字都在多个 agent 同时编译、训练、跑 GPU 的机器上测得（负载 6–83）。Unity 与 UE5 无法在
本机直接运行，对标靠 Bevy、three.js 与 Jolt（Godot 4.6 默认物理）作为同机参照。

### 3.2 agent 原生 gameplay

| 项 | 结果 | 位置 |
|---|---|---|
| 玩家层 | 每个席位只感知可见之物（射程、物理射线视线、队伍视野、记忆）；文本（带 token 预算）、JSON、165 维张量三种投影；意图作为组件持续；时间模式含决策暂停、步进、锁步、回合 | 分支 `feat/play`，修复未完 |
| 决策模型 | arena 5v5，十个脑各 60 Hz：单线程每 tick 1.3–1.8 ms，每秒 5,650–7,623 次决策，9–13 倍实时 | 同上 |
| 对局 | 进攻对防守 20 局 16–4，换边 15–5；结果确定可复现 | 同上 |
| 前瞻 | 两条 180 tick 分支在丢弃式 fork 上 0.2 s 内回答 | 同上 |
| LLM 对局 | opencode + GLM 5.3 Flash 通过 CLI 下了三局（轶事，非胜率） | 同上 |

### 3.3 web 渲染与神经渲染

| 项 | 结果 | 位置 |
|---|---|---|
| 浏览器完整游戏 | worker 里跑游戏、页面 WebGPU 绘制；帆船 1.1 ms，动画示例（蒙皮、UI、粒子）0.8 ms GPU | main，`docs/evidence/render/web-player-*.png` |
| 3D 高斯 splat | 原生 1M splat：预处理 1.0 ms、排序 0.95 ms、绘制 5–6 ms；浏览器 30 万 splat 与网格同画面，总 GPU 4.1 ms | main，`docs/spec/splats.md` |
| 神经纹理压缩 | 9 通道材质 5.29 bit/texel（BCn 的 1/6）；lod 0 质量与半分辨率 BCn 相当，lod 2 低 7–9 dB；全屏 2560×1440 解码 f16 约 8–10 ms | 分支 `feat/neural`，修复未完 |
| GTAO | 半分辨率，只压暗间接光；1600×900 约 0.55–0.7 ms，2560×1440 约 1.2–1.7 ms；Metal、Vulkan、Chrome 都已验证 | 分支 `feat/gtao`，修复未完 |

### 3.4 编辑器

| 项 | 结果 | 位置 |
|---|---|---|
| 编辑器连真宿主 | 层级、检视器、Gizmo、选中描边、wasm 视口点选；断点调试全流程（见 3.5） | main，`docs/evidence/editor/` |
| 类型 | 宿主按项目生成 `.d.ts`，Monaco 与 TypeScript 7 对 14 种错误给出相同的代码与位置；一次检查约 40 ms | main（sdk 合并，未推送），`docs/bench/typecheck.md` |

缺口：性能面板（宿主没有 profile 推送）、agent 会话面板、层级重挂（没有父子关系）、资源导入。

### 3.5 断点调试与 agent 原生调试

| 项 | 结果 | 位置 |
|---|---|---|
| 人用调试 | Chrome DevTools 与 VS Code js-debug 命中 TS 断点；编辑器里断点、单步、改值、数据断点、运行中改脚本 | main，`docs/spec/debugger.md`，`docs/evidence/editor/debug-host-*.png` |
| agent 调试 | GLM 5.3 Flash 只拿到玩家描述的症状、只用 CLI，23 次修好 21 次 | main，`docs/bench/debug-eval.md` |

评测暴露的问题（按损失排序）：游戏停在断点时 agent 的读命令也被阻塞；恢复快照会悄悄换回旧脚本包；
`debug.eval` 看不到块作用域里后声明的局部变量（QuickJS-ng 的问题，需补丁 P11）；调试命令不在命令
目录与 MCP 的参数模式里。另外，23 次记录是在沙箱修好之前跑的，需要重跑。

## 4. 排期

每一天以一个可运行的端到端结果收尾；并行的工作各用一个 worktree 与自己的 cargo target 目录。

### 第 1 天（10-03，已完成）

纲领 v1.0；从 Amoris Pioneer rebuild 线导入游戏侧 crate、QuickJS-ng 与 xtask；GPU 驱动渲染器的骨架。

### 第 2 天（10-04，已完成）

渲染器做到超过 Bevy；浏览器视口与 three.js 对照；宿主、CLI、MCP、编辑器连真宿主；调试器；
浏览器完整游戏；splat 合并；物理对照、类型 SDK、编辑器调试、agent 调试评测合并。
`feat/play`、`feat/neural`、`feat/art`、`feat/hzb`、`feat/gtao` 已实现、已评审，修复阶段被中止。
（2026-10-09 Pioneer 更正：`feat/hzb` 在合并前遗失，任何仓库里都没有它的代码；遮挡剔除已由
`explore/hiz` 重新实现，见 3.1。）

### 第 3 天（10-05）：收尾合并

1. 跑完 sdk 合并后 `pocket-script`、`pocket-check` 的测试，推送 main。
2. 用保存的脚本恢复两个 workflow（已完成的 agent 从缓存回放），完成 play、neural、hzb、gtao 的修复
   与 art 的复查。
3. 依次合并：art → hzb → gtao → neural → play。neural 与 hzb/gtao 都改 `renderer.rs`；play 与 sdk 都改
   `client.rs`、`catalog.rs`、`game.rs` 与 MCP。每次合并后重建并提交 `web/viewport/pkg`（帧格式指纹）。
4. 在 M5 上完整跑一次 `cargo xtask check`，日志入库。

退出条件：main 含全部分支；`pocket play samples/harbor`、`pocket match samples/arena`、浏览器播放页都能跑；
check 日志入库。

### 第 4 天（10-06）：物理迁往 Jolt

按 `docs/bench/physics.md` 给出的顺序做四项检查，任何一项失败就留在 Rapier：

1. joltc 分叉加 `SaveState` / `RestoreState` 与 WASI shim，原生与 pocket-web 的 wasm（wasm-bindgen，与
   QuickJS-ng 共用一份 wasi-libc）都能构建。
2. `pocket-physics` 的求解器部分（约 1,100 行）换成 Jolt，组件与力（浮力、风、帆、船体）不变。
3. 确定性：帆船场景 3,001 tick 原生与 Chrome 哈希一致；每个 tick 从字节恢复的 fork 继续一致。
4. 测量：本仓库物理 bench 与帆船场景在安静机器上重跑。

同一天并行：把帆船物理预制体改成与 9 m 模型一致（现为 3.4 m），给岛屿与码头加碰撞体。

退出条件：sailing、harbor、arena 的 `pocket check` 全部通过；`physics.md` 补上迁移后的数字。

### 第 5 天（10-07）：agent 接口与调试

1. 一个生成器：命令目录（含 `pocket_debug::methods()`、player 命令）的模式生成 CLI 短式、MCP 工具与
   编辑器的命令表，取代手写的三份。
2. 修评测暴露的问题：断点暂停时读命令照常回答，`step` 返回停止原因；快照恢复默认用当前脚本包；
   QuickJS-ng 补丁 P11 修正 `debug.eval` 的作用域；`debug writes`（谁写了这个字段）；按字段读取与
   `step --sample` 表格。
3. 在修好的沙箱里重跑调试评测，每个 bug 至少 3 次，给出成功率。

退出条件：新的评测表；CLI 与 MCP 都由目录生成，`smoke_server.py` 通过。

### 第 6 天（10-08）：agentic gameplay 前沿

1. 学习出来的决策模型：在 arena 的张量投影上用自博弈训练一个小策略网络（Python，MPS），推理在
   游戏线程外按每秒上百次决策提交意图；与手写脑对局给出胜率。
2. LLM 对手写脑的多局对局（决策暂停模式），给出胜率与 token 开销。
3. arena 在浏览器播放页里运行；按席位推送事件（替换 250 ms 轮询）。

退出条件：`docs/bench/` 里有学习模型与 LLM 的胜率表；浏览器里能看一局 arena。

### 第 7 天（10-09）：测量与展示

1. 安静机器上统一重测：many_cubes（开关遮挡剔除）对 Bevy，浏览器对 three.js，物理，splat，神经纹理，
   GTAO。每项多次取最小值，记下负载。
2. 展示：harbor 原生与浏览器各一段录屏；arena 中 LLM 与学习模型的对局录屏。
3. 结项报告：五个证明目标各一页（数字、命令、证据路径、已知限制）。

退出条件：第 1 节的四条全部满足。

## 5. 风险

| 风险 | 影响 | 处理 |
|---|---|---|
| Jolt 进 wasm 要与 QuickJS-ng 共用 wasi-libc 与 libc++ | 第 4 天 | 检查 1 先做；失败就留在 Rapier，第 4 天转做 Rapier 的缓存瘦身（不存静态几何） |
| 合并冲突（`renderer.rs`、`client.rs`、视口二进制） | 第 3 天 | 按第 3 天给的顺序合并，每次合并后构建并跑相关 crate 的测试 |
| 数字在负载下测得 | 证据可信度 | 第 7 天在没有其他 agent 运行时统一重测 |
| Metal 的渲染通道时间戳不可靠（常为 0） | GPU 计时 | 渲染通道用 Vulkan（MoltenVK）时间戳或整帧时间交叉核对 |
| 神经纹理全屏解码 8–10 ms | 实用性 | 作为先进性证明保留；实用路径（加载时解码或转 BCn）列为后续 |
| 学习决策模型可能一天训练不出强于手写脑的策略 | 第 6 天 | 先做行为克隆（模仿手写脑）保证可用，再自博弈改进 |

## 6. 第 7 天之后

Windows 与 Direct3D 12（Pioneer 2026-10-09 已开始：`docs/bench/dx12.md`）；Tauri 桌面外壳；地形
细节贴图与 splatting；时间性抗锯齿与超分；资源导入管线与压缩纹理；把引擎按第 4 节的顺序拆分迁入
所有者的引擎（由所有者主导）。
