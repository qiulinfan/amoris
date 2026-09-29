#include "command_help.hpp"

#include <algorithm>

namespace pocket::app {

namespace {

constexpr CommandHelp kHelp[] = {
    {"state", "", "The simulation's tick, time, pause state, exposed script state and hashes."},
    {"perf", "", "Where the time goes: milliseconds per phase (frame, poll, tick, script, physics, world, state, render)."},
    {"step", "ticks?", "Run the paused simulation for ticks (1 by default) and answer the state after."},
    {"pause", "", "Stop the simulation clock; frames still draw and commands still work."},
    {"resume", "", "Start the simulation clock again."},
    {"time.scale", "scale?, seconds?", "Slow motion (under 1), fast forward (up to 8) or a hit-stop (0), for seconds of real time or until changed; no params reads it."},
    {"quit", "", "End the runtime (it writes its report)."},
    {"capture", "path?, ids?, pixel?, pixels?", "The last frame: written to a PNG at path, with pixel values at pixel {x, y} or pixels [{x, y}], the center and corner pixels and render stats."},
    {"log.tail", "n?, level?", "The last log lines, optionally at or above a level."},
    {"report", "", "The run's full report: state, hashes, world summary, events, errors."},
    {"commands", "usage?", "Every command name; usage: true adds each one's parameters and summary."},
    {"help", "command?", "How to call a command: its parameters and what it does; without a command, all of them."},
    {"transcript", "since_tick?, until_tick?, max_lines?, tolerance?, debounce?", "The run compressed into segments: what each exposed value did and which events happened, for reading instead of stepping tick by tick."},
    {"world.spawn", "name?, parent?, components?, cause?", "Create an entity with components ({Transform: {...}, MeshRenderer: {...}}); answers its id and path."},
    {"world.destroy", "entity, cause?", "Remove an entity and its children."},
    {"world.get", "entity, component", "One component's values of an entity."},
    {"world.set", "entity, component, value, cause?", "Change a component: value holds the fields to change ({color: {r, g, b, a}}); the component is added when missing."},
    {"world.remove", "entity, component, cause?", "Take a component off an entity."},
    {"world.has", "entity, component", "Whether an entity has a component."},
    {"world.describe", "entity", "Everything about one entity: name, path, parent, children and every component's values."},
    {"world.find", "path", "The id of the entity at a path or with a name (Player, /Level/Player); null when none."},
    {"world.children", "entity", "The ids of an entity's children."},
    {"world.roots", "", "The ids of the entities without a parent."},
    {"world.components", "entity", "The component names an entity has."},
    {"world.reparent", "entity, parent?, keep_world?, cause?", "Move an entity under another (no parent: to the root)."},
    {"world.rename", "entity, name, cause?", "Give an entity a new name."},
    {"world.tree", "root?, depth?, max_entities?, values?, components?", "The scene as text: one line per entity with the fields that differ from defaults."},
    {"world.query", "with?, without?, name?, under?, fields?, limit?", "Entities with (or without) components, a name pattern, under an entity; fields picks component values to return."},
    {"world.summary", "", "Counts: entities, roots, components in use."},
    {"world.schema", "", "Every component: its fields, types, defaults and docs."},
    {"world.save", "entity?", "The world (or an entity and its children) as a scene document."},
    {"world.load", "path | scene, clear?, cause?", "Load a scene file (project-relative) or a scene document, clearing the world first unless clear is false."},
    {"world.instantiate", "prefab | mesh | scene, name?, parent?, components?, position?, cause?", "Make entities from a prefab file, a model's node tree (with its lights and cameras) or a scene document."},
    {"world.save_prefab", "entity, path", "Write an entity and its children as a prefab file."},
    {"world.pack", "component, fields, with?, without?, name?, under?, limit?", "Many entities' fields as flat numbers in one call (for scripts moving thousands)."},
    {"world.unpack", "component, fields, count?", "Write packed numbers back (the other half of world.pack)."},
    {"world.update_transforms", "", "Recompute world transforms now instead of at the end of the tick."},
    {"world.clear", "cause?", "Destroy every entity."},
    {"events.emit", "type, subject?, data?, cause?", "Add an event to the causal log (scripts' own events)."},
    {"events.since", "seq? | since?, limit?, type?", "Events after a sequence number, oldest first; type filters by prefix (player.)."},
    {"events.recent", "limit? | n?, type?", "The most recent events."},
    {"events.histogram", "seq?", "How many events of each type (since a sequence number)."},
    {"events.last_seq", "", "The newest event's sequence number."},
    {"events.why", "seq, limit?", "An event's chain of causes, as a story."},
    {"recorder.start", "ticks?", "Keep the last ticks of the world for time travel."},
    {"recorder.stop", "", "Stop recording (what was kept stays)."},
    {"recorder.clear", "", "Forget what was recorded."},
    {"recorder.status", "", "What the recorder holds: first and last tick, entities."},
    {"recorder.at", "tick, entity?", "The world (or one entity) as it was at a tick."},
    {"recorder.diff", "from?, to?, entity?, limit?", "What changed between two recorded ticks."},
    {"recorder.track", "entity, component?, field?, from?, to?, every?", "One field's values over the recorded ticks."},
    {"recorder.first", "entity, component?, field?, op?, value, from?", "The first recorded tick where a field compares (op: <, >, ==, ...) to a value."},
    {"render.stats", "", "The last frame: draw calls, instances, lights, shadows, tone mapping, sky, missing assets."},
    {"render.pick", "x, y", "The entity under a pixel of the last frame."},
    {"render.project", "point | entity", "Where a world point or an entity lands on screen, in pixels."},
    {"render.unproject", "x, y, plane?, at?", "The world ray under a pixel, and where it meets a plane."},
    {"render.visible", "limit?, path?", "What the camera sees, largest first: coverage, pixel bounds, center."},
    {"render.ids", "path?, limit?", "The entity id buffer: visible entities with pixel counts, optionally written as a PNG."},
    {"render.compare", "path, tolerance?, threshold?, diff?, update?", "Hold the last frame against a reference PNG (recorded on first use): match, differing fraction, where."},
    {"render.viewport", "x?, y?, w?, h?, width?, height?, reset?", "Draw the scene into a rectangle of the window (the editor's scene pane)."},
    {"render.shadows", "enabled?, strength?, bias?, cascades?, distance?", "The sun's cascaded shadows: on or off, darkness, bias, cascades, reach."},
    {"render.msaa", "samples?", "Multisampling: 1 or 4."},
    {"render.bloom", "enabled?, threshold?, strength?, radius?", "The glow of light over white: on or off, threshold, strength, spread."},
    {"render.grade", "enabled?, exposure?, filmic?, temperature?, contrast?, saturation?, tint?, vignette?", "The frame's look: exposure, warmth, contrast, saturation, tint, vignette."},
    {"render.tonemap", "operator?, exposure?, auto_exposure?, compensation?, min_ev?, max_ev?, speed?", "How the HDR scene becomes the frame: exposure (fixed or metered) and the operator (none, aces, agx, neutral)."},
    {"render.ao", "enabled?, radius?, intensity?, samples?", "Ambient occlusion: the sky's and ambient light darkened where geometry crowds a point (crevices, contact with the ground)."},
    {"render.debug", "colliders?, joints?, bounds?, axes?, nav?, all?", "Engine overlays drawn as lines: colliders, joints, bounds, axes, the navigation grid."},
    {"debug.line", "a, b, color?, ticks?", "Draw a line for some ticks."},
    {"debug.box", "center, half, rotation?, color?, ticks?", "Draw a box for some ticks."},
    {"debug.sphere", "center, radius, color?, ticks?", "Draw a sphere for some ticks."},
    {"debug.clear", "", "Remove the debug lines scripts and agents drew."},
    {"debug.stats", "", "How many debug lines are up."},
    {"assets.list", "", "The project's files under assets/ by kind (mesh, image, tilemap, audio) with sizes, and its scripts."},
    {"assets.describe", "path", "A mesh's vertices, materials, parts, lights and cameras; an image's size; a map's layers."},
    {"assets.reload", "path?", "Forget decoded files (one or all) so they load again from disk."},
    {"assets.stats", "", "Counts of loaded meshes, images and failures."},
    {"assets.import", "path, force?", "Read a model now (OBJ, STL, glTF, or through Blender: .blend, .fbx, .dae, .usd, ...) and describe it."},
    {"input.map", "actions", "Set the action map: {name: {keys, axis, ...}}."},
    {"input.actions", "", "The action map as it is."},
    {"input.describe", "", "Actions with the keys, buttons and axes bound to them."},
    {"input.hold", "key | action, ticks?, sign?, value?", "Press a key or an action for ticks, through the same path as real input."},
    {"input.press", "key | action, sign?, value?", "Press a key or an action for one tick."},
    {"input.touch", "x, y, phase?, finger?", "A finger down, moving or up at window points."},
    {"input.pad", "pad?, button | axis, pressed?, value?", "A gamepad button or axis."},
    {"input.rumble", "pad?, low?, high?, ms?", "Shake a gamepad's motors."},
    {"input.state", "", "What is down now: keys, actions and their values, fingers, pads, held keys, gesture settings."},
    {"audio.play", "clip, volume?, pitch?, pan?, lowpass?, reverb?, loop?, entity?, tag?, spatial?, near?, range?, occlusion?", "Start a clip (project-relative WAV or Ogg); answers the voice id."},
    {"audio.stop", "voice | clip | tag | all", "Stop voices; answers how many."},
    {"audio.set", "voice, volume?, pitch?, pan?, lowpass?, reverb?, loop?", "Change a playing voice."},
    {"audio.list", "", "Every voice with its position, duration, volume, pan."},
    {"audio.clips", "", "Loaded clips with lengths and sizes."},
    {"audio.stats", "", "The audio device, voices, clips, master volume."},
    {"audio.reverb", "room?, damping?, mix?", "The room every voice plays in: how long its reverb rings, how fast its high end dies, how loud it is (room 0 is dry)."},
    {"audio.master", "volume?, muted?", "The master volume and mute."},
    {"physics.stats", "", "Bodies, contacts, joints and step times of the last step."},
    {"physics.raycast", "origin, direction, max_distance?, include_triggers?, mask?", "The first collider along a ray."},
    {"physics.sweep", "origin, direction, radius?, max_distance?, include_triggers?, mask?", "The first collider a moving sphere touches."},
    {"physics.overlap", "center, radius, mask?", "Every collider within a radius."},
    {"physics.contacts", "", "The touching pairs of the last step."},
    {"physics.gravity", "gravity?", "The world's gravity vector."},
    {"physics.joints", "", "Every joint with its current values."},
    {"physics.layers", "", "The collision layers by name and bit."},
    {"physics.ignore", "a, b, ignore?", "Let two entities pass through each other (or not)."},
    {"physics.ignored", "mask?", "The pairs set to ignore each other."},
    {"nav.bake", "min?, max?, cell?, agent_radius?, agent_height?, max_step?, max_slope?, diagonal?, entity?, mode?, jump_height?, jump_range?, max_drop?", "Build the walkability grid from the colliders in a box, or from a tile map's entity."},
    {"nav.path", "from, to, smooth?, mesh?", "A path between two points: its corners and length."},
    {"nav.reachable", "from, to", "Whether one point can be walked to from another."},
    {"nav.nearest", "point | entity, radius?", "The nearest walkable point."},
    {"nav.info", "", "The grid: size, cells, how many are walkable, the navmesh."},
    {"nav.mesh", "", "The navmesh's polygons and portals."},
    {"nav.agents", "", "Every NavAgent with its goal, state and path."},
    {"nav.clear", "", "Drop the grid."},
    {"env.describe", "", "The environment the project exposes: actions, observation keys, episode length."},
    {"env.reset", "seed?, max_ticks?, ...", "Start an episode again; answers the first observation."},
    {"env.step", "actions?, ticks?, ...", "Act for ticks and answer the observation, reward terms and whether the episode is done."},
    {"env.observe", "...", "The observation now."},
    {"sprite.clip", "name, texture?, columns?, rows?, frames?, first?, count?, fps?, loop?, ...", "Define a sprite animation clip on a sheet."},
    {"sprite.clips", "", "The defined sprite clips."},
    {"sprite.play", "entity, clip?, loop?, speed?, fps?, restart?, cause?", "Play a sprite clip on an entity."},
    {"sprite.stop", "entity, reset?, cause?", "Stop an entity's sprite clip."},
    {"particles.stats", "", "Live particles per emitter."},
    {"particles.burst", "entity, count?", "Emit a burst from an emitter."},
    {"particles.clear", "entity?", "Remove live particles."},
    {"particles.list", "entity?, limit?", "Live particles with positions and ages."},
    {"animation.clips", "entity | mesh", "The animation clips a model has."},
    {"animation.play", "entity, clip?, loop?, speed?, time?, fade?, restart?, cause?", "Play a skeletal clip on an entity (fade cross-fades)."},
    {"animation.stop", "entity, reset?, cause?", "Stop an entity's clip."},
    {"animation.pose", "entity", "The joints' positions and rotations now."},
    {"animation.layer", "entity, index?, clip?, weight?, mask?, additive?, playing?, loop?, speed?, time?, remove?, cause?", "Add, change or remove a layered clip on an entity."},
    {"tilemap.info", "entity", "A map's size, layers, tilesets and objects."},
    {"tilemap.tile", "entity, x, y | tile_x, tile_y, layer?", "What is in a cell on every layer."},
    {"tilemap.solid", "entity, x, y | tile_x, tile_y", "Whether a point or cell is solid."},
    {"tilemap.objects", "entity, layer?", "The map's placed objects."},
    {"tilemap.spawn", "entity, prefabs, layer?, parent?", "Make entities from a map's objects by type ({type: prefab path})."},
    {"tilemap.cell", "entity, x, y", "The cell under a world point."},
    {"tilemap.set", "entity, x, y | tile_x, tile_y, layer?, gid | id | clear, tileset?, flip_h?, flip_v?", "Put a tile into a cell (or clear it)."},
    {"tilemap.fill", "entity, tile_x, tile_y, width, height, layer?, gid | id | clear, tileset?, flip_h?, flip_v?", "Fill a rectangle of cells."},
    {"tilemap.save", "entity, path?", "Write the edited map back to its file (or another)."},
    {"tilemap.add_layer", "entity, name, visible?, opacity?, solid?, properties?", "Add a tile layer."},
    {"tilemap.remove_layer", "entity, name", "Remove a tile layer."},
    {"tilemap.layer", "entity, name, visible?, opacity?, solid?, properties?, rename?, index?", "Change a layer: visibility, opacity, solidity, name, order."},
    {"tilemap.add_tileset", "entity, name, image, tile_width?, tile_height?, spacing?, margin?, tiles?", "Add a tileset from an image."},
    {"tilemap.remove_tileset", "entity, name", "Remove an unused tileset."},
    {"tilemap.copy", "entity, name?", "Give the entity its own copy of the map to edit apart."},
    {"save.write", "slot, data?", "Save the game into a slot (with the scripts' own data)."},
    {"save.read", "slot", "Load a slot back."},
    {"save.list", "", "The slots there are."},
    {"save.delete", "slot", "Remove a slot."},
    {"save.dir", "", "Where the slots are kept."},
    {"script.start", "name", "Start a loaded script context."},
    {"script.reload", "name?, start?", "Load a script context again (the project's by default), keeping the world."},
    {"script.contexts", "", "The loaded script contexts."},
    {"script.eval", "source", "Run JavaScript in the project's context and answer its value."},
    {"project.info", "", "The project's name, directories, settings and scripts."},
    {"project.reload", "scene?, scripts?", "A fresh world from the scene with the scripts started over it (after edits)."},
    {"project.save_scene", "path?", "Write the world as the project's scene."},
    {"project.write", "path, text | json", "Write a project file."},
    {"project.read", "path", "Read a project file."},
    {"ui.apply", "ops", "Apply element operations (create, set, append, remove...)."},
    {"ui.snapshot", "root?, depth?, max_nodes?, layout?, styles?", "The interface as text: one line per element."},
    {"ui.query", "type?, text?, name?", "Find elements by type, text or name."},
    {"ui.describe", "id", "Everything about one element."},
    {"ui.hit", "x, y", "The element under a point."},
    {"ui.focus", "id", "Give an element the keyboard focus."},
    {"ui.click", "id | x, y, button?, clicks?, mods?", "Click an element or a point, as a player would."},
    {"ui.drag", "id | x, y, dx, dy, steps?, button?", "Drag from an element or a point."},
    {"ui.type", "text", "Type text into the focused input."},
    {"ui.key", "key, mods?", "Press a key (Return, Tab, Escape, Z with mods [meta]...)."},
    {"ui.wheel", "id | x, y, dx?, dy?", "Scroll over an element or a point."},
    {"ui.stats", "", "Element counts and the interface's paint stats."}
};

std::size_t distance(std::string_view a, std::string_view b) {
    std::vector<std::size_t> row(b.size() + 1);
    for (std::size_t j = 0; j <= b.size(); ++j) row[j] = j;
    for (std::size_t i = 1; i <= a.size(); ++i) {
        std::size_t diag = row[0];
        row[0] = i;
        for (std::size_t j = 1; j <= b.size(); ++j) {
            const std::size_t up = row[j];
            row[j] = std::min({row[j] + 1, row[j - 1] + 1, diag + (a[i - 1] == b[j - 1] ? 0 : 1)});
            diag = up;
        }
    }
    return row[b.size()];
}

}  // namespace

std::span<const CommandHelp> command_helps() { return kHelp; }

const CommandHelp* command_help(std::string_view name) {
    for (const CommandHelp& h : kHelp) if (h.name == name) return &h;
    return nullptr;
}

std::vector<std::string> command_param_names(std::string_view params) {
    std::vector<std::string> out;
    std::string word;
    auto flush = [&]() {
        while (!word.empty() && (word.back() == ' ' || word.back() == '?')) word.pop_back();
        std::size_t start = word.find_first_not_of(' ');
        std::string k = start == std::string::npos ? std::string() : word.substr(start);
        if (!k.empty() && k != "..." && std::find(out.begin(), out.end(), k) == out.end()) out.push_back(k);
        word.clear();
    };
    for (char c : params) {
        if (c == ',' || c == '|') flush();
        else word += c;
    }
    flush();
    return out;
}

bool command_params_open(std::string_view params) { return params.find("...") != std::string_view::npos; }

Json command_help_json(const CommandHelp& h) {
    Json j;
    j["command"] = std::string(h.name);
    j["usage"] = std::string(h.name) + (h.params.empty() ? std::string() : " {" + std::string(h.params) + "}");
    j["params"] = command_param_names(h.params);
    j["summary"] = std::string(h.summary);
    return j;
}

std::vector<std::string> command_suggestions(std::string_view name, std::size_t max) {
    std::vector<std::pair<std::size_t, std::string_view>> scored;
    for (const CommandHelp& h : kHelp) {
        std::size_t d = distance(name, h.name);
        // A name without its family (spawn for world.spawn) is close too.
        const std::size_t dot = h.name.find('.');
        if (dot != std::string_view::npos) d = std::min(d, distance(name, h.name.substr(dot + 1)) + 1);
        scored.emplace_back(d, h.name);
    }
    std::stable_sort(scored.begin(), scored.end(), [](const auto& a, const auto& b) { return a.first < b.first; });
    std::vector<std::string> out;
    for (const auto& [d, n] : scored) {
        if (out.size() >= max || d > std::max<std::size_t>(3, name.size() / 3)) break;
        out.emplace_back(n);
    }
    return out;
}

}  // namespace pocket::app
