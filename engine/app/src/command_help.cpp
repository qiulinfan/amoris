#include "command_help.hpp"

#include <algorithm>
#include <unordered_map>

namespace pocket::app {

namespace {

constexpr CommandHelp kHelp[] = {
    {"state", "keys?", "The simulation's tick, time, pause state, exposed script state and hashes; with keys (a list of exposed names) only those values and the tick. After a restart (project.reload, project.apply, env.reset) run_tick is the tick the scripts see (t.tick), counted from the restart; tick is the session's, which events go by."},
    {"perf", "reset?", "Where the time goes: milliseconds per phase (frame, poll, tick, script, physics, world, state, render), and systems: each of the tick's systems (bodies, characters, particles, animation, cloth, navigation, ...) by its average, the costliest first. reset starts the averages over after answering. script.profile breaks the script time down, render.stats.gpu the GPU's by pass."},
    {"step", "ticks?, render?, until?, watch?, every?, keys?", "Run the paused simulation for ticks (1 by default) and answer the state after; headless, only the last tick is drawn unless render is \"each\". until stops early: {event: \"coin.\"}, {state: \"score\", at_least: 3}, {entity, component, field: \"position.y\", below: 0} (equals, above, below, at_least, at_most, changes), {where: \"Plot.stage == 3\"} (until an entity meets a world.query condition; count: n or {at_least: n} for more; the text alone does too); the answer's until says met, tick and what was seen. watch follows values through the step instead of stepping one tick at a time: [\"score\", \"Player:Transform.position.y\"] answers each one's first, last, min and max with the ticks they came at and how often it changed; every: n adds the value every n ticks as [tick, value]. keys: only those exposed values in the answer."},
    {"pause", "", "Stop the simulation clock; frames still draw and commands still work."},
    {"resume", "", "Start the simulation clock again."},
    {"time.scale", "scale?, seconds?", "Slow motion (under 1), fast forward (up to 8) or a hit-stop (0), for seconds of real time or until changed; no params reads it."},
    {"quit", "", "End the runtime (it writes its report)."},
    {"capture", "path?, ids?, pixel?, pixels?, ascii?", "The last frame: written to a PNG at path (under the project when relative; the answer's path is where), with pixel values at pixel {x, y} or pixels [{x, y}], the center and corner pixels and render stats; with ascii (true: 64 across, or a width) a look at it for a model that cannot see: the frame as rows of characters dark to light (\" .:-=+*#%@\"), as colour letters, and its colours by name."},
    {"log.tail", "n?, level?", "The last log lines, optionally at or above a level."},
    {"report", "", "The run's full report: state, hashes, world summary, events, errors."},
    {"commands", "usage?, text?, family?, search?", "Every command name; usage: true adds each one's parameters and summary, text: true the same as one line each (compact); family (world, render, ...) or search (a word) narrows the list."},
    {"help", "command?, family?, search?, sdk?", "How to call a command: its parameters and what it does; without a command, all of them, or a family's or those matching search. sdk: a script function's signature and documentation (\"timer.after\", \"world.get\", \"RayHit\"), the exports whose names hold the text, or with \"\" the SDK's parts and their exports."},
    {"transcript", "since_tick?, until_tick?, max_lines?, tolerance?, debounce?", "The run compressed into segments: what each exposed value did and which events happened, for reading instead of stepping tick by tick."},
    {"world.spawn", "name?, parent?, components?, cause?", "Create an entity with components ({Transform: {...}, MeshRenderer: {...}}); answers its id and path."},
    {"world.destroy", "entity, cause?", "Remove an entity and its children."},
    {"world.get", "entity, component?, field?, components?", "One component's values of an entity, or every component it has (by name) when none is named; field picks one field or a path into it (\"position.y\", \"layers.1.height\"); components lists several."},
    {"world.set", "entity, component, value, components?, cause?, quiet?", "Change a component (or several: components: {Transform: {...}, MeshRenderer: {...}}, answered as {ok, values}): value holds the fields to change ({color: {r, g, b, a}}; a named code by its name, {kind: \"point\"}; a key may be a path into a field, {\"position.y\": 2} or {\"lods.1.ratio\": 0.2}, a number picking a list's element, and the rest stays as it was); the component is added when missing. Answers {ok, value}, the component as it now is (quiet: true answers {ok} alone, for callers that do not read it); a field it does not have or a value of the wrong type is refused and nothing is applied."},
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
    {"world.query", "with?, without?, name?, under?, where?, fields?, limit?", "Entities with (or without) components, a name pattern (* any run of characters, ? one: \"Coin*\", \"Segment_?\"; matched against the whole name), under an entity, whose fields meet where: text (\"Plot.water == 0 and Plot.stage >= 0\"; == != > < >= <=) or an object ({\"Plot.water\": 0, \"Health.current\": {\"below\": 20}}), an enum field by its value's name; fields picks what to return: components (\"Transform\") or their fields (\"Transform.position\", \"Transform.position.y\")."},
    {"world.summary", "", "Counts: entities, roots, components in use."},
    {"world.lint", "limit?", "What is likely wrong with the world: parts that do nothing together (a Collider without a RigidBody), files that are not there, names that name nothing, no active camera, script errors; each problem with its severity (error, warning, info), entity, component, what it does and a fix; ok when there is no error."},
    {"world.schema", "component?, components?, search?", "Every component: its fields, types, defaults and docs; or one (component), several (components), or those whose name, fields or docs mention search."},
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
    {"events.last_seq", "", "The newest event's sequence number, as {seq}: give it to events.since later to read only what came after."},
    {"events.why", "seq, limit?", "An event's chain of causes, as a story. In place of seq, event: a type (or its start, \"coin.\") explains the latest event of it."},
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
    {"render.ids", "path?, limit?", "The entity id buffer: visible entities with pixel counts, optionally written as a PNG at path (under the project when relative)."},
    {"render.views", "path, entity?, views?", "The scene, or an entity and what is under it, drawn from standard views at once into one sheet (a PNG at path, under the project when relative), without moving anything: views from front, back, right, left, top, bottom, perspective (the first six but bottom by default), each framing its bounds. What an agent looks at to check what it built from every side."},
    {"render.compare", "path, tolerance?, threshold?, diff?, update?", "Hold the last frame against a reference PNG (recorded on first use): match, differing fraction, where."},
    {"render.viewport", "x?, y?, w?, h?, width?, height?, reset?", "Draw the scene into a rectangle of the window (the editor's scene pane)."},
    {"render.shadows", "enabled?, strength?, bias?, cascades?, distance?, softness?, contact?, contact_length?", "The sun's cascaded shadows: on or off, darkness, bias, cascades, reach; softness (the sun's radius in degrees, penumbrae widening with distance), contact shadows and their length."},
    {"render.msaa", "samples?", "Multisampling: 1 or 4."},
    {"render.bloom", "enabled?, threshold?, strength?, radius?", "The glow of light over white: on or off, threshold, strength, spread."},
    {"render.ambient", "color?, intensity?", "The flat light every surface gets from all around where there is no Sky (a 2D game's, or a scene without one): color as a picker shows it (\"#202030\", {r, g, b} or [r, g, b]) and intensity; low for a dungeon lit by its torches. [render] ambient in project.toml does the same at start."},
    {"render.grade", "enabled?, exposure?, filmic?, temperature?, contrast?, saturation?, tint?, vignette?, lut?, lut_strength?", "The frame's look: exposure, warmth, contrast, saturation, tint, vignette, and a look-up table image (N slices of N by N side by side) applied to the finished colors."},
    {"render.tonemap", "operator?, exposure?, auto_exposure?, compensation?, min_ev?, max_ev?, speed?", "How the HDR scene becomes the frame: exposure (fixed or metered) and the operator (none, aces, agx, neutral)."},
    {"render.ao", "enabled?, radius?, intensity?, samples?", "Ambient occlusion: the sky's and ambient light darkened where geometry crowds a point (crevices, contact with the ground)."},
    {"render.oit", "enabled?", "Order-independent transparency (weighted blended): translucent meshes blend without being sorted, so interleaved ones look alike whichever is drawn first. Not with MSAA."},
    {"render.probes", "refresh?", "The reflection probes in use (entity, layer, center, size, captured, bounces: the captures still to make, the frame of their last capture) and the irradiance volumes (entity, center, size, probes along each axis, ready once every probe is captured, passes still to make, the next probe); refresh captures every one again (three times, the light bouncing)."},
    {"render.ssr", "enabled?, max_distance?, max_roughness?, steps?, thickness?, intensity?", "Screen-space reflections: glossy surfaces (up to max_roughness) reflect what is on screen, found by marching the mirror ray through the depth, in place of the sky's reflection."},
    {"render.toon", "enabled?, bands?, softness?, outline?, outline_color?, opacity?", "A cel look: the sun's and the lights' light on surfaces in `bands` flat steps (3), their edges softened by `softness` (0.04 of a band), and an outline `outline` pixels across (2; 0 none) round every entity where the id under the pixels changes, in `outline_color` (\"#rrggbb\" or {r, g, b}, near black) at `opacity`. Any setting turns it on; enabled: false turns it off. Also [render.toon] in project.toml. Answers the settings."},
    {"render.colorblind", "mode?, simulate?, strength?", "Colour vision: the finished frame corrected for players with a dichromacy (mode protanopia, deuteranopia or tritanopia: the colours they cannot tell apart are shifted into ones they can), or with simulate the frame as they see it, for a designer checking that what matters still reads; off by default, strength 0..1 (1). The interface is drawn after it and keeps its own colours. Answers the settings."},
    {"capture.gif", "path, seconds?, every?, width?", "An animated picture of play: steps seconds (2) of ticks, drawing each, takes a frame every `every` ticks (2), makes it width pixels across (480), and writes a looping GIF of one 256-colour palette to path (project-relative, .gif). Answers the frames, size and bytes. Show a person what the game does, not a still."},
    {"render.scale", "scale?, dynamic?, target_ms?, least?, sharpen?, pixelated?", "Render scale: the window's view drawn at scale (0.25..1) of its width and height and stretched up to fill it, sharpened (sharpen 0..1, 0.25); fewer pixels for weak GPUs and dense screens. dynamic moves the fraction between least (0.5) and scale to keep the GPU's frame under target_ms (12; needs the GPU's timestamps). pixelated stretches it with hard pixel edges instead (scale 0.25 and pixelated: a retro, low-resolution look). Cameras with viewports or targets of their own draw whole. Answers the settings and what was drawn (scale, width, height)."},
    {"render.ssgi", "enabled?, distance?, rays?, steps?, thickness?, intensity?", "Screen-space global illumination: light bounced off what is on screen onto what is near it (a red wall reddening the floor beside it), gathered at half size by rays (2 a pixel) marched up to distance (3) through the depth, blended over frames. Answers the settings."},
    {"render.dof", "enabled?, focus?, aperture?, max_blur?", "Depth of field: a lens focused at `focus` world units; what is nearer or farther blurs, `aperture` being the blur of something at infinity as a fraction of the view's height."},
    {"render.motion_blur", "enabled?, strength?, samples?", "Motion blur: each pixel smeared along its motion (camera and objects) over `strength` of the frame."},
    {"render.taa", "enabled?, feedback?", "Temporal anti-aliasing: the view jittered inside the pixel every frame and the frames blended through the motion of the camera and objects; smooth edges and fine detail over a few frames."},
    {"render.debug", "colliders?, joints?, bounds?, axes?, nav?, lights?, paths?, all?", "Engine overlays drawn as lines: colliders, joints, bounds, axes, paths, the navigation grid, lights (a point light's reach, a spot's cone, the sun's direction)."},
    {"debug.line", "a, b, color?, ticks?", "Draw a line for some ticks."},
    {"debug.box", "center, half, rotation?, color?, ticks?", "Draw a box for some ticks."},
    {"debug.sphere", "center, radius, color?, ticks?", "Draw a sphere for some ticks."},
    {"debug.clear", "", "Remove the debug lines scripts and agents drew."},
    {"debug.stats", "", "How many debug lines are up."},
    {"assets.list", "", "The project's files under assets/ by kind (mesh, image, tilemap, audio) with sizes, and its scripts."},
    {"assets.describe", "path, ascii?", "A mesh's vertices, materials, parts, lights and cameras; a map's layers; an image's size and what it looks like: coverage, the drawn box, its colours by name with shares and hex, mirror symmetry, and with ascii (true: 32 across, or a width) itself in characters (\" .:+#\" by coverage) and in colour letters, for a model that cannot see."},
    {"assets.reload", "path?", "Forget decoded files (one or all) so they load again from disk."},
    {"assets.stats", "", "Counts of loaded meshes, images and failures."},
    {"assets.preview", "path, size?, out?, image?", "A model drawn on its own (framed from three quarters above, under a sky and a sun, off screen): written to `out` as PNG and returned as base64 PNG with image: true. The scene's frame is untouched."},
    {"assets.import", "path, force?", "Read a model now (OBJ, STL, glTF, or through Blender: .blend, .fbx, .dae, .usd, ...) and describe it."},
    {"input.map", "actions", "Set the action map: {name: {keys, axis, ...}}."},
    {"input.actions", "", "The action map as it is."},
    {"input.describe", "", "Actions with the keys, buttons and axes bound to them."},
    {"input.hold", "key | action, ticks?, sign?, value?", "Press a key, a mouse button (mouse:left) or an action for ticks, through the same path as real input."},
    {"input.press", "key | action, sign?, value?", "Press a key or an action for one tick; a key still down goes up and down again, so presses a tick apart are two."},
    {"input.release", "key? | action?", "Let go now of what input.hold holds: a key, an action's keys both ways, or every held key when neither is given."},
    {"input.axis", "action, value", "Set an action's value directly, -1 to 1, until set again (0 lets go): a stick tilted part way, an on-screen stick (touch.ts); past 0.5 either way the action is down. Journaled like any input."},
    {"input.touch", "x, y, phase?, finger?, pressure?", "A finger down, moving or up at window points, pressing 0..1 (1 by default)."},
    {"input.pad", "pad?, button | axis, pressed?, value?", "A gamepad button or axis."},
    {"input.rumble", "pad?, low?, high?, ms?, pattern?, repeat?, stop?", "Shake a gamepad's motors: one step, or a pattern of {low, high, ms} steps played in turn (repeat times); stop silences it. Each step is an input.rumble world event."},
    {"input.state", "", "What is down now: keys, actions and their values, fingers, pads, held keys, gesture settings, the cursor."},
    {"input.cursor", "locked?, visible?", "Capture the pointer (locked: hidden, held in the window, motion as mouse:x/y however far) or hide it; Escape lets it go and a click takes it again. Answers locked (asked for), held (captured now; never headless or in the editor), visible, and x and y: where the pointer is, in the window's pixels (what render.unproject and render.pick take)."},
    {"audio.play", "clip, volume?, pitch?, pan?, lowpass?, reverb?, loop?, entity?, tag?, bus?, spatial?, near?, range?, occlusion?, doppler?", "Start a clip (project-relative WAV, Ogg or MP3) on a bus (main by default); answers the voice id. A spatial voice follows its entity: volume and pan by where it is, pitch by how it moves (doppler)."},
    {"audio.stop", "voice | clip | tag | bus | all", "Stop voices; answers how many."},
    {"audio.set", "voice, volume?, pitch?, pan?, lowpass?, reverb?, loop?, bus?", "Change a playing voice (or move it to another bus)."},
    {"audio.bus", "name, volume?, muted?, lowpass?, highpass?, echo?, echo_feedback?, echo_mix?, reverb?, duck_by?, duck_amount?, duck_seconds?", "A bus: its voices set as one (music, effects, dialogue): volume, mute, a low-pass and a high-pass over its mix, an echo (seconds apart, feedback, mix), its share of the room's reverb, and ducking to duck_amount while a voice plays on the duck_by bus. Answers the bus."},
    {"audio.buses", "", "Every bus with its settings, its ducking gain right now and its voices."},
    {"audio.list", "", "Every voice with its position, duration, volume, pan."},
    {"audio.analyze", "clip", "What a clip sounds like, for an agent that cannot hear it: seconds, audible_seconds, peak, loudness_db, attack_ms, decay_ms, onsets and onsets_per_second, pitch over time as segments {from, to, hz, note}, tonal (the share of the sound with a pitch), brightness_hz (the spectral centroid) and character in words (\"short, bright, tonal, rising, sharp attack\"). Recipes (.sfx), scores (.song) and files alike."},
    {"audio.clips", "", "Loaded clips with lengths and sizes."},
    {"audio.stats", "", "The audio device, voices, clips, master volume."},
    {"audio.reverb", "room?, damping?, mix?", "The room every voice plays in: how long its reverb rings, how fast its high end dies, how loud it is (room 0 is dry)."},
    {"audio.master", "volume?, muted?", "The master volume and mute."},
    {"physics.stats", "", "Bodies, contacts, joints and step times of the last step."},
    {"physics.cloth", "entity, points?", "Where a Cloth is now: its particles across and down, the box around them and the lowest one; points: true lists every particle, row by row from the top-left."},
    {"physics.raycast", "origin, direction, max_distance?, include_triggers?, mask?", "The first collider along a ray."},
    {"physics.sweep", "origin, direction, radius?, max_distance?, include_triggers?, mask?", "The first collider a moving sphere touches."},
    {"physics.overlap", "center, radius?, shape?, size?, height?, rotation?, characters?, mask?", "Every collider (and Character) a shape placed at center overlaps: a sphere of radius (1), a box of half extents size, or a capsule of radius and height, turned by rotation; characters: false leaves the Characters out."},
    {"physics.contacts", "", "The touching pairs of the last step."},
    {"physics.gravity", "gravity?", "The world's gravity vector."},
    {"physics.joints", "", "Every joint with its current values."},
    {"physics.layers", "", "The collision layers by name and bit."},
    {"physics.ignore", "a, b, ignore?", "Let two entities pass through each other (or not)."},
    {"physics.ignored", "mask?", "The pairs set to ignore each other."},
    {"combat.hitscan", "from, direction, range?, damage?, knockback?, team?, shooter?, cause?", "A shot that arrives at once: the nearest solid collider or character capsule along the ray within range (100), the shooter's own left out; the Health on it, or on its nearest ancestor with one, takes damage (10) by the hitbox rules (team, invulnerability, a `hit` event with the point, `health.depleted`) and is pushed knockback units a second along the shot. Answers {hit, entity, path, point, normal, distance, target, landed, health}; {hit: false} when nothing is in range."},
    {"physics2d.raycast", "from, to | direction, distance?, mask?", "The nearest 2D rigid body shape (or TileMap cell) a segment meets: entity, path, point, normal, distance; {hit: false} when none."},
    {"physics2d.overlap", "center, half? | radius, angle?, mask?", "The 2D rigid bodies whose shapes a box (half extents, turned by angle) or a circle at center touches."},
    {"physics2d.impulse", "entity, impulse, point?, angular?", "Push a 2D rigid body now: an impulse (mass times velocity) at a world point (its center of mass by default) and an angular one; it wakes."},
    {"physics2d.stats", "", "The 2D rigid body simulation: bodies, awake, shapes, joints, contacts, the TileMaps it collides with and their shapes, the last step's milliseconds."},
    {"nav.bake", "min?, max?, cell?, agent_radius?, agent_height?, max_step?, max_slope?, diagonal?, layers?, entity?, mode?, jump_height?, jump_range?, max_drop?", "Build the walkability grid from the colliders in a box, or from a tile map's entity. From colliders, every floor of a column up to layers (4; 1 the top only) is walkable where the agent fits: a bridge and the road under it, a building's storeys, joined where a step, a ramp or stairs lead from one to the next."},
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
    {"sprite.sheet", "path, prefix?", "Read a sprite sheet Aseprite exported (its JSON, hash or array frames): a clip per tag named prefix.tag (the file's name by default), each frame its own rectangle and time, reverse and ping-pong tags followed."},
    {"sprite.clips", "", "The defined sprite clips."},
    {"sprite.play", "entity, clip?, loop?, speed?, fps?, restart?, cause?", "Play a sprite clip on an entity."},
    {"sprite.stop", "entity, reset?, cause?", "Stop an entity's sprite clip."},
    {"particles.stats", "", "Live particles per emitter."},
    {"particles.preset", "entity, name, set?", "Make an entity's ParticleEmitter one of the tuned looks: fire, smoke, sparks (burst them), explosion (burst it), rain, snow (both over a 30 by 30 square, on the GPU), dust, fireflies, magic; `set` changes fields over it ({rate: 20, color: {...}}). Answers the emitter, whose fields are then the game's to change."},
    {"particles.burst", "entity, count?, at?, speed?", "Emit a burst from an emitter: from its place, or from the world point `at` ([x, y, z] or {x, y, z}); particle speeds times `speed` (default 1)."},
    {"particles.clear", "entity?", "Remove live particles."},
    {"particles.list", "entity?, limit?", "An emitter's live particles summed up (how many, the box they fill, their mean speed and age) and the first `limit` of them (10) with positions, velocities and ages."},
    {"animation.clips", "entity | mesh", "The animation clips a model has."},
    {"animation.play", "entity, clip?, loop?, speed?, time?, fade?, restart?, cause?", "Play a skeletal clip on an entity (fade cross-fades)."},
    {"animation.stop", "entity, reset?, cause?", "Stop an entity's clip."},
    {"animation.pose", "entity", "The joints' positions and rotations now."},
    {"animation.layer", "entity, index?, clip?, weight?, mask?, additive?, playing?, loop?, speed?, time?, remove?, cause?", "Add, change or remove a layered clip on an entity."},
    {"animation.param", "entity, name, value | values", "Set a parameter of the entity's AnimationGraph (what its transitions' conditions and blend spaces read), or several with values {name: value} (true and false are 1 and 0; an unknown name sets none); answers the state and every parameter."},
    {"animation.trigger", "entity, name", "Fire a trigger parameter of the entity's AnimationGraph: 1 until a transition that reads it is taken."},
    {"path.info", "entity", "A Path's length in world units, whether it is closed, its two ends."},
    {"path.sample", "entity, distance | fraction", "The point a distance along a Path (or a fraction of its length) and the way it runs there."},
    {"path.nearest", "entity, point", "How far along a Path the point nearest a given one lies, that point, and how far away it is."},
    {"render.post", "effects?", "A project's post effects, in order after the tonemap (the interface is drawn after them): effects is a list of shader files in the project (\"effects/crt.wgsl\"), or {shader | code, name?, params?: up to 8 numbers, enabled?}. Each is WGSL that defines fn effect(uv: vec2f) -> vec4f, with sample_frame(uv), param(i), resolution() and time() to call; the answer gives, per effect, ok or the compiler's message. Without effects, the ones running."},
    {"window.info", "", "The game's window: its size in points and pixels, the pixel density, whether it is fullscreen, its title, whether the run is headless."},
    {"window.set", "fullscreen?, width?, height?, title?", "Change the window: fullscreen on or off (in a browser only during a click or key press), its size in points when windowed, its title; answers as window.info. Headless, the requests are remembered."},
    {"world.mark", "name?", "Keep the world as it is now under a name (\"default\"), for world.diff: every entity's saved components."},
    {"world.diff", "since?, limit?", "What changed since a mark: entities spawned (with their components) and destroyed, and for the rest the components added or removed and each changed field's value then and now ({Transform: {fields: {position: [then, now]}}}), a renamed or moved entity's path; lists cut at limit (50)."},
    {"animation.library", "mesh, files, translations?", "Clips from other files onto a model, matched by joint name (a Mixamo \"mixamorig:Hips\" finds \"Hips\"): a file of one clip gives it the file's name, one of several keeps their names; translations \"root\" keeps only the root's moves (the model's bone lengths kept). Answers the clips added and the channels left out; [animations] in project.toml does the same at start."},
    {"mesh.create", "name, positions, indices?, normals?, uvs?, colors?, double_sided?", "A mesh made from numbers, drawn and collided as \"mesh:<name>\" (MeshRenderer.mesh, Collider.mesh with shape mesh): positions three numbers a vertex (flat, [x, y, z] or {x, y, z}), indices three a triangle (counter-clockwise seen from the front; without them every three positions), normals made from the triangles when not given (shared vertices smooth, separate ones flat), uvs two a vertex, colors sRGB three or four a vertex. The same name again replaces it; world.save and save slots carry it. Answers the path, counts and bounds."},
    {"mesh.list", "", "The meshes made by mesh.create, with their sizes."},
    {"mesh.remove", "name", "Forget a made mesh (what still names it draws nothing)."},
    {"tilemap.create", "name, width, height, tile_width?, tile_height?, orientation?, layers?, tilesets?, stagger_axis?, stagger_index?, hex_side?", "A map made by code, kept under name (a project-relative path such as maps/dungeon.tmj) for a TileMap to draw: width by height tiles of tile_width by tile_height pixels (16), orthogonal (or isometric, staggered, hexagonal), tile layers by name (\"ground\"; or {name, solid}) all empty, tilesets [{image, tile_width?, tile_height?, spacing?, margin?, solid?: [local ids]}] numbered on from 1. Fill it with tilemap.set and tilemap.fill on an entity drawing it; tilemap.save {path} writes it; world.save and save slots carry it."},
    {"tilemap.text", "name, rows, legend, tilesets?, layers?, tile_width?, tile_height?", "A map drawn in characters: rows of text, a legend from a character to a tile on a layer ({layer, tile} with tile a local id of the first tileset, or of tileset; {layers: [...]} for several, bottom first), to an object ({object: \"coin\", name?, under?}: a point at the cell's center named Coin_1, Coin_2 ... in an object layer \"objects\"), or null; \"*\" puts a tile under every cell, a space is empty. layers [{name, solid}] orders and marks them. Kept like a map made by code: a TileMap names it; tilemap.spawn turns its objects into prefabs. Answers the layers, tilesets and objects counted by type."},
    {"tilemap.info", "entity", "A map's size, layers, tilesets and objects."},
    {"tilemap.paths", "entity, layer?, smooth?", "Tiled's polylines and polygons (of one object layer, or all) as Path entities named after the objects, their points where the map draws them, a polygon closed (docs/design/paths.md)."},
    {"tilemap.rows", "entity, layer?, solid?, tile_x?, tile_y?, width?, height?", "The map read as rows of characters (the reverse of tilemap.text): per tile layer, a character for each tile used (. empty) and a legend from character to tile; with solid: true, the collision (# solid, - one way, / slope, . free). A window with tile_x, tile_y, width, height."},
    {"tilemap.tile", "entity, x, y | tile_x, tile_y, layer?", "What is in a cell on every layer."},
    {"tilemap.solid", "entity, x, y | tile_x, tile_y", "Whether a point or cell is solid."},
    {"tilemap.sight", "entity, from, to, layers?", "Whether a straight line between two world points crosses no cell that hides what is behind it (a solid cell, or a tile of one of `layers`; a tile's opaque property says otherwise): visible, distance, and blocked_at {tile_x, tile_y, point, distance} when it is not. Orthogonal maps."},
    {"tilemap.fov", "entity, from, radius?, layers?", "The cells seen from a world point within radius cells (8): each cell whose center or a point near a corner is in sight, the walls facing the viewer included; answers cells [[tile_x, tile_y], ...], count and walls. For fog of war, a guard's view, a roguelike's light."},
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
    {"terrain.info", "entity?", "A Terrain's size, height range, resolution, source (a heightmap or noise), lowest and highest points, whether it was sculpted, its revision. The first terrain without entity."},
    {"terrain.height", "x, z, entity?", "The ground at world x, z: its height, the point and the normal there, whether it is inside the terrain, the paint there ({r, g, b, a}, a how much it covers) once the terrain is painted, and with textured layers each layer's share there (layers: [{layer, name, share}])."},
    {"terrain.paint", "x?, z?, points?, color?, layer?, radius?, amount?, mode?, entity?", "Paint the ground around world x, z, or along a stroke through points ([{x, z}, ...]: a path, a road, a river bank) in one go: mode paint (default) lays color ({r, g, b} in sRGB) over the ground's own colours, or with layer (an index or a name of Terrain.layers) that textured layer over the others, erase takes paint (or that layer's paint) away; amount (0.5) at the centre fading to nothing at radius (3). Emits terrain.painted."},
    {"terrain.paints", "entity?, paint?, layers?", "The paint grid, sample after sample as in terrain.heights, four numbers each (r, g, b in sRGB and a, how much it covers; with layers true, the textured layers' paint, each of four layers' share): read, or set whole with paint (an empty array clears it)."},
    {"locale.get", "", "The language scripts' `t` uses now, the project's default ([locale] default) and the languages it has a file for (locales/<lang>.json)."},
    {"locale.set", "lang", "Switch the game's language (its file must exist); scripts' interfaces draw again in it. Emits locale.changed."},
    {"locale.table", "lang?", "A language's texts by key (nested groups flattened to dotted keys), the current language by default."},
    {"locale.check", "base?", "Every language against the default (or `base`): the keys it lacks, the keys only it has, and the texts whose {placeholders} differ; `complete` when none lack anything."},
    {"timeline.play", "entity, path?, time?, speed?, loop?", "Play a timeline file on an entity (its Timeline component, added if it has none): from `time` (0), at `speed`, looping or not; answers its duration and what in the world it cannot move."},
    {"timeline.stop", "entity", "Stop the entity's timeline where it is (playing false)."},
    {"timeline.seek", "entity, time", "Put the entity's timeline at a time (within its duration) and apply its tracks there at once: no tick passes and no event fires. An editor's playhead; a look at a moment of a cutscene."},
    {"timeline.info", "path, entity?", "A timeline file read and checked against the world: its duration, each track's target, keys and problem, its events; `entity` stands for tracks that move the Timeline's own entity."},
    {"wind.at", "x, z", "The air's motion at world x, z now (docs/design/wind.md): its velocity and speed, the gust there (-1..1), where the Wind blows to and its entity; entity null and a velocity of zero with no Wind."},
    {"water.height", "x, z, entity?", "The water's surface over world x, z now (the first Water covering it, or the one given): its height, the point, the normal, the water's velocity there (waves and current), the rest level and the bottom; entity null and inside false where no water is."},
    {"terrain.sculpt", "x, z, radius?, amount?, mode?, target?, entity?", "Reshape the ground around world x, z: mode raise (default), lower, flatten (toward the height target, or the height at the centre) or smooth, by amount at the centre fading to nothing at radius (3). Emits terrain.sculpted."},
    {"terrain.save", "path, paint?, layers?, entity?", "Write the heights as a 16-bit greyscale PNG in the project and make it the Terrain's heightmap; with paint true, the paint as an RGBA PNG made its paintmap; with layers true, the textured layers' paint as an RGBA PNG (a channel a layer) made its layermap."},
    {"terrain.reset", "entity?, paint?, layers?", "Heights made again from the heightmap or the noise, sculpting dropped; with paint true, the paint instead: read again from the paintmap, or cleared; with layers true, the textured layers' paint, from the layermap."},
    {"net.info", "", "The lockstep game (docs/design/networking.md): this peer's mode (off, host, player), its player number, the players, whether the game started, the input delay, the port (host), who is connected and who left, desyncs found, and whether the next tick waits for someone's input."},
    {"scatter.copies", "entity, limit?", "Where a Scatter's copies stand: each one's point, size and shade (the first limit, 100), and how many were placed."},
    {"terrain.heights", "entity?, heights?", "The grid of heights, row after row along z (resolution squared numbers, 0..height): read, or set whole with heights."},
    {"save.write", "slot, data?", "Save the game into a slot (with the scripts' own data)."},
    {"save.read", "slot", "Load a slot back."},
    {"save.list", "", "The slots there are."},
    {"save.delete", "slot", "Remove a slot."},
    {"save.dir", "", "Where the slots are kept."},
    {"script.start", "name", "Start a loaded script context."},
    {"script.reload", "name?, start?", "Load a script context again (the project's by default), keeping the world."},
    {"script.contexts", "", "The loaded script contexts."},
    {"script.diagnostics", "diagnostics?", "The project's type errors ({file, line, column, severity, message}) as `pocket run/editor --watch` last found them after a rebundle; diagnostics sets them (and logs each). `pocket check` finds them without a runtime."},
    {"script.eval", "source", "Run JavaScript in the scripts' global scope and answer its value: the SDK's exports are names there (world.spawn(...), events.since(...), input.axis(...); pocket has them all), and a failure answers {error} with its message and the stack in the source's lines."},
    {"script.profile", "reset?", "Where the scripts' time goes since the profile last started over (reset: start over after answering): each handler (tick, frame, input, contacts, start, stop) and each exposed value's getter (expose, run every tick for state) with its name, the file and line it was registered at, calls, ms, ms_per_call, max_ms and share of the script time; each command the scripts called with calls, ms and calls_per_tick (a handler's ms includes its commands'); component_reads and component_writes, the numbers-only world.get and world.set."},
    {"project.info", "", "The project's name, directories, settings and scripts."},
    {"project.brief", "", "What to know first, as text: the files, the scene's roots, the components in use and the project's own, the input actions, the exposed state, the events so far, what lint finds, when ticks are slow where the time goes (the scripts, their exposed values, the costliest system), and where to look next."},
    {"project.reload", "scene?, scripts?, settings?", "A fresh world from the scene with the bundled scripts started over it, and project.toml's settings read again (input map, audio buses and room, render and physics settings, sprite clips; settings defaults to scripts); a restart of both starts as a fresh run does (random numbers from the run's seed again, the scripts' tick and time and the world's wind, waves and cloth from 0 again, the last run's sounds stopped, held actions let go; the session's tick and the event log carry on); lists sources newer than the bundle as stale. After editing scripts use project.apply."},
    {"project.apply", "ticks?", "After editing scripts or project.toml: bundle and type-check the project with the pocket tool, reload it and step ticks (1); answers the type errors, the reload, the state and any script errors. A bundle that fails leaves the running project as it was."},
    {"project.save_scene", "path?", "Write the world as the project's scene."},
    {"project.write", "path, text | json", "Write a project file."},
    {"project.read", "path", "Read a project file."},
    {"ui.apply", "ops", "Apply element operations (create, set, append, remove...)."},
    {"ui.snapshot", "root?, depth?, max_nodes?, layout?, styles?", "The interface as text: one line per element (root: an id or an element's name)."},
    {"ui.query", "type?, text?, name?", "Find elements by type, text or name."},
    {"ui.describe", "id", "Everything about one element (by its id, or its name)."},
    {"ui.hit", "x, y", "The element under a point."},
    {"ui.focus", "id", "Give an element the keyboard focus (by its id, or its name)."},
    {"ui.click", "id | x, y, button?, clicks?, mods?", "Click an element (by its id, or its name: {id: \"start\"}) or a point, as a player would."},
    {"ui.drag", "id | x, y, dx, dy, steps?, button?", "Drag from an element (by its id, or its name) or a point."},
    {"ui.type", "text", "Type text into the focused input."},
    {"ui.key", "key, mods?", "Press a key (Return, Tab, Escape, Z with mods [meta]...)."},
    {"ui.wheel", "id | x, y, dx?, dy?", "Scroll over an element (by its id, or its name) or a point."},
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
    // Every command call looks its help up (to check its keys): by hash, not along the table.
    static const std::unordered_map<std::string_view, const CommandHelp*> index = [] {
        std::unordered_map<std::string_view, const CommandHelp*> m;
        for (const CommandHelp& h : kHelp) m.emplace(h.name, &h);
        return m;
    }();
    const auto it = index.find(name);
    return it == index.end() ? nullptr : it->second;
}

const std::vector<std::string>& command_param_names(const CommandHelp& h) {
    static const std::unordered_map<const CommandHelp*, std::vector<std::string>> names = [] {
        std::unordered_map<const CommandHelp*, std::vector<std::string>> m;
        for (const CommandHelp& c : kHelp) m.emplace(&c, command_param_names(c.params));
        return m;
    }();
    return names.at(&h);
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
