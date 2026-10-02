# Lessons from Unity

Unity is the second teacher, for the editor model rather than the engine core. The engine is closed
source, but Unity publishes its C# reference source (`UnityCsReference`) under the Unity
Reference-Only License. A read-only clone lives at `~/Reference/UnityCsReference` (snapshot "Unity
6000.7.0a6 C# reference source code").

## Reference policy

Same as Unreal: read for design, never copy code, identifiers or comments. The license permits
reference only; it forbids modification and redistribution.

## Where to look

| Topic | Path in UnityCsReference |
|---|---|
| UI Toolkit core: VisualElement, styles, layout, events, bindings | `Modules/UIElements/Core` |
| UI Toolkit editor side: EditorWindow hosting, inspectors, drawers, live reload | `Modules/UIElementsEditor`, `Editor/Mono/UIElements` |
| Serialized properties and objects (the inspector data model) | `Editor/Mono/SerializedProperty`, `Editor/Mono/SerializedProperty.bindings.cs`, `Editor/Mono/SerializedObject.bindings.cs` |
| Inspector framework and custom editors | `Editor/Mono/Inspector` |
| Undo | `Editor/Mono/Undo`, `Editor/Mono/Undo.cs` |
| Asset database and import pipeline | `Editor/Mono/AssetDatabase`, `Editor/Mono/AssetPipeline` |
| Package manager and its UI | `Modules/PackageManager`, `Modules/PackageManagerUI` |
| Scripting binding pattern (managed side of the engine API) | `Runtime/Export` |

## What to learn from

1. **UI Toolkit.** One retained-mode UI for editor and runtime with a web-like model: an element
   tree, flexbox layout through Yoga, stylesheets (USS), event capture and bubbling, runtime data
   binding, a UI debugger and a visual builder. Pocket UI (ADR 0004) adopts the shape with
   TypeScript and TSX instead of C# and UXML.
2. **SerializedObject and SerializedProperty.** Inspectors edit a reflected, undoable, multi-object
   view of data rather than live objects; prefab overrides and undo fall out of it. Pocket3D's
   inspector edits the document through commands over the same generated metadata.
3. **Asset pipeline.** Every asset has a stable id, importers produce derived data into a cache,
   references point at ids rather than paths. Pocket3D keeps stable ids and a content-addressed
   derived-data cache; sidecar metadata stays text.
4. **Package Manager.** Packages with a manifest, semantic versions, local, git and registry
   sources, and samples. Pocket3D's TypeScript packages and editor extensions follow this model.
5. **Editor scripting surface.** Menu items, custom windows and property drawers declared from code
   and discovered automatically. Pocket3D discovers them from TypeScript packages through metadata.
6. **Text serialization with stable ordering** made scenes mergeable; the lesson is to start
   text-first.
7. **Runtime modernization** (the CoreCLR migration, Burst, DOTS) shows that runtime choices made
   early are the hardest to change. Pocket3D picks V8 and an ECS from the start.

## What to avoid

1. **Two of everything during transitions**: IMGUI and UI Toolkit, uGUI and UI Toolkit, built-in and
   scriptable render pipelines, old and new input, MonoBehaviour and DOTS. Pocket3D introduces a
   system only when it can replace the old one completely, and never keeps both.
2. **Domain reload and slow play-mode iteration.** Script hot reload is designed in, not
   retrofitted.
3. **GameObject and MonoBehaviour overhead for everything.** Hot data belongs in ECS storage.
4. **Scene files with unstable ordering and id churn** caused merge pain for years; schemas and
   stable ordering from day one.
5. **Closed source.** Users cannot learn from or fix the engine; Pocket3D is open and
   reference-friendly.
