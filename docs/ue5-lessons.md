# Lessons from Unreal Engine 5

Unreal Engine 5 is the teacher for this project: both the positive example and the cautionary one. A
read-only reference clone lives outside the repository at `~/Reference/UnrealEngine` (branch
`release`, sparse checkout of `Engine/Source`, `Engine/Build` and `Engine/Config`).

## Reference policy

- The Unreal Engine source is licensed under Epic's EULA. It may be read to understand designs. No
  code, identifiers, comments, data tables or shader snippets are copied into Pocket3D, and nothing
  derived from it is committed. When a design is adopted, cite the UE concept by name in the
  decision record and write our own implementation.
- PocketEngine and aipocket are our own code and may be ported freely.

## Where to look in the reference

| Topic | Path |
|---|---|
| Build tool | `Engine/Source/Programs/UnrealBuildTool` (ModuleRules, TargetRules, ToolChain, ActionGraph), `Engine/Build/BatchFiles` |
| Reflection and objects | `Engine/Source/Runtime/CoreUObject`; UnrealHeaderTool under `Engine/Source/Programs/Shared/EpicGames.UHT` |
| Core utilities, containers, HAL | `Engine/Source/Runtime/Core` |
| Rendering abstraction | `Engine/Source/Runtime/RHI`, `RHICore`, `RenderCore`, `Renderer` |
| Editor framework | `Engine/Source/Editor/UnrealEd`, `Engine/Source/Runtime/Slate` |
| Automation and testing | `Engine/Source/Programs/AutomationTool`, `Engine/Source/Runtime/Core/Public/Misc/AutomationTest.h` |
| Python editor scripting | `Engine/Plugins/Experimental/PythonScriptPlugin` (add `Engine/Plugins` to the sparse checkout when needed) |

## What to learn from

1. **Explicit module graph with public and private dependencies.** Kept as is in `module.toml`.
2. **Target types.** Editor, game, client, server and programs are one code base with different
   defines and module sets.
3. **Platform abstraction layers.** `HAL` and per-platform toolchain classes isolate platform code.
4. **Reflection drives everything.** Serialization, the details panel, Blueprints, networking and
   Python bindings all read one metadata source. Pocket3D does the same with a metadata DSL now and
   standard reflection later.
5. **RHI as an explicit abstraction** over D3D12, Vulkan and Metal, with a render graph (RDG) built
   on it.
6. **Derived Data Cache.** A content-addressed cache of cooked assets, shareable across machines.
7. **Automation as a product.** UAT, BuildGraph, Gauntlet and the automation test framework make CI
   and headless verification normal; Unreal Insights makes profiling normal.
8. **Editor scripting in Python and a dedicated new language (Verse).** Epic itself moved away from
   C++ plus Blueprint as the only authoring path; a typed, tooling-friendly scripting language is
   the direction.

## What to avoid

1. **The build requires .NET and imperative C# descriptions.** Tools and agents cannot reason about
   `Build.cs`; ours are declarative TOML.
2. **Invasive reflection macros and generated headers** (`UCLASS`, `GENERATED_BODY`,
   `*.generated.h`) couple every header to a code generator. Ours keeps metadata beside the code and
   generates into a separate directory.
3. **UObject overhead everywhere**: garbage collection, reflection cost and indirection for objects
   that never needed them. Ours keeps hot data in ECS storage and reflects only what the editor and
   scripts need.
4. **Compile times and dependence on unity builds and PCH.** Modules are huge; unity builds hide
   include hygiene. Ours keeps modules small, disables unity builds by default and measures compile
   time in CI.
5. **Binary assets** (`.uasset`, `.umap`) make diffs and merges opaque. Ours uses text formats with
   schemas and stable ordering; bulk data is separate.
6. **Editor and engine intertwined**; headless use is possible but awkward. Ours makes headless a
   first-class configuration.
7. **Global state** (`GEngine`, `GWorld`, static registries) makes tests and multiple worlds hard.
   Ours passes explicit handles.
8. **Blueprint as the main scripting path** produced unversionable spaghetti; a typed text language
   is the primary path here.
9. **Sheer size.** Every subsystem exists because a shipping game needed it; a pocket engine adds
   subsystems only when a sample needs them.
