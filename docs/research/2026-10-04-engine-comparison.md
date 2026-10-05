# 引擎对照研究：UE5、Godot 4、Bevy 0.19、three.js 与 Pocket3D 的计划

状态：研究报告（只读调查；不改变纲领，建议需所有者决定）<br>
日期：2026-10-04<br>
对照对象：`docs/charter.md` 1.0（含 CLI 优先与物理后端对照的修订）与 `docs/spec/`

源码根（只读学习，未复制任何代码；UE 受 Epic EULA 约束）：

| 前缀 | 路径 | 版本 |
|---|---|---|
| `UE:` | `~/Reference/UnrealEngine/` | 5.8 前后，稀疏（只有 `Engine/Source`、`Build`、`Config`，没有 `Engine/Plugins`，所以 MCP、Remote Control、Network Prediction 等插件只能引用官方文档） |
| `Godot:` | `~/Reference/godot/` | master，2026-10 |
| `Bevy:` | `~/Reference/bevy/` | 0.19.0 |
| `three:` | `~/Reference/threejs/` | dev，2026-10 |
| `wgpu:` | `~/.cargo/registry/src/index.crates.io-*/wgpu-30.0.1/` | 30.0.1（Pocket3D 所用版本） |

网络来源列在文末，正文用 [n] 引用。

## 0. 十五条要点

| # | 教训 | 来源 | Pocket3D 的计划要改什么 | 优先级 |
|---|---|---|---|---|
| 1 | 浏览器 WebGPU 的基线装不下 Bevy 式 GPU 剔除。multi-draw indirect 仍要开实验 flag；`indirect-first-instance` 是可选特性；immediates 2026 年才在 Chrome 进入发货流程；默认每个着色阶段只有 8 个 storage buffer、4 个 storage texture。Bevy 的 GPU 剔除要求 `INDIRECT_FIRST_INSTANCE \| IMMEDIATES` 以及 12/10 的限额，meshlet 只支持 Vulkan 与 Metal | Bevy、WebGPU 规范 [10][11][12] | 渲染规格拆成 web 层与原生层：web 层每批一次 `draw_indexed_indirect`，`first_instance` 置 0，实例基址经动态偏移 uniform 传入，Hi-Z 每趟最多写 4 个 mip；原生层再启用 MDI 与 immediates。能力检测结果写进 `RenderStats` | P0 |
| 2 | 所有 127.0.0.1 端点都要校验令牌和 `Origin`。UE 5.8 的 MCP 只靠回环、没有认证；Unity 要求人工批准新连接；Chrome 111 起 DevTools 端点拒绝未知 Origin | UE [1]、Unity [4]、Chrome [17] | host 启动时生成会话令牌（写入项目的 `.pocket/session`），`/ws`、`/render`、MCP HTTP、CDP 都校验令牌与 `Origin`，CLI 自动读取令牌 | P0 |
| 3 | 资源身份分两层。Godot 的 `uid://` 与内容无关（存在 `.import`、`.uid` 旁文件和 `.tscn` 头里），派生物按源 md5 与 importer 版本失效 | Godot、UE DDC | 场景引用稳定的资源 UID；replay 与快照头记录 UID→`ContentHash` 锁表，继续保证"replay 指明确切字节"。写进 `pocket-assets` 规格 | P0 |
| 4 | 脚本出错就进调试器。GDScript 的运行时错误调用 `debug_break(err, false)`，停在出错行；暂停期间调试循环仍处理其他消息 | Godot | 把"异常时暂停"从 debugger spike 记下的缺口提升为切片必做项；`debug.state` 带出当前系统已暂存、尚未提交的写入 | P0 |
| 5 | Unity 在 2026 年弃用编辑器内 MCP，改由 CLI 的 `unity command`、`unity eval` 驱动编辑器（官方说法是更快、更省 token），MCP 只留给不能跑 shell 的 agent，并提供 `unity skill install` | Unity [5] | 印证纲领的"CLI 优先"。补 `pocket eval`（在运行中的世界里只读求值 TS），并从命令目录生成随引擎版本分发的 agent skill | P1 |
| 6 | 渲染图三种做法：UE RDG 声明式、可剔除、帧内别名；Godot 从即时调用自动建图并重排；Bevy 0.19 删掉了节点式 RenderGraph，改用 ECS 调度 | UE、Godot、Bevy | wgpu 自动插屏障、没有放置资源，做不了别名。采用"声明式通道表 + 按描述符复用的纹理池 + 每通道 GPU 时间戳 + 图可导出为 JSON" | P1 |
| 7 | 持久 GPU 槽位加脏数据散写：UE `FGPUScene::AddPrimitiveToUpdate` 配 `FRDGAsyncScatterUploadBuffer`；Bevy `sparse_buffer_update.wgsl` | UE、Bevy | 渲染器按 `EntityId` 分配稳定实例槽，`/render` 差量只散写变动的槽；插值用的上一帧变换留在 GPU 上 | P1 |
| 8 | 管线编译不能卡帧。UE 预收集 PSO，未就绪时用回退材质；Godot 用 ubershader 顶上；three.js 用 `compileAsync` 和 `createRenderPipelineAsync`；Bevy 在 wasm 与 macOS 上同步编译。wgpu 30 的 web 后端只调同步的 `createRenderPipeline`，`PIPELINE_CACHE` 只实现了 Vulkan | 四家与 wgpu | 渲染器提供 `prepare(scene)`，载入时枚举管线键并预建；新材质未就绪时画回退材质；逐管线测量创建耗时；web 上的异步建管线作为 wgpu 贡献或本地补丁评估 | P1 |
| 9 | 材质以文本为一等。Godot 着色语言是带固定入口的受限 GLSL，被注入引擎模板；UE 新增 MaterialIR，把材质图先译成 IR 再出 HLSL；three.js TSL 记录 JS 调用栈，把着色器错误指回源码 | Godot、UE、three.js | 材质是遵守固定契约的 WGSL 函数，插进引擎模板；参数反射为 JSON Schema；材质实例只换参数、不换管线；naga/Tint 的错误映射回材质文件行号；节点图编辑器以后作为文本的投影 | P1 |
| 10 | 浏览器线程。Godot 4.3 起默认单线程导出，因为 SharedArrayBuffer 需要的跨源隔离挡住了发布平台；web 包约 40 MB，brotli 后约 5 MB | Godot [7][8] | 默认 web 构建不依赖 SAB；渲染器放进 Worker 的 OffscreenCanvas，不与 React 编辑器争主线程；wasm 体积进性能记录 | P1 |
| 11 | 撤销。UE `FTransaction` 存对象序列化后的前后状态做差；Godot `UndoRedo` 存 do/undo 方法对，用 `MERGE_ENDS` 合并拖动；`EditorUndoRedoManager` 分全局、每场景、远程三类历史 | UE、Godot | 事务记录被触及组件的 PCE 前后字节，不手写逆操作；Gizmo 拖动合并成一步；编辑世界与 Play 分支各有一条历史 | P1 |
| 12 | 检视器靠提示驱动，而不只靠类型。Godot `PropertyInfo{type, hint, hint_string, usage}` 有 50 种 hint、34 种 usage 标志；UE 用 `meta=(ClampMin, EditCondition)` 与 `IPropertyTypeCustomization` | Godot、UE | JSON Schema 加 `x-pocket` 扩展：范围、步长、单位、枚举标签、编辑条件、对玩家 agent 是否可见；React 端按类型名注册自定义编辑器 | P1 |
| 13 | 一份机读 API 描述生成所有绑定，并做跨版本差异检查。Godot 的 `extension_api.json` 加 `misc/extension_api_validation/`；UE 5.8 的 MCP 工具也从反射（`meta=(AICallable)`）生成 | Godot、UE | `cargo xtask api` 输出 `engine-api.json`（命令目录、组件 schema、事件），从它生成 `.d.ts`、CLI 帮助、MCP 工具表和文档；与上一个标签做差，报告破坏性变更 | P1 |
| 14 | 资源烘焙是带版本的纯函数。UE DDC 的 build function 要求无状态，版本代表代码；Godot 用 `importer_version` 与 `source_md5`/`dest_md5` 判定是否重导入 | UE、Godot | 导入与烘焙函数带版本号，缓存键 = (函数, 版本, 参数, 源哈希)；LOD 与 meshlet 在导入时生成一次（render spike 每次加载都重建 LOD，约 3 s）；导入参数写在源文件旁的 sidecar 里并入库 | P1 |
| 15 | 物理确定性要在编译期打开，后端不能混用。Godot 集成了 Jolt，却没有定义 `JPH_CROSS_PLATFORM_DETERMINISTIC`；UE Chaos 的重模拟靠服务器状态纠错，不追求逐位一致 | Godot、UE | 对照后若选 Jolt，就打开跨平台确定性并编译到 wasm，只留一个后端；否则 replay 头与世界哈希要带后端标识，原生 Jolt 的录制不能拿到 web Rapier 上验证 | P1 |

## 1. 渲染架构

### 1.1 渲染图

- **UE RDG**：`UE:Engine/Source/Runtime/RenderCore/Public/RenderGraphBuilder.h` 的 `FRDGBuilder`。`AddPass` 接收参数结构，资源依赖、屏障与生命周期都从结构里的 RDG 参数推导；`Execute()` 编译、剔除、执行。`RenderGraphDefinitions.h` 的 `ERDGPassFlags` 有 `AsyncCompute`、`NeverCull`（输出无法被图跟踪时防止剔除）、`NeverMerge`；`ERDGBuilderFlags::Parallel` 并行做 setup、compile、execute。瞬态资源经 `Runtime/RHI/Public/RHITransientResourceAllocator.h` 在帧内别名；TAA 历史等跨帧资源用 `QueueTextureExtraction` 取出。
- **Godot**：没有显式图。`Godot:servers/rendering/rendering_device_graph.h/.cpp` 的 `RenderingDeviceGraph` 把 `RenderingDevice` 的即时调用录成 `RecordedCommand`，用 `ResourceTracker` 推出依赖，在 `end(p_reorder_commands, …)` 里按依赖层级（`level`）重排，并用 `_group_barriers_for_render_commands` 合并屏障。上层 `renderer_rd/forward_clustered/render_forward_clustered.cpp` 仍是顺序代码。
- **Bevy 0.19**：节点式 RenderGraph 已删除。`Bevy:crates/bevy_render/src/renderer/mod.rs` 里的 `RenderGraph` 现在是一个 `ScheduleLabel`（Begin/Render/Submit/Finish 四个集合）；`crates/bevy_core_pipeline/src/schedule.rs` 的 `Core3d` 是调度，`Core3dSystems::{Prepass, MainPass, EarlyPostProcess, PostProcess}` 是系统集，`camera_driver` 为每台相机运行子调度。瞬态纹理由 `crates/bevy_render/src/texture/texture_cache.rs` 的 `TextureCache` 按描述符跨帧复用，没有帧内别名。
- **three.js**：没有渲染图。后处理是 TSL 节点图（`three:src/renderers/common/PostProcessing.js`、`pass()` 节点、`src/nodes/core/MRTNode.js`）。

### 1.2 游戏线程到渲染的交接

- **UE**：组件的 `CreateSceneProxy()`（`UE:Engine/Source/Runtime/Engine/Classes/Components/PrimitiveComponent.h`）生成渲染线程拥有的 `FPrimitiveSceneProxy`（`Runtime/Engine/Public/PrimitiveSceneProxy.h`），之后的改动用 `ENQUEUE_RENDER_COMMAND`（`Runtime/RenderCore/Public/RenderingThread.h`）把 lambda 送过去。GPU 侧 `Runtime/Renderer/Private/GPUScene.h` 使用持久图元索引和 `EPrimitiveDirtyState`，只把脏图元经 `FRDGAsyncScatterUploadBuffer` 散写上传。
- **Godot**：节点只持有 RID，经 `RenderingServer` API 修改；`Godot:servers/rendering/rendering_server_default.h` 在非服务线程调用时压入 `CommandQueueMT`。独立渲染线程（`rendering/driver/threads/thread_model` = Separate）在 `main/main.cpp` 里仍被警告为实验性、可能崩溃。固定步长到帧的插值在渲染侧完成：`renderer_scene_cull.h` 的 `update_interpolation_tick/frame`。
- **Bevy**：主世界与渲染世界分开。`crates/bevy_render/src/extract_plugin.rs` 的 `ExtractSchedule` 同时访问两个世界，期间主世界被占用，文档要求它尽量短；`pipelined_rendering.rs` 把渲染子应用放到另一个线程；`sync_world.rs` 维护 `RenderEntity`/`MainEntity` 映射。
- **Pocket3D 现状**：游戏线程发布不可变快照（分节 `Arc`），`/render` 发视觉组件的差量（`docs/spec/threads.md` 4、`host-protocol.md` 5）。它不阻塞游戏线程，比 Bevy 的 extract 耦合更松，方向正确；缺的是渲染端"持久槽位加散写"的规格。

### 1.3 GPU 驱动与剔除

- **UE**：`UE:Engine/Source/Runtime/Renderer/Private/Nanite/NaniteCullRaster.cpp`（簇级剔除、基于 HZB 的两趟遮挡、小三角形软光栅）；非 Nanite 几何走 `InstanceCulling/InstanceCullingContext.cpp` 与 `InstanceCullingManager.h`（按 HZB 分 bin 的负载均衡）。
- **Bevy**：`crates/bevy_render/src/batching/gpu_preprocessing.rs` 定义 `GpuPreprocessingMode::{None, PreprocessingOnly, Culling}`。GPU 剔除要求设备特性 `INDIRECT_FIRST_INSTANCE | IMMEDIATES`，还要每阶段至少 12 个 storage texture（Hi-Z 降采样）和 10 个 storage buffer（早期遮挡剔除）；WebGL2 没有计算着色器，模式为 None；部分 Adreno、Mali、Pixel 10 驱动被降级。原生端用 `multi_draw_indirect_count`。meshlet（`crates/bevy_pbr/src/meshlet/mod.rs`）要求 `TEXTURE_INT64_ATOMIC`、`SHADER_INT64`、`SUBGROUP`、`IMMEDIATES` 等，文档写明只在 Vulkan 与 Metal 上工作、不兼容 MSAA、几何量小时比标准渲染器慢。
- **Godot**：没有 GPU 剔除。`servers/rendering/renderer_scene_cull.cpp` 在 CPU 上剔除，遮挡剔除在 `renderer_scene_occlusion_cull.cpp`（遮挡体光栅化，Embree 在 `modules/raycast`）；间接绘制只用于 MultiMesh（`render_forward_clustered.cpp` 的 `INSTANCE_DATA_FLAG_MULTIMESH_INDIRECT`）。
- **three.js**：`BatchedMesh` 在 WebGPU 后端逐个 `drawIndexed`，`firstInstance = i`（`three:src/renderers/webgpu/WebGPUBackend.js` 约 2126 行起）；`IndirectStorageBufferAttribute` 支持 `drawIndexedIndirect`；没有 MDI。
- **浏览器约束**：multi-draw indirect 需要 `chrome://flags` 与 `chromium-experimental-multi-draw-indirect`，web3dsurvey 统计的支持率接近 0 [10]；immediates 于 2026-05 在 Chrome 发出 Intent to Ship [11]；WebGPU 默认限额 `maxStorageBuffersPerShaderStage = 8`、`maxStorageTexturesPerShaderStage = 4` [12]。

### 1.4 聚簇光照与阴影

- **UE**：`Runtime/Renderer/Private/LightGridInjection.cpp` 用计算着色器把光源注入 froxel 光照网格，供前向着色与半透明使用；另有 `ClusteredDeferredShadingPass.cpp` 与 `MegaLights/`。阴影：`VirtualShadowMaps/VirtualShadowMapArray.h` 中 `PageSize × Level0DimPagesXY` 构成 16k 的虚拟地址空间（开关 `r.Shadow.Virtual.Enable`），`VirtualShadowMapCacheManager.cpp` 只重绘失效页；VSM 依赖 Nanite 才能高效地把几何画进页。
- **Godot**：`renderer_rd/cluster_builder_rd.h` 把 omni/spot/area 光、decal、反射探针统一放进 32 px 屏幕格加深度位段，`shaders/cluster_render.glsl` 光栅化光源代理体来写簇。Mobile 渲染器不用簇，每个实例最多 `MAX_RDL_CULL = 8` 个光、探针或 decal（`forward_mobile/render_forward_mobile.h`）。方向光最多 4 级分割（`LIGHT_DIRECTIONAL_SHADOW_PARALLEL_4_SPLITS`），点光与聚光进四象限阴影图集（`storage_rd/light_storage.h` 的 `ShadowAtlas`）。
- **Bevy 0.19**：`crates/bevy_pbr/src/cluster/gpu.rs` 的 GPU 聚簇把硬件光栅器当计算单元用：先按 Z 切片生成间接实例，再关掉颜色写入做计数光栅化（片元里原子计数），用 Hillis-Steele 前缀和分配空间，最后填表。级联阴影在 `crates/bevy_light/src/cascade.rs`。
- **three.js**：`three:examples/jsm/tsl/lighting/ClusteredLightsNode.js` 用计算着色器分配（默认 1024 光、32 px 格、24 个指数深度切片、每簇 64 光）；阴影有 `examples/jsm/csm/CSMShadowNode.js` 与 `examples/jsm/tsl/shadows/TileShadowNode.js`。

### 1.5 材质、着色器交叉编译与管线缓存

- **UE**：材质图经 `Runtime/Engine/Private/Materials/HLSLMaterialTranslator.cpp` 生成 HLSL；新路径是 `MaterialIRModuleBuilder.cpp` → `FMaterialIRModule` → `MaterialIRToHLSLTranslator.cpp`，由 `MaterialShared.cpp` 里的 `r.Material.Translator.EnableNew` 切换。HLSL 再经 `Developer/VulkanShaderFormat`（`ThirdParty/ShaderConductor`）和 `Developer/Apple/MetalShaderFormat/Private/MetalCompileShaderMSC.cpp`（Apple 的 `metal_irconverter`，DXIL 转 Metal）编译。PSO：`Runtime/Engine/Public/PSOPrecache.h` 的 `FPSOPrecacheParams` 收集组件可能用到的所有 PSO；`PSOPrecache.cpp` 的 `EPSOPrecacheProxyCreationStrategy::{AlwaysCreate, DelayUntilPSOPrecached, UseFallbackMaterialUntilPSOPrecached}` 决定未就绪时的行为；`Runtime/RHI/Public/PipelineFileCache.h` 记录运行中遇到的 PSO，供下次预热。
- **Godot**：`servers/rendering/shader_language.cpp` 解析受限 GLSL（`shader_type spatial`、`ALBEDO` 等内建量），`shader_compiler.cpp` 产出 GLSL 片段注入引擎模板，`rendering_device.cpp` 的 `shader_compile_spirv_from_source` 经 glslang 生成 SPIR-V，再由各驱动的 `RenderingShaderContainer` 转换（`drivers/metal/rendering_shader_container_metal.cpp` 用 SPIRV-Cross 出 MSL，再调 `xcrun metal` 生成 metallib）；导出时 `editor/shader/shader_baker/` 预烘焙。防卡顿：`scene_shader_forward_clustered.h` 的 `SHADER_COLOR_PASS_FLAG_UBERSHADER`，专用管线在 `renderer_rd/pipeline_hash_map_rd.h` 里交给 `WorkerThreadPool` 后台编译，期间画 ubershader；`rendering/rendering_device/pipeline_cache/enable` 把驱动管线缓存落盘。
- **three.js TSL**：`three:src/nodes/tsl/TSLCore.js` 构图，`src/nodes/core/NodeBuilder.js` 做通用生成，`src/renderers/webgpu/nodes/WGSLNodeBuilder.js` 出 WGSL，`src/renderers/webgl-fallback/nodes/GLSLNodeBuilder.js` 出 GLSL ES 3.0。`src/renderers/common/nodes/NodeManager.js` 按缓存键复用 `NodeBuilderState`；`buildAsync(yieldFn)` 在着色器阶段之间让出主线程并排队限制并发；构建失败退回默认 `NodeMaterial`。`src/nodes/core/StackTrace.js` 在节点创建时记下 JS 调用栈，用来把错误指回 TSL 源码。管线：`src/renderers/common/Pipelines.js` 按缓存键共享并用 `usedTimes` 计数释放；`src/renderers/webgpu/utils/WebGPUPipelineUtils.js` 在 `compileAsync` 时走 `createRenderPipelineAsync`，同步路径用 `pushErrorScope('validation')` 捕错后 `_reportShaderDiagnostics`。
- **Bevy**：`crates/bevy_shader/src/shader.rs` 用 naga_oil 处理 `#import` 与 shader defs；`crates/bevy_render/src/render_resource/pipeline_cache.rs` 的 `CachedPipelineState::{Queued, Creating, Ok, Err}`，未就绪的管线这一帧直接不画；`create_pipeline_task` 在 `wasm32` 和 `macos` 上同步阻塞编译。
- **wgpu 30.0.1**：`src/backend/webgpu.rs` 的 `create_render_pipeline` 只调同步 API，`createRenderPipelineAsync` 只出现在生成的 web-sys 绑定里，没有公开接口；`wgpu-types` 中 `Features::PIPELINE_CACHE` 只实现了 Vulkan，DX12 与 Metal 未实现。

### 1.6 抗锯齿与超分

UE：`Runtime/Renderer/Private/PostProcess/TemporalSuperResolution.cpp`（TSR）、`TemporalAA.cpp`。Godot：`renderer_rd/effects/` 下 `taa.cpp`、`fsr.cpp`、`fsr2.cpp`、`smaa.cpp`、`metal_fx.cpp`（MetalFX 空间与时间超分，实现 `SpatialUpscaler` 接口）。Bevy：`crates/bevy_anti_alias/src/{taa, smaa, fxaa, contrast_adaptive_sharpening, dlss}`。three.js：`examples/jsm/tsl/display/TRAANode.js`、`FSR1Node.js`。

### 1.7 对 Pocket3D 的教训

1. **渲染图做轻量版**（要点 6）。wgpu 自动插入屏障，又不提供放置资源，UE RDG 最贵的两项（屏障推导、内存别名）在 wgpu 上要么已经免费、要么做不到；Godot 和 Bevy 0.19 说明"声明顺序加自动跟踪"已经够用。值得从 RDG 学的三点：没有消费者的通道不执行（调试视图等）；TAA 历史、Hi-Z 这类跨帧资源显式声明；每个通道一个 GPU 时间戳。图导出为 JSON，供 `render.graph` 命令让 agent 查看这一帧画了什么。
2. **GPU 驱动分两层**（要点 1，证据见 1.3）。web 层：一个计算通道做视锥剔除加上一帧 Hi-Z 遮挡剔除，按"网格×材质"批次压缩可见实例并写 `DrawIndexedIndirect` 参数；不依赖 `indirect-first-instance` 与 immediates。原生层在 `MULTI_DRAW_INDIRECT_COUNT` 可用时合并成一次调用。meshlet 与虚拟几何不进本轮，因为连 Bevy 的实现都只支持 Vulkan/Metal。
3. **持久实例槽位**（要点 7）。快照差量已经存在，补上 GPU 侧之后，"实例多、每帧变动少"的场景（海岛植被、箱子）几乎不需要上传。
4. **管线预热与回退**（要点 8）。主开发机是 Apple M5，正好落在 Bevy 同步编译、wgpu 无管线缓存的组合上，浏览器端也是同步建管线。先测量每个管线的创建耗时与首帧卡顿，再决定是否给 wgpu 贡献异步接口。
5. **阴影与光照**。保留四级 CSM，借用 VSM 的"缓存加失效"思路：静态投射体（岛屿）所在的远级联只在太阳方向或级联移动时重画，动态物体每帧画。聚簇光照先用 three.js 式的计算着色器分配（简单，web 友好），光源很多时再评估 Bevy/Godot 的光栅化聚簇。AA 先做 TAA（运动矢量、抖动、历史），Apple 原生端可经 wgpu 的 Metal hal 接入 MetalFX（Godot `metal_fx.cpp` 是先例）。

## 2. Web 渲染

### 2.1 three.js WebGPURenderer

- **后端抽象**：`three:src/renderers/common/Renderer.js`（约 4,060 行）加 `Backend.js` 接口。`src/renderers/webgpu/WebGPURenderer.js` 选用 `WebGPUBackend`，失败时 `getFallback` 返回 `WebGLBackend`（`src/renderers/webgl-fallback/WebGLBackend.js`，计算用 transform feedback 模拟）；`forceWebGL` 可强制回退。同一个 TSL 图在两个后端分别出 WGSL 和 GLSL。
- **设备**：`WebGPUBackend.init` 用 `featureLevel: 'compatibility'` 请求 adapter，并把 adapter 支持的全部特性放进 `requiredFeatures`；设备没有 `core-features-and-limits` 时处于兼容模式，直接关掉 MSAA。Chrome 146 已发货兼容模式，可在 OpenGL ES 3.1 级设备上跑 WebGPU [13]。
- **绑定**：`src/nodes/core/UniformGroupNode.js` 按更新频率把 uniform 分成 `frameGroup`、`renderGroup`（共享）与 `objectGroup`；`src/renderers/webgpu/utils/WebGPUBindingUtils.js` 用布局键缓存 `GPUBindGroupLayout`，按版本号决定是否重建 bind group。
- **Render bundle**：`src/renderers/common/BundleGroup.js` 与 `RenderBundles.js` 把静态子树录成一次 `GPURenderBundle`，之后 `executeBundles`；WebGL 后端没有收益。
- **其他**：`Renderer.compileAsync` 预建整个场景的管线；计算节点在 `src/nodes/gpgpu/`（`ComputeNode.js`、`AtomicFunctionNode.js`、`SubgroupFunctionNode.js`）；`reversedDepthBuffer` 参数会翻转 depth bias；`TimestampQueryPool.js` 做 GPU 计时；处理 `device.lost`。

### 2.2 Godot 的 web 导出

- 只支持 Compatibility 渲染器（WebGL 2，`drivers/gles3`，`platform/web/detect.py` 的 `-sMAX_WEBGL_VERSION=2`）；Forward+ 与 Mobile 要等 WebGPU [8]。
- 线程：`detect.py` 的 `threads` 选项（`-sPTHREAD_POOL_SIZE`、`proxy_to_pthread`）。4.3 起默认单线程，因为 SAB 所需的跨源隔离会挡住 itch.io、Poki 等平台上的外部调用；同时重新引入基于 Web Audio 的采样播放；多线程版可用 PWA Service Worker 注入 COOP/COEP [7]。
- 体积：4.3 的 web 构建约 40 MB，brotli 后约 5 MB [7]；`editor/settings/editor_build_profile.cpp` 能从项目检测用到的类，生成给 SConstruct `build_profile` 用的裁剪配置。
- C# 项目不能导出 web；GDExtension 需要 `dlink_enabled`，体积更大 [8]。
- 调试：`platform/web/remote_debugger_peer_messageport.cpp` 注册 `messageport://`，web 版游戏的远程调试走 MessagePort。

### 2.3 UE 没有 web

UE 4.23 弃用 HTML5，4.24 移出引擎交给社区，UE5 从未支持；官方路线是 Pixel Streaming，即服务器渲染后推视频流 [9]。

### 2.4 wgpu 引擎该抄什么

1. **按更新频率分组的绑定模型加版本号**。web 上每次 draw 都要经过 JS、wasm、WebGPU 三层调用，CPU 侧上限由它决定。帧级与相机级 bind group 每帧只建一次，物体级数据走 storage buffer 索引；做了 GPU 驱动后，物体级 bind group 基本消失。
2. **静态批次用 render bundle**。wgpu 有 `RenderBundleEncoder`，three.js 的 `BundleGroup` 说明它在 WebGPU 上能省掉重复编码。与 GPU 驱动结合时，间接参数缓冲每帧由计算着色器改写，bundle 本身不必重录。
3. **渲染器放进 Worker**（要点 10）。编辑器是 React 应用，主线程会被 UI 占用；render spike 测到 61–102 ms 的帧间隔且没找到原因，主线程争用是首要嫌疑，值得用 OffscreenCanvas 加 Worker 渲染做一次对照。默认构建不需要 COOP/COEP（Godot 4.3 的教训），快照继续用 transferable 传递（纲领 5.3）。wasm 体积按提交记录，必要时按项目关闭 cargo feature（Godot build profile 的做法）。
4. **不做 WebGL2 回退是合理的**：three.js 要为此维护两套 builder 和 transform feedback 模拟计算。但应把 WebGPU 兼容模式当作覆盖面选项评估：它能覆盖 GLES 3.1 级设备，代价是更低的限额且没有 MSAA。
5. **对标 three.js 要对它的最佳路径**：`InstancedMesh` 或 `BatchedMesh`，加 `BundleGroup` 与 `compileAsync`，而不是每个物体一个 `Mesh`。纲领的目标是"web 比肩 three.js"，对照不公平会得出错误结论。

## 3. 脚本与调试

### 3.1 UE

- **Blueprint VM**：`UE:Engine/Source/Runtime/CoreUObject/Public/UObject/Script.h` 的 `EExprToken`（约 100 个操作码）。`EX_Breakpoint`、`EX_Tracepoint`、`EX_WireTracepoint` 的注释写明只在编辑器里起作用，其他情况等同 `EX_Nothing`。`Editor/KismetCompiler/Private/KismetCompilerVMBackend.cpp` 的 `EmitInstrumentation` 在 `KCST_DebugSite` 处写入 `EX_Tracepoint`；`KismetCompilerMisc.cpp` 中 `bCreateDebugData = GIsEditor && !IsRunningCommandlet()`，即编辑器构建总是插桩。断点与单步在 `Editor/UnrealEd/Public/Kismet2/KismetDebugUtilities.h`；重编译后由 `Kismet2/KismetReinstanceUtilities.h` 重新实例化已有对象。
- **Verse**：字节码 VM 在 `Runtime/CoreUObject/Public/VerseVM/`（`VVMBytecode.h` 等），编译器在 `Runtime/VerseCompiler/`；失败语义依靠 `Runtime/AutoRTFM/`，它用另一种编译器让现有 C++ 代码获得事务语义（`AutoRTFM::Transact`）。
- **Live Coding**：`Developer/Windows/LiveCoding/`，基于 Live++（`Private/External/LC_API.h`），`bEnableReinstancing` 可选；改动类布局仍不可靠。

### 3.2 Godot

- **VM**：`Godot:modules/gdscript/gdscript_vm.cpp` 用 computed goto（`OPCODE_SWITCH`）；`gdscript_function.h` 约 150 个操作码，含类型化快路径 `OPCODE_OPERATOR_VALIDATED`、`OPCODE_CALL_METHOD_BIND_VALIDATED_RETURN`。调试检查放在 `OPCODE_LINE`：每行检查 `lines_left` 与 `is_breakpoint(line, source)`。
- **协议**：`core/debugger/remote_debugger_peer.cpp` 用 `encode_variant` 把 `[command, Array]` 消息编码后走 TCP。编辑器是服务端（`editor/debugger/editor_debugger_server.cpp`，`network/debug/remote_port`），游戏用 `--remote-debug tcp://127.0.0.1:6007` 连回，断点在启动时经 `--breakpoints source:line` 带上。核心消息：`debug_enter`、`debug_exit`、`stack_dump`、`stack_frame_vars`、`evaluate`、`breakpoint`、`set_skip_breakpoints`、`step`、`next`、`out`、`continue`、`break`、`reload_scripts`。其余功能按前缀注册 capture（`EngineDebugger::register_message_capture`：`core`、`scene`、`profiler`、`servers`、`multiplayer`、`snapshot`）；`scene:` 下有 `request_scene_tree`、`inspect_objects`、`live_node_prop`、`live_create_node` 等实时编辑消息（`scene/debugger/scene_debugger.cpp`）。
- **暂停**：`RemoteDebugger::debug(p_can_continue, p_is_error_breakpoint)` 进入循环，暂停期间仍经 `_try_capture` 处理其他 capture，所以编辑器能在断点处查看远程场景树。运行时错误调用 `GDScriptLanguage::debug_break(err_text, false)`，停在错误处，且不能继续执行。
- **DAP 与 LSP 都在编辑器进程里**：`editor/debugger/debug_adapter/` 把 DAP 翻译成上述协议（`network/debug_adapter/remote_port`）；`modules/gdscript/language_server/` 提供 LSP（`network/language_server/remote_port`），依赖编辑器中活的 ClassDB 与场景缓存。
- **热重载**：`gdscript.cpp` 的 `GDScriptLanguage::reload_scripts` 对每个实例调用 `get_property_state`，按成员名保存 `(StringName, Variant)` 再写回。C# 的 `modules/mono/csharp_script.cpp` 中 `reload_assemblies` 要卸载 AssemblyLoadContext 并序列化委托（`ManagedCallable`），不可回收的委托会阻止重载。

### 3.3 对 TS on QuickJS-ng + CDP 的判断

Pocket3D 已经确定：CDP 端点，编辑器使用同一个 CDP，agent 用 MCP 的 `debug.*`，调试器连接时才重编译插桩（spike 数据：编入补丁但未装 handler 时为原速的 0.98–1.00 倍，装上 handler 后 2.04 倍，改成操作数形式后 1.08 倍）。对照三家之后的结论：

1. **无状态脚本加重载等价检查优于三家的热重载**：GDScript 按成员名搬状态，C# 要序列化委托，UE 要重新实例化对象。这一点保持不变。需要补的是：项目组件的 schema 变化在 swap 时走迁移函数（`hot-update.md` 6 已列出），并把它做成可测的检查。
2. **出错即暂停**（要点 4）。agent 调试最常见的入口是"脚本报错"，Godot 默认就这样处理，而 debugger spike 记录补丁缺"异常暂停"。做法：调试器连接时，在 QuickJS 抛出异常、栈尚未展开的位置进入暂停（需要在 throw 路径加钩子），CDP 发出 `Debugger.paused{reason: "exception"}`；未连接时维持现有规则，停止 tick 并返回带 TS 行号与调用栈的结构化错误。
3. **暂停时的世界视图要包含未提交的写入**。Godot 暂停时仍能查看对象；Pocket3D 的系统是事务（写入先暂存），断点处可见的快照是上一个边界的状态，agent 看不到这个系统正要写什么。`debug.state` 增加 `staged_writes`。
4. **调试传输与 LSP 放在哪里**。CDP 留在 host，不像 Godot 那样放在编辑器里，因为编辑器是网页，权威在 host。web 构建借鉴 Godot 的 `messageport://`：Worker 里的调试核心用 `postMessage` 发 CDP 帧，页面可以再转成 WebSocket 给 DevTools。LSP 不自研：Monaco 自带 TS 语言服务，加上生成的 `.d.ts`；组件注册表变化时 host 推送新的 `.d.ts`（Godot 的 LSP 离不开活的 ClassDB，原因相同）。
5. **启动前即可下断点**。Godot 用 `--breakpoints`；CDP 的对应做法是"等待调试器"，协议里已有 `Runtime.runIfWaitingForDebugger`。补一个 `pocket run --wait-debugger`，让第一个 tick 之前设置的断点也能命中。

## 4. 编辑器架构

### 4.1 UE

- **UI**：`UE:Engine/Source/Runtime/Slate`、`SlateCore`（声明式 C++ UI 框架）。细节面板在 `Editor/PropertyEditor`：由 `FProperty` 与元数据自动生成，可经 `IDetailCustomization`、`IPropertyTypeCustomization` 定制，`EditConditionEvaluator.cpp` 解析 `meta=(EditCondition=…)`。
- **事务**：`Editor/UnrealEd/Classes/Editor/Transactor.h` 的 `FTransaction::FObjectRecord` 保存对象的序列化快照（`SerializedObject` 与 `SerializedObjectFlip`）和 `FTransactionObjectDeltaChange`；改动外面包一层 `FScopedTransaction`（`Editor/UnrealEd/Public/ScopedTransaction.h`），对象在改之前调用 `Modify()`；历史缓冲是 `TransBuffer.h`。
- **PIE**：`Editor/UnrealEd/Private/PlayLevel.cpp` 的 `CreatePIEWorldByDuplication` 在同一进程里复制世界，包名加 `PLAYWORLD_PACKAGE_PREFIX`（`UEDPIE`，见 `Runtime/Core/Public/Misc/CoreMiscDefines.h`）；`PlayLevelNewProcess.cpp` 支持在新进程中运行；Simulate 模式的 `KeepSimulationChanges`（`Editor/LevelEditor/Public/LevelEditorActions.h`）可以把模拟中的改动带回编辑世界。
- **网页 UI 的先例**：Remote Control API 插件在引擎里起 HTTP 与 WebSocket 服务（WebSocket 默认端口 30020，JSON 消息包含 `MessageName`、`Parameters`），让外部网页读写属性、调用函数 [6]。

### 4.2 Godot

- 编辑器本身用引擎的 `Control` 节点写成（`Godot:editor/editor_node.cpp`）。
- **撤销**：`core/object/undo_redo.h` 提供 `create_action(name, MergeMode)`、`add_do_method`/`add_undo_method`、`add_do_property`、`add_do_reference` 和 `commit_action`，`MERGE_ENDS` 让连续拖动合并成一步。`editor/editor_undo_redo_manager.h` 按对象归属分历史：`GLOBAL_HISTORY`、每个场景一条、`REMOTE_HISTORY`（对运行中游戏的远程编辑）。
- **检视器**：`editor/inspector/editor_inspector.cpp` 遍历 `get_property_list()`；`editor_properties.cpp` 的 `EditorInspectorDefaultPlugin::get_editor_for_property` 按 `Variant::Type` 与 `PropertyHint` 选控件；扩展点是 `EditorInspectorPlugin`（`editor_inspector.h`）。
- **进程分离**：游戏是独立进程，通过远程调试协议调试（3.2）；远程场景树在 `editor/debugger/editor_debugger_tree.cpp`，远程检视在 `editor_debugger_inspector.cpp`。Game 视图能把游戏窗口嵌进编辑器：`editor/run/game_view_plugin.cpp`、`embedded_process.cpp`，macOS 用 `platform/macos/display_server_macos_embedded.mm`；`scene:next_frame`、`scene:runtime_node_select_*` 等消息支持逐帧推进和在游戏画面里选节点。编辑器插件与 `@tool` 脚本运行在编辑器进程里。4.6 还提供 LibGodot（`core/extension/libgodot.h`、`godot_instance.cpp`），可以把引擎当库嵌入 [14]。

### 4.3 哪种分离适合"web 编辑器 + 原生宿主"

Pocket3D 实际上组合了两家：UI 在进程外（同 Godot，编辑器崩溃不影响世界，也天然支持多客户端），世界与 Play 在宿主进程内（同 UE PIE，但 Play 是 ECS fork，不需要像 UE 那样复制对象再改包名，隔离由 fork 一致性检查保证）。教训：

1. **撤销记录组件级前后状态，不手写逆操作**（要点 11）。命令目录会持续增长，agent 也会调用，这样任何命令都自动可撤销；PCE 规范编码已有，前后字节就相当于 UE `FTransaction` 的快照差分。Godot 的 `MERGE_ENDS` 必须有，否则一次 Gizmo 拖动会产生几百条历史。
2. **Play 分支有自己的历史，并提供"带回"命令**。Godot 区分了 `REMOTE_HISTORY`，UE 有 `KeepSimulationChanges`。Pocket3D 增加 `play.keep {entities, components}`：把分支上选中实体的组件差异作为一个事务应用到编辑世界。
3. **检视器提示**见要点 12 与 5.4。
4. **Play 中也要能在画面里拾取**。Godot 的 `runtime_node_select` 说明运行时需要在游戏画面里选对象；Pocket3D 已有实体 ID 缓冲，Play 分支应走同一条拾取路径，返回实体路径与组件。
5. **编辑器扩展限定为"命令加面板"**。Godot 的 `EditorPlugin` 和 `@tool` 在编辑器进程里执行任意代码，UE 是 C++ 模块；Pocket3D 的扩展面板应是前端模块，只经命令目录与宿主交互，维持"编辑器没有特权通道"（纲领 3.1）。

## 5. 反射、序列化与 schema

### 5.1 UE

UHT（C# 实现，`UE:Engine/Source/Programs/Shared/EpicGames.UHT/`）解析 `UCLASS`、`UPROPERTY`、`UFUNCTION`，生成 `.generated.h`；运行时是 `FProperty`（`Runtime/CoreUObject/Public/UObject/UnrealType.h`，派生自 `FField`）。同一份反射驱动了带标签的属性序列化（容忍版本差异）、细节面板（`meta` 元数据）、Blueprint 暴露、网络复制、Python 绑定，以及 5.8 的 MCP 工具（`UToolsetDefinition` 中标 `UFUNCTION(meta=(AICallable))` 的静态函数）[1]。

### 5.2 Godot

- `Godot:core/object/class_db.h`：在 `_bind_methods()` 中运行时注册（`GDREGISTER_CLASS`、`ADD_PROPERTY`），不做代码生成。`core/object/property_info.h` 的 `PropertyInfo{type, name, class_name, hint, hint_string, usage}`，旁边的注释要求加字段前先问维护者；`Variant` 是统一的值类型。
- **资源格式**：`scene/resources/resource_format_text.cpp` 写出 `[gd_scene format=N uid="uid://…"]`，外部资源用 `ext_resource`（带 uid 与路径），内嵌资源用 `sub_resource`，只写与默认值不同的属性。资源 UID 在 `core/io/resource_uid.cpp`；脚本、着色器这类没有文件头的资源使用 `.uid` 旁文件（`core/io/resource_loader.cpp`）。
- **绑定生成**：`core/extension/extension_api_dump.cpp` 把 ClassDB 导出为 `extension_api.json`，godot-cpp、godot-rust、C# 绑定都由它生成；C ABI 本身也写成 `gdextension_interface.json`（附 JSON Schema），由 `gdextension_interface_header_generator.cpp` 生成头文件；`misc/extension_api_validation/` 按版本登记允许的破坏性变化，由 `misc/scripts/validate_extension_api.sh` 校验。

### 5.3 Bevy

`bevy_reflect` 的 `TypeRegistry`。BRP（`Bevy:crates/bevy_remote/src/builtin_methods.rs`）提供 JSON-RPC 方法：`registry.schema`（由反射生成 JSON Schema，见 `src/schemas/`）、`rpc.discover`、`world.query`、`world.get_components+watch` 等。场景方面有 `crates/bevy_world_serialization`（反射序列化整个世界）和 0.19 的 BSN（`crates/bevy_scene/src/scene_patch.rs`，可组合的场景补丁）。

### 5.4 与 serde + schemars + 生成 .d.ts 相比

Rust 方案相当于"UE 的编译期反射"加上"Godot 的 JSON 导出"，却不需要 UHT 那样的额外解析器，derive 宏在 cargo 内部完成。教训：

1. **一份 `engine-api.json`**（要点 13）。Godot 的经验是"一个机读文件生成所有语言绑定，并做跨版本检查"。Pocket3D 有 `.d.ts`、JSON Schema、CLI 帮助、MCP 工具表、文档五个消费者，如果各自从 Rust 类型生成，容易互相漂移。
2. **编辑提示放进 schema 扩展**（要点 12），并标注对玩家 agent 的可见性。Godot 的 `usage` 标志（`PROPERTY_USAGE_STORAGE`、`PROPERTY_USAGE_EDITOR` 等）把"存不存、显不显示"与类型分开；Pocket3D 还要额外区分"玩家可感知"与"仅全知视图"。
3. **编辑格式与持久化格式分开**。PCE 是严格、规范、用于哈希的格式；人和 agent 编辑的场景应像 `.tscn`：文本、稳定 ID、只写非默认值（新增字段时不会让所有场景文件都变）、外部资源用 UID 引用。预制件的覆盖用补丁表达（Bevy BSN、Godot 继承场景）。
4. **迁移函数必须覆盖每次 schema 变化**。UE 的标签化序列化可以跳过未知属性；Pocket3D 的严格解码会拒绝未知字段，这对 agent 是对的，但也意味着每次改 schema 都要有迁移，并在 `pocket-check` 里用旧版本快照做回归。

## 6. 资源管线

### 6.1 UE

- **DDC**：`UE:Engine/Source/Developer/DerivedDataCache/Public/DerivedDataBuildFunction.h` 规定 build function 应是纯函数、不保存状态，版本号代表代码，行为一变就必须改版本；另有 `DerivedDataBuildKey.h`、`DerivedDataCacheKey.h`；共享缓存是 `Developer/Zen` 与 `Programs/UnrealCloudDDC`。
- **Cook**：`Editor/UnrealEd/Private/Cooker/` 下有 `CookDirector` 与 `CookWorkerServer`（多进程）、`CookDeterminismManager.h`（检测非确定的输出）、`IncrementalValidatePackageWriter`（增量校验）。
- **虚拟资源**：`Developer/Virtualization/Public/IVirtualizationBackend.h` 与 `Runtime/Core/Public/Virtualization/VirtualizationSystem.h`：包里只留载荷哈希，大载荷按需从后端拉取。

### 6.2 Godot

`Godot:editor/file_system/editor_file_system.cpp` 负责扫描。源文件旁的 `.import`（importer、参数、uid、目标路径）入库；导入产物写到 `.godot/imported/<文件名>-<路径 md5>.<扩展名>`（`core/io/resource_importer.cpp`）；`.md5` 文件里的 `source_md5`、`dest_md5` 与 `.import` 里的 `importer_version` 决定是否重导入（`_test_for_reimport`）。导入器在 `editor/import/`。重导入后，资源在编辑器和运行中的游戏里热替换。

### 6.3 Bevy

`Bevy:crates/bevy_asset/src/processor/mod.rs` 写明四条原则：自动、可配置、无损、确定。`.meta` 文件保存处理器与参数（`meta.rs`）；`processor/log.rs` 是用于崩溃恢复的事务日志；`file_watcher` 特性提供热重载。

### 6.4 教训

1. **两层身份**（要点 3）。按 `architecture.md` 4.2，世界用内容哈希引用资源，那么改一次贴图就要改所有引用。Godot 的 UID 把"它是谁"与"它的字节"分开；replay 的精确性改由锁表保证。
2. **烘焙函数带版本**（要点 14）。产物放在被忽略目录下的内容寻址缓存里，键为 (烘焙函数名, 版本, 参数哈希, 源哈希)；导入参数写在源文件旁的 sidecar（如 `boat.glb.pocket`）并入库。render spike 每次加载都重建 LOD，约 3 s；Godot 和 UE 都只在输入或导入器版本变化时才重做。
3. **烘焙确定性检查**。同一输入烘焙两次、比较产物哈希（UE `CookDeterminismManager`，Bevy 原则 4）。web 只加载原生烘焙的资源，产物不确定会让 `rendering-spec` 里原生与 web 的网格哈希比对失真。
4. **大文件按虚拟资源处理**。仓库里只放哈希与后端位置（UE Virtualization 的做法），也就是现有的 `tools/scripts/data.py` 加 rclone 协议；把它接到资源加载的"缺失"路径上，而不只用于测试数据。
5. **热重载走命令**。重导入完成后作为一条命令在 tick 边界替换（与 `architecture.md` 4.2"不走侧门"一致），渲染端只替换 GPU 资源；Godot 的重导入信号是先例。

## 7. 物理、确定性与网络

### 7.1 UE Chaos

- `UE:Engine/Source/Runtime/Engine/Classes/PhysicsEngine/PhysicsSettings.h`：`bTickPhysicsAsync` 加 `AsyncFixedTimeStepSize` 实现固定步长的异步物理；`FPhysicsPredictionSettings` 有 `bEnablePhysicsPrediction`、`MaxSupportedLatencyPrediction`，历史长度 `GetPhysicsHistoryCount()` 等于最大延迟除以步长后取整。`Runtime/Experimental/Chaos/Public/RewindData.h` 的 `FRewindData` 提供 `RewindToFrame`、`PreResimStep_Internal`。`Runtime/Engine/Classes/Engine/EngineTypes.h` 的 `EPhysicsReplicationMode::{Default, PredictiveInterpolation, Resimulation}`，其中 Resimulation 标注为 Work In Progress。
- 确定性：`Chaos/GeometryParticles.h` 的 `CHAOS_DETERMINISTIC`；`Chaos/PBDCollisionConstraints.h` 注释说确定性需要每 tick 对活跃约束排序，有额外开销。
- Network Prediction 插件（不在稀疏树内）：`ENetworkPredictionService` 分 FixedTick/FixedRollback 与 IndependentTick/IndependentRollback 两组 [15]。5.8 树里还出现了早期的 `Runtime/Experimental/RigidPhysics/`。

### 7.2 Godot

- `Godot:servers/physics_3d/physics_server_3d_manager.cpp` 选择物理引擎；`editor/editor_node.cpp` 的 `get_initial_settings` 让新项目默认使用 `JOLT_PHYSICS_NAME`（4.4 集成、4.6 起新项目默认 [14]）。`modules/jolt_physics/SCsub` 编译了 `DeterminismLog.cpp`，但没有定义 `JPH_CROSS_PLATFORM_DETERMINISTIC`（Jolt 构建文档说明这是编译期开关 [16]）。
- `main/main.cpp`：`physics_ticks_per_second` 默认 60，`max_physics_steps_per_frame` 默认 8（防止死亡螺旋），另有 `physics_jitter_fix`；物理插值在渲染服务器里做（1.2）。
- 网络：`modules/multiplayer` 的 `MultiplayerSynchronizer`、`MultiplayerSpawner` 做状态同步，没有回滚。

### 7.3 教训

1. **后端统一，或显式隔离**（要点 15）。纲领 4.7 的备选方案"原生用 Jolt、web 用 Rapier"会让 replay、fork 一致性和"原生与 wasm 哈希相同"同时失效。Jolt 的跨平台确定性是编译期选项，Godot 的默认构建就没有打开，不能当作默认成立。对照测量要包含"Jolt 打开确定性"这一列（纲领已列），再加一项：同一场景在原生与 wasm 上逐 tick 比较哈希。
2. **追赶上限**。Godot 的 `max_physics_steps_per_frame` 限制每帧最多补几个 tick。Pocket3D 的时间模型已有 `behind_ms`，应补一个明确的上限和降速策略（放弃追赶真实时间，而不是丢 tick），写进 `threads.md` 3.3。
3. **网络先做 lockstep，回滚以后建立在已有的快照环上**。UE 的重模拟是"非确定性加服务器纠错"，需要 `FRewindData` 记录粒子历史；Pocket3D 有逐 tick 哈希和快照，只传输入的 lockstep 与"首个分叉 tick"诊断成本很低。回滚只需在快照环上加输入延迟与重放；`FixedRollback` 只存在于固定步长模式，说明固定步长是前提。
4. **确定性的代价要测出来**。Chaos 承认排序约束有开销，Rapier 的确定性配置关掉了 SIMD 与并行。物理基准同时列"确定"和"非确定"两组数字，"比肩 PhysX/Chaos"的结论才可解释。

## 8. 构建系统

### 8.1 UE

`UE:Engine/Source/Programs/UnrealBuildTool/` 是 C# 写的构建工具，用 `*.Build.cs`、`*.Target.cs` 描述模块与目标；UHT 在 `Programs/Shared/EpicGames.UHT`，构建前生成反射代码；分布式编译靠 `Programs/UnrealBuildAccelerator` 与 `Horde`；`AutomationTool` 与 BuildGraph 负责编排。这一整套存在的原因，是 C++ 既没有模块系统也没有反射。

### 8.2 Godot

`Godot:SConstruct`、`methods.py`、`platform/*/detect.py`（SCons，Python）；`scu_build` 单编译单元；`build_profile` 按类裁剪；着色器在构建时由 `glsl_builders.py`、`gles3_builders.py` 转成头文件；`modules/*/config.py` 控制模块开关。

### 8.3 判断与教训

不自研构建系统是对的：cargo 已经提供了 UBT 的模块图，proc-macro 相当于 UHT 的代码生成；aipocket master 的第二套构建工具也已被否决。可借鉴的是周边做法：

1. **生成物入库并做差**。`.d.ts`、`engine-api.json`、着色器反射结果由 `cargo xtask gen` 生成并入库，`xtask check` 报告未提交的生成差异（Godot 把 `gdextension_interface.json` 到头文件的生成放在构建里）。这样 agent 和审阅者看 diff 就知道接口变了。
2. **着色器在构建期校验**。所有 WGSL（含材质模板和常用特性组合）在 `xtask check` 中过 naga，可选再过 Tint（Dawn 的 `tint` 命令行或无头 Chrome），代替"改着色器后在浏览器里验证"这条纪律。Godot 在构建期处理 GLSL，aipocket 有"naga 通过、Tint 拒绝"的教训。
3. **项目级 feature profile**。按项目关闭不用的 crate 特性，压缩 wasm 体积（Godot `build_profile`）。
4. **编译时间**。UE 用 unity build 与 UBA，Godot 用 SCU；Rust 侧对应的是 crate 划分（已有）和可选的 sccache，不需要更多机制。

## 9. AI 与 agent 集成（2026）

### 9.1 现状

- **UE 5.8**：实验插件 Unreal MCP 在编辑器进程里起 MCP 服务器，只支持 HTTP，默认地址 `http://127.0.0.1:8000/mcp`，只绑回环、没有认证。工具按 toolset 组织（`SceneTools`、`ActorTools`、`MaterialInstanceTools`、`ObjectTools` 等）。扩展方式有三种：Python 继承 `unreal.ToolsetDefinition` 并用 `@toolset_registry.tool_call` 修饰；C++ 继承 `UToolsetDefinition`，写 `UFUNCTION(meta=(AICallable))` 静态函数；底层直接 `IModelContextProtocolModule::AddTool`。随附的 toolset 都不提供 resources 或 prompts [1][2][3]。
- **Unity**：AI Assistant 包内置过 MCP（relay 进程加编辑器内的 Bridge，外部客户端要在 Pending Connections 中批准；工具如 `Unity_ManageScene`、`Unity_ReadConsole`）[4]。2026 年这个编辑器内 MCP 被弃用，由 Unity CLI 取代：`unity command`、`unity eval` 直接驱动编辑器，官方称更快、更省 token；`unity mcp` 只留给不能跑 shell、或拼不好命令行的 agent；`unity skill install <agent>` 安装技能；CLI 不接 agent 也能用于 CI [5]。
- **Godot**：没有官方 MCP。据报道，Godot 基金会 2026-06-30 起禁止 AI 代理提交 PR 和大量 AI 生成的代码 [18]，短期内不会有官方 agent 接口，生态靠第三方插件。
- **Bevy**：BRP（见 5.3），JSON-RPC over HTTP，有 `rpc.discover`、由反射生成的 `registry.schema`、`+watch` 流式订阅；不是 MCP，但机器可读程度高。

### 9.2 agent-native 引擎还应提供什么

三家都只覆盖"agent 当开发者"，而且只是把编辑器操作变成 RPC。Pocket3D 规划的玩家接口、受限感知、fork 与 replay、数据断点、因果链，在它们那里都没有对应物。补充教训：

1. **CLI 为主，MCP 是投影**（要点 5），"在运行中的世界里求值"做成一等命令。Unity 提供 `unity eval`，说明 agent 需要用一行表达式直接问世界。`pocket eval` 在 tick 边界以只读方式执行 TS（沙箱与脚本相同）；需要写操作时在 fork 里执行。
2. **安全**（要点 2）。UE 回环加无认证的做法在浏览器环境里有现实风险：网页可以向 127.0.0.1 发起 WebSocket，Chrome 111 正是为此给 DevTools 加了 Origin 限制。Pocket3D 同时开着 MCP HTTP、编辑器 WS、CDP 三类端点。
3. **写命令自带验证结果**。UE 的 `AICallable` 与 Pocket3D 的命令目录思路相同（工具从类型生成）；差异化在于每个写命令都返回可验证的信息：事务 ID、受影响实体，以及可选的 fork 内预演哈希，让 agent 一步确认结果。第三方 UE MCP 服务器要靠 `describe_graph` 回读来补这一环 [3]，说明官方插件缺这一块。
4. **随引擎分发 agent 技能**。Unity 用 `unity skill install`；Pocket3D 从 `engine-api.json` 生成工作流技能文档（编辑-应用-验证、调试、玩家回合），与引擎版本绑定。
5. **可复现的评测作为卖点**。三家都没有可复现的 agent 评测；Pocket3D 的确定性让"同种子、同输入、同模型"的评测可以复现。建议公开任务集格式（纲领第 6 节已有测量计划）。

## 附 A：对现有文档的事实更正

- 纲领 1.0 第 2 节说 Jolt 是"Godot 4.4 起的默认 3D 物理"。实际上 4.4 把 Jolt 集成为可选项，4.6 起才成为新项目的默认（`Godot:editor/editor_node.cpp` 的 `get_initial_settings`；[14]）。
- `docs/spec/threads.md` 第 1 节仍写 presenter 线程运行 `egui` 编辑器，与纲领 4.5（React web 编辑器）不一致。

## 附 B：网络来源

1. Epic，Unreal MCP in Unreal Editor：https://dev.epicgames.com/documentation/unreal-engine/unreal-mcp-in-unreal-editor?lang=en-US
2. VP Land，Unreal Engine 5.8 Embeds an MCP Server：https://www.vp-land.com/stories/unreal-engine-5-8-embeds-an-mcp-server-so-ai-agents-can-drive-the-editor
3. StraySpark（第三方插件作者的对比文章，立场不中立）：https://www.strayspark.studio/blog/epic-official-mcp-plugin-ue5-8-vs-third-party
4. Unity，MCP get started（AI Assistant 2.0）：https://docs.unity3d.com/Packages/com.unity.ai.assistant@2.0/manual/unity-mcp-get-started.html
5. Unity，Unity CLI as the replacement for the in-Editor MCP server：https://docs.unity.com/en-us/unity-cli/replace-mcp-server-unity-cli
6. Epic，Remote Control API WebSocket Reference：https://dev.epicgames.com/documentation/en-us/unreal-engine/remote-control-api-websocket-reference-for-unreal-engine
7. Godot，Progress report: Web export in 4.3：https://godotengine.org/article/progress-report-web-export-in-4-3/
8. Godot 文档，Exporting for the Web（4.5）：https://docs.godotengine.org/en/4.5/tutorials/export/exporting_for_web.html
9. UE HTML5 社区维护仓库（4.24 起移出引擎）：https://github.com/ufna/UE-HTML5
10. Chrome，What's next for WebGPU：https://developer.chrome.com/blog/next-for-webgpu ；web3dsurvey：https://web3dsurvey.com/webgpu/features/chromium-experimental-multi-draw-indirect/platform/Windows
11. blink-dev，Intent to Ship: WebGPU: Immediates：https://groups.google.com/a/chromium.org/g/blink-dev/c/8LdEcW1CkNo/m/rrV3CUyGAAAJ
12. W3C WebGPU 规范，Limits：https://www.w3.org/TR/webgpu/#limits
13. Chrome，New in WebGPU 146（兼容模式）：https://developer.chrome.com/blog/new-in-webgpu-146
14. Godot 4.6 发布页：https://godotengine.org/releases/4.6/
15. Epic API，ENetworkPredictionService：https://dev.epicgames.com/documentation/unreal-engine/API/Plugins/NetworkPrediction/ENetworkPredictionService
16. Jolt Physics 构建说明：https://raw.githubusercontent.com/jrouwe/JoltPhysics/master/Build/README.md
17. Selenium issue #11750（Chrome 111 的 `--remote-allow-origins`）：https://github.com/SeleniumHQ/selenium/issues/11750
18. GamingOnLinux，Godot Engine to get stricter on AI contributed code：https://www.gamingonlinux.com/2026/07/godot-engine-to-get-stricter-on-ai-contributed-code/
