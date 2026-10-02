# Rendering

`engine/renderer` is a forward renderer on the WebGPU-shaped RHI (`engine/rhi`, wgpu-native today,
browser WebGPU later). It draws what the world says, every frame, from scratch: no retained scene
graph beside the ECS, so an agent's `world.set` is the whole API.

`samples/showcase` (a courtyard at dusk: a procedural sky and a low sun with cascaded shadows,
volumetric fog the colonnade cuts into shafts, a reflecting pool, lanterns with shadows, a spot on
crates in a pavilion whose polished floor a reflection probe keeps from mirroring the sky, AgX,
bloom, ambient occlusion and TAA, the camera circling, three built-in humanoids strolling a path
round the pool and fireflies drifting as GPU particles) turns nearly everything below on at once;
`pocket editor showcase` opens it, and `renderer_tests` (`[showcase]`) checks that every part runs.

## One frame

1. Camera: the first active `Camera` (perspective, or orthographic for 2D) or a default one.
   `render.viewport` confines the output to a rectangle (the editor's scene pane).
2. Lights: the first directional `Light` is the sun; point and spot lights, as many as the scene
   has, go into a storage buffer and a froxel grid over the view (Many lights, below); a scene
   without a directional light gets a default key light so nothing is invisible.
3. Objects: one row per `MeshRenderer` entity (one per material for glTF meshes) and one per
   `Sprite`, written into a single storage buffer (`model`, normal matrix, color, entity id, uv
   rectangle). Rows are sorted by texture, mesh and submesh; each run of equal mesh, submesh and
   material is one instanced draw, `instance_index` selecting the row. Three thousand cubes are one
   draw (`docs/evidence/swarm.md`).
4. Shadow passes: cascaded shadow maps from the sun, four 2048x2048 depth layers by default. The
   view from the camera's near plane out to `distance` (80 units, no farther than the scene's bounds
   or the camera's far plane) is cut into slices by depth, mostly logarithmically so the near slices
   are short; each slice's frustum corners get a bounding sphere (its radius rounded up so it does
   not breathe as the camera turns) and an orthographic view from the sun that reaches back to the
   far side of the scene for casters, snapped to its texel grid so edges do not crawl as the camera
   moves. Every mesh casts into every cascade (one pass each, the cascade's matrix bound with a
   dynamic offset). A lookup picks the cascade covering the point's view depth, blends into the next
   over the last tenth of it, moves a texel and a half out along the surface's normal against acne,
   and filters 3x3 with a slope-scaled bias (or, under a sun given a size, as wide as the penumbra:
   Soft and contact shadows, below). So a post a few steps away has a crisp shadow and a house fifty
   units off still casts one.
5. Scene pass: a half-float HDR color target (`RGBA16Float`) plus an `R32Uint` id target and a depth
   buffer. Lighting is computed in linear light: colors set on components are decoded from sRGB
   (what a color picker shows) and base color and emissive maps are sampled through sRGB views,
   while metallic-roughness and normal maps are read as stored; values over 1 survive, so an
   emissive of 4 is four times white and a lamp can outshine the sun. With MSAA on
   (`[render] msaa = 4` or `render.msaa {samples: 4}`) the colors are drawn four samples per pixel
   and resolved into the HDR target, and the ids come from a pass of their own at one sample per
   pixel, since an integer target cannot be multisampled; `render.stats` reports `msaa` and the
   `id_draws` of that pass. Meshes are lit with a metallic-roughness model (Lambert diffuse, GGX
   specular with Schlick's Fresnel and Smith's masking, ambient reflected by dielectrics and metals
   in their own colors, point and spot lights with a windowed quadratic falloff) and shadowed by a
   3x3 comparison filter with a slope-scaled bias. Sprites follow, unlit and alpha blended (in
   linear light), sorted by layer then far to near, without depth writes. Bodies of `Water` are
   drawn after the solid and translucent meshes, from copies of the scene and of the prepass depth:
   refraction, absorption with depth, reflections, a sun glint and foam (`docs/design/water.md`).
6. Bloom, when it is on (`render.bloom {enabled: true}` or `[render] bloom = true`): what is
   brighter than the threshold (in linear light, so a threshold of 1 picks out only what is over
   white) goes into a half-size HDR texture, is blurred across and down by a nine-tap gaussian, and
   is added back onto the scene, so lights, emissive surfaces and white sprites glow; four small
   draws, counted in `draw_calls`, and `render.stats` reports `bloom`.
7. The final pass draws the frame from the HDR target: the exposure (`render.tonemap`, times the
   grade's), multiplied by what the auto exposure metered when it is on, then the tone-mapping
   operator, then the grade when it is on (warmth, saturation and the tint in linear light, the
   vignette, contrast on the encoded values), then the sRGB encoding into the 8-bit frame; outside
   `render.viewport` the frame keeps the clear color. `render.stats` reports `hdr`, `tonemap`,
   `auto_exposure` and `grade`. The interface is painted afterwards, in sRGB, so a menu over a
   graded scene keeps its own colors.
8. The id target is what `render.pick`, `render.ids` and the editor's click-to-select read;
   `render.project` maps world points to pixels of the same frame.

## Settings and stats

`render.shadows {enabled, strength, bias, cascades, distance, softness, contact, contact_length}`
(or `[render] shadows = false`, `shadow_cascades`, `shadow_distance`, `shadow_softness`,
`contact_shadows`, `contact_shadow_length` in `project.toml`) controls the shadow maps and their
softness (Soft and contact shadows, below; `render.stats` reports `shadow_cascades`, the
`shadow_distance` they reached, `soft_shadows` and `contact_shadows`); `render.msaa {samples}` (or
`[render] msaa = 4`) turns multisampling on or off; `render.taa {enabled, feedback}` (or
`[render] taa = true`) turns temporal anti-aliasing on (Temporal anti-aliasing, below); `render.dof`
and `render.motion_blur` (or `[render.dof]`, `[render.motion_blur]`) blur by depth and by motion
(Depth of field and motion blur, below); `render.ssr` (or `[render] ssr = true`) traces reflections
on screen (Screen-space reflections, below); `render.ssgi` (or `[render] ssgi = true`) bounces light
off what is on screen (Screen-space global illumination, below);
`render.bloom {enabled, threshold, strength, radius}` (or `[render] bloom = true` with
`bloom_threshold`, `bloom_strength`, `bloom_radius`) turns the glow on and shapes it (threshold 0.8,
strength 0.6 and radius 1 by default; a `MeshRenderer` with a white `emissive` glows at any
threshold);
`render.tonemap {operator, exposure, auto_exposure, compensation, min_ev, max_ev, speed}` (or
`[render.tonemap]` in `project.toml`) turns the HDR scene into the frame (Light and color below);
`render.grade {enabled, exposure, filmic, temperature, contrast, saturation, tint, vignette}` (or
`[render.grade]` in `project.toml` with the same keys, on unless it says `enabled = false`) sets the
frame's look: exposure 1, contrast 1, saturation 1, no warmth, a white tint and no vignette by
default, so turning it on changes nothing until a value moves; `tint` is `{r, g, b}`, `[r, g, b]` or
`"#rrggbb"`; `filmic` rolls the top of the range off (the fitted ACES curve), so an exposure of 1.5
brightens the mids without clipping the highlights, and also softens a plain white to about 0.8 of
the light (it is the `aces` operator, used when `render.tonemap` has none); `temperature` 1 is warm
(red up, blue down by 15 percent), -1 cool; `vignette` 1 takes the corners to black and leaves the
middle half of the frame alone; `lut` names a look-up table image (N slices of N by N side by side,
N*N wide and N high, red across a slice, green down it, blue from slice to slice: the strip
color-grading tools export), applied to the finished, encoded colors with the two nearest slices
blended, `lut_strength` mixing it in (a picture of any other shape is refused and reported in
`render.stats.assets.missing`; `render.stats.lut` says one applied); `capture {pixel: {x, y}}` or
`{pixels: [...]}` reads pixels of the frame; `render.stats` reports `draw_calls`, `shadow_draws`,
`instances`, `sprites`, lights and assets; `perf` reports the render time beside the other phases.

## Render scale

A dense screen (a laptop's at twice its points, a phone's at three times) or a weak GPU (a browser
on integrated graphics) can make a game pay for pixels nobody sees. `render.scale {scale}` (or
`[render] scale = 0.75`) draws the window's view at that fraction of its width and height (0.25 to
1) into a frame of its own, every pass at that size, and stretches the finished picture into the
window by bilinear filtering, sharpened against each pixel's four neighbours and held within their
range so edges do not ring (`sharpen`, 0 to 1, 0.25). `pixelated: true` stretches it through the
nearest texel instead, unsharpened, so each drawn pixel becomes a hard-edged block: a quarter scale
and `pixelated` give a 3D game the look of an old console or of pixel art, at a sixteenth of the
shading cost. The interface is painted afterwards at the window's own size, so text stays crisp.
Coordinates stay the window's: `render.project`, `render.unproject`, `render.pick` and
`capture {pixel}` take and answer window pixels, while `render.ids` and `render.visible` read the
smaller id target. `dynamic: true` (`[render.scale] dynamic = true, target_ms = 12, least = 0.5`)
moves the fraction: every thirty frames whose GPU time came back, if the median is over `target_ms`
the fraction drops to what should bring it there (the time going with the pixels, so with the square
of the fraction), and if it is under six tenths of it the fraction rises a twentieth, between
`least` and `scale`, in twentieths, so the targets are not made again every frame. It needs the
GPU's timestamps (Where the time goes, below): without them the fraction holds. `render.stats.scale`
says the fraction and size drawn when it is under 1. Cameras with viewports or targets of their own
(split screens, minimaps) are drawn whole. At 1920 by 1080 on this machine the showcase's GPU frame
took 5.8 ms whole, 3.6 at three quarters and 1.9 at a half; the hills (bound more by their 1.1
million triangles and shadows) 4.7, 3.0 and 2.4. `runtime_tests` (`[render][scale]`): the hello ball
projected and picked at the same window pixel whole and at half (an id target of 160 by 90 under a
320 by 180 window), and a dynamic target no frame can meet taking the fraction down.

## Toon

A cel look in one setting: `render.toon {bands, softness, outline, outline_color, opacity}` (or
`[render.toon]` in `project.toml`; `toon = true` for the defaults). The sun's and the lights' light
on every surface comes in `bands` flat steps (3), each step's edge blurred over `softness` (0.04 of
a band) so it does not crawl as things move; the sky's and the ambient light stay smooth, so shade
is a colour and not black. An outline `outline` pixels across (2; 0 for none) in `outline_color`
(near black, `"#rrggbb"` or `{r, g, b}`) at `opacity` is drawn wherever the entity under the pixels
changes, read from the id pass in the final pass (before colour vision, after the grade): every
entity has its silhouette against the sky, the ground and each other, and none inside itself. The
width is in the window's pixels whatever the render scale. `render.stats` says `toon`. It suits the
built-in props and the humanoid (`docs/design/assets.md`, Props). `runtime_tests`
(`[render][toon]`): a row of pixels across the hello sample's ball has no outline colour, has it at
both of the ball's edges with a magenta outline of four pixels, and has none again once it is off; a
colour that does not read is refused. `tests/evidence/rendering/toon.png`
(`tools/scripts/toon_evidence.py`) is a few props and a humanoid drawn plainly (left) and with
`render.toon` (right).

## Highlights: outlining what the player can use

What the player can pick up, talk to or open, or the enemy in the sights, wants a line round it.
`MeshRenderer.highlight` is a colour whose alpha is the line's strength (0, the default, none): the
final pass draws it on the pixels just outside the entity (three window pixels out, read from the id
pass as the toon look's outlines are), for up to 32 entities at once, with or without the toon look.
A script sets it when the player comes near and clears it when it leaves; `render.stats` says how
many `highlights` were drawn. `runtime_tests` (`[render][highlight]`): a row across the hello
sample's ball has green just outside both of its edges once the ball's highlight is green, and none
before or after. The village sample outlines the elder in gold while the player is near enough to
talk to him.

## Colour vision

About one man in twelve sees colours with one of the three kinds of cone missing or shifted.
`render.colorblind {mode}` (or `[render] colorblind = "deuteranopia"`) corrects the finished frame
for one of them, `protanopia` (red), `deuteranopia` (green) or `tritanopia` (blue): the difference
their eye loses is shifted into colours it keeps, so a red key and a green door that look alike to
them differ again (daltonization: the frame minus what they see, carried into green and blue, or red
and green for tritanopia). With `simulate: true` the frame is shown as they see it instead, for a
designer checking that what matters still reads. The sight is Machado, Oliveira and Fernandes's
model of a full dichromacy (2009), one matrix in linear light applied in the final pass after the
grade, so it costs nothing measurable; `strength` (0 to 1) mixes it with the plain frame. The
interface is drawn afterwards and keeps its own colours. `render.stats.colour_vision` names the mode
on. `runtime_tests` (`[render][colorblind]`): a red and a green panel seen as with deuteranopia both
turn toward the same yellows, corrected they change and the red stays redder than green, and off
gives the plain frame back.

## Where the time goes

`render.stats.gpu` says what the frame cost the GPU: `ms` from the first pass's start to the last's
end, and `passes`, every pass by its name (`shadow`, `ids`, `scene`, `glass`, `water`, `ao`,
`volume`, `ssr`, `taa`, `bloom.*`, `post` and the rest; the same name twice summed), the busiest
first. Every pass writes timestamps at its start and end (the WebGPU `timestamp-query` feature,
which Metal, Vulkan, D3D12 and the browsers that grant it have; without it there is no `gpu`), read
back a few frames later (`frames_ago`; `frames_timed` counts the readings). A GPU runs passes
overlapped (a tiler shades one's pixels under the next one's vertices), so each pass's milliseconds
are what it adds: how much later it finishes than every pass before it. They are the place to look
when a scene is slow, and `render.stats.triangles` and the draw counts say what it drew.

The CPU keeps at most two frames queued behind the one it builds: a native build waits for the
oldest submission when it would run further (headless, or with a hidden window, nothing else paces
it), so what it reads back from the GPU is at most that old and a headless run's frame time is the
GPU's. It waits for that one submission only: a wait for everything queued left the GPU idle while
the CPU caught up. The camera's passes (the scene, the ids, glass and translucency) draw only what
reaches into the view, each draw's bounding sphere against the view's six planes
(`render.stats.out_of_view` counts the rest); a sun cascade draws only the casters over its own
square of the light's view, and none that do not cast. In `samples/hills` the view leaves 971 of the
draws out and the cascades draw 2,242 casters where they drew four times 1,341.

## Light and color

The scene is lit in linear light in a half-float target and turned into the 8-bit frame by one final
pass, so light can go past white and still be brought back.

- **Colors.** A color on a component (`MeshRenderer.color` and `.emissive`, `Light.color`,
  `Sprite.color`, a tile map's tint, a particle's colors, the clear color, a debug line's color) is
  authored as a color picker shows it, in sRGB (`{r, g, b, a}`, an array, or `"#rrggbb"`, `#rgb`,
  `#rrggbbaa` in any component write and in the SDK's types), and decoded to linear light where it
  is used; a channel over 1 is an intensity and is kept as it is, so `emissive {r: 4, g: 4, b: 4}`
  glows four times white. glTF material factors are linear already (the glTF convention) and
  multiply in unchanged. Base color and emissive maps are sampled through an sRGB view of the
  texture, so filtering and blending happen on light, not on encoded values; the interface paints
  the same images through the stored view, in sRGB, as before.
- **Exposure.** `render.tonemap {exposure}` multiplies the scene (1 as lit). With
  `auto_exposure: true` a compute pass meters the viewport every frame (the mean of 4096 log
  luminances on a grid, clamped to `min_ev`..`max_ev`) and sets the exposure that brings that
  average to mid gray (0.18), plus `compensation` in EV; the exposure eases toward it at `speed` per
  second of frame time (the tick length), in EV, so stepping from a cave into sunlight is bright for
  a moment and then settles. The first metered frame after the meter is turned on takes the target
  at once. The answer to `render.tonemap` carries `metered {exposure_ev, average_ev}`, read back
  from the GPU, so an agent can see what the eye decided.
- **Operators.** `none` clips at white: an unlit sprite or tile comes out in exactly the colors it
  was drawn in, so 2D games keep their art (the default). `aces` is the fitted ACES filmic curve:
  contrasty, highlights rolled off, white at about 0.8 of the light. `agx` is AgX: a gentler
  shoulder that desaturates highlights toward white instead of skewing their hue, white at about 0.6
  of the light. `neutral` is Khronos PBR Neutral: colors under the shoulder are kept exactly (a
  product shot or a UI-like scene), only what nears white is compressed. The operator comes after
  the exposure and before the grade.
- **Ambient.** Where there is no `Sky`, every surface gets a flat light from all around:
  `render.ambient {color, intensity}` (`[render] ambient` in `project.toml`), a cool grey by
  default. A 2D game lit by its torches turns it down (`docs/design/sprites.md`, Light).
- **Blending.** Translucent meshes, sprites and particles blend in linear light: a half-transparent
  white over black is half the light, which the screen shows as 188 of 255, not 128.

`renderer_tests` (`[hdr]`) holds these to numbers: an emissive of 4 at an exposure of an eighth is
half the light (188, where an 8-bit scene would have clipped first and shown 99), white through
`aces`, `agx` and `neutral`, a dim slab and one sixty times brighter both metered to mid gray (118)
with the eye adapting over the frames after the change, one stop of compensation (161), and a sprite
colored (0.5, 0.25, 0.75) coming out as (128, 64, 191).

## Sky and environment light

A `Sky` component (on any entity; the first enabled one counts) puts a sky around the scene and
lights the scene with it:

- **Procedural** (`mode: 1`, the default): a gradient from `zenith` straight up to `horizon` and a
  `ground` below, colors authored in sRGB, times `intensity`; where the first directional `Light`
  points from, a glow and a disc `sun_size` degrees across in the sun light's color, bright enough
  (40 times the light) for bloom to catch. Turning the sun turns the sky's glow with it.
- **Image** (`mode: 2`, `image: "assets/sky.hdr"`): an equirectangular panorama, 2:1 with the
  horizon across the middle. A Radiance `.hdr` keeps real light levels (a sun of thousands times
  white lights the scene like one); a `.png` or `.jpg` is decoded from sRGB. `rotation` turns it
  about the vertical axis.
- **Atmosphere** (`mode: 3`): the sky the air makes of the sun's light (below).

### Atmosphere

In mode 3 the sky is computed rather than authored: the first directional light's sunlight scattered
once on its way through a planet's air to the eye (after Nishita), by the air itself (Rayleigh
scattering, which takes out and spreads the blue) and by `haze` (Mie scattering from dust and water,
white and thrown forward around the sun), both thinning with height over a planet 6360 km across.
The same sun then gives a blue day with a pale horizon, a yellow afternoon, the orange and gold of a
sunset, and a sky that darkens once the sun is below the horizon, with the afterglow along it, into
the night below; moving the sun (a script, a timeline) is all a day needs. The view's own path
through the air counts half its thickness, standing in for the light scattered more than once, which
keeps a noon horizon white rather than orange. The panorama is rebuilt from it when the sun turns
(the environment light follows), and below the horizon the `ground` colour lit by the sun fades up
into it.

The air colours the sun light too: the directional light reaches the ground through it, so its
colour is multiplied by the air's transmittance toward the sun over that straight up (a light high
in the sky is as authored, a low one redder and dimmer, one below the horizon out, fading as its
disc sinks). `render.stats.sun_light` reports what reaches the ground. Once the sun is below the
horizon the directional light is the moon instead: across the sky from the sun and lifted thirty
degrees or so (at dusk and dawn it is well up, not lying along the ground), cool, a tenth as bright,
and rising from nothing as the sun sinks its first six degrees so nothing jumps. The air takes the
moon's light as it takes the sun's, besides the sun's afterglow: the same scattering at a tenth the
light is a deep blue night sky, and the environment light and the fog follow it. Stars come out as
the sun goes from three to fourteen degrees down: a grid over the directions with a star in a few
cells in a thousand, each as bright and as warm or cool as its hash says, twinkling a little and
sinking into the haze toward the horizon; clouds cover them. A `Fog` under an atmosphere takes the
sky's colour around the horizon (the mean of eight directions a little above it) instead of its own,
so distance fades into the sky at noon, into gold at sunset and into the dark at night.

### A day

`Sky.time_of_day` (hours, 0 to 24; negative, the default, leaves the light alone) stands the first
directional light where the sun is at that hour: rising in the east (+x) at 6, at `sun_height`
degrees (60) to the south (+z) at noon, setting in the west at 18, under the ground at night. With
`day_length` (seconds of game time for a whole day; 0, the hour stands) the hour runs on by itself
and is written back into the `Sky`, so it is saved, hashed and read like any other field. Under an
atmosphere that is a whole day: a blue noon, a gold evening, a moonlit night under the stars with
the game's own lights warm in it (the village's lamps; `tests/evidence/rendering/village-day.png`:
noon, half past five, dusk, eleven at night and its stars). A point or spot light with `after_dark`
is lit only once the sun is down: it fades in as the first directional light sinks from four degrees
above the horizon to two below and is not there at all by day (no clustering, no shadow faces);
without a directional light it is always lit. The village's lamps are. `runtime_tests`
(`[sky][day]`): at noon the sunlight shines down, at 6 west along the ground, at midnight up from
below, and a day of 24 seconds moves the hour on by one in a second; `renderer_tests`
(`[atmosphere]`): with the sun six degrees down the light is cool and dim and the sky overhead a
dark blue.

`clouds` (0..1) lays a layer of clouds over it at `cloud_height` (1500 units above the camera) with
features `cloud_scale` (900) across: five octaves of value noise drawn in the sky pass, thinning
toward the horizon, lit by the sun (brightest looking toward it, reddened with it) and by the sky
from above, darker where they are thick. They drift with the `Wind` (`docs/design/wind.md`) at six
times its speed, as air up there moves faster, or at 12 units a second along +x without one, on the
simulation clock. They shade what is under them: a surface's sunlight is cut by up to seven tenths
where the way to the sun crosses a cloud, so their shadows drift over the ground with them. They are
drawn in the sky pass only, so the environment's reflections and its light do not show them.

The sky is an environment map on the GPU: a 512x256 half-float panorama whose mip levels are the
same sky convolved with GGX lobes of roughness 0 to 1 (compute passes: the first level from the
settings or the image, each next level importance-sampled from the one above with the lobe widened
by what that level lacks), and nine spherical-harmonic coefficients of its light for diffuse
lighting (a compute reduction over a small level, convolved with the cosine lobe). It is rebuilt
only when something it is made of changes (its settings, its image, and for a procedural sky the
sun's direction and color, in steps of about a fifth of a degree and of its light to three figures,
so a running day rebuilds it some eight times a second of a five-minute day rather than every
frame); `render.stats.env_updates` counts the rebuilds and `render.stats.sky` says which sky is
drawn (`none`, `procedural`, `image`, `atmosphere`).

A surface then gets from the sky, in place of the flat ambient: diffuse light from the harmonics in
its normal's direction (times `diffuse`), and specular light from the prefiltered level matching its
roughness in the mirror direction, weighted by Karis's fit of the split-sum BRDF (times `specular`),
so a polished metal mirrors the sky and the ground and a rough one takes their blurred colors. The
sky is drawn first in the scene pass, a full-screen triangle behind everything with an id of 0, so
picking sees through it. A scene with a `Sky` has no flat ambient even when `diffuse` and `specular`
are 0, so turning them down makes the shadows go black. Orthographic 2D scenes leave it out.

`renderer_tests` (`[atmosphere]`, Atmosphere above; `tests/evidence/rendering/atmosphere.png`) and
(`[sky]`) hold it to pixels: the procedural sky blue up high and darker below, the sun's disc in
front of the camera white-hot, no rebuild on a frame where nothing changed, a mirror ball with sky
in its top and ground in its bottom, a rough white ball lit bluish from above by the sky alone and
black with the sky's light at 0, and a generated `.hdr` panorama (red above at twice white, green
below at half) behind the scene and in the mirror.

### Weather

A `Weather` (one for the whole world: the first enabled by id) makes it rain or snow. `rain` and
`snow` (0 to 1) say how hard; drops and flakes fall in a box 36 by 22 by 36 units about the camera,
up to nine thousand drops and seven thousand flakes (times `density`), slanted by the `Wind`. They
are made in the vertex stage from each one's number and the simulated time, with nothing kept from
frame to frame: a drop's place is a hash in the box, slid along its fall and wrapped about the
camera, so the box goes with the camera while each drop keeps falling where it is in the world. Rain
is a thin streak along its fall, snow a round flake swaying as it drifts down; both are lit by the
sky from above and a little by the sun, fade up close and toward the box's edge, and are drawn after
the sprites without writing ids, so picking and outlines see through them.

What it falls on changes, in the same step that paints decals, so the id pass's roughness (and the
reflections traced from it) agree. `wet` (0 to 1) darkens surfaces as porous things darken and gives
them a damp sheen, most where they face up, metal not darkened; on flat ground water stands in
puddles where a broad noise is high, more of them the wetter, dark and mirror-smooth. `cover` (0 to 1)
lays snow on what faces up: where a two-octave noise is under the cover, so about that share of flat
ground, in patches while there is little and everywhere at full, white and matte; walls stay bare.
Both build up and go by themselves, written back each tick: `wet` runs toward `rain` (fully wet
after 15 seconds of a downpour, dry 90 seconds after it stops) and `cover` builds while it snows
(full after 40 seconds at `snow` 1) and melts in four minutes once it stops, faster in rain; a game
sets either to start wet or white. `overcast` (negative follows the weather: seven tenths of the
rain or snow) dims the sun's direct light by up to four fifths and, under an atmosphere, clouds the
sky over at least that much; the sky's own light is left as it was. It does not know what is under a
roof: the inside of a house open to the sky gets wet too, and rain falls through ceilings onto a
camera indoors. Pair it with the sound beds (`sfx:rain`, `sfx:wind`; `docs/design/audio.md`).

`render.stats.weather_drops` counts what is drawn. `runtime_tests` (`[weather]`): seven and a half
seconds of a downpour wet things halfway, nine dry seconds take a tenth off, ten seconds of snow lay
a quarter, ten after melt a twenty-fourth and rain takes the rest. `renderer_tests` (`[weather]`): a
wet floor is darker, snow lying whitens the floor and a block's top but not its side, the drops
drawn are as many as the rain, the snow and the density say, and the sun's light falls with the
overcast. `tests/evidence/rendering/weather.png` is the village in a downpour and under snow lying a
quarter, half and wholly.

## Ambient occlusion and fog

Both read a depth prepass: when MSAA, ambient occlusion or fog is on, the id pass runs first at one
sample with a depth target of its own (`render.stats.depth_prepass`), and what comes after samples
that depth.

- **Ambient occlusion** (`render.ao {enabled, radius, intensity, samples}`, or `[render.ao]`): a
  pass at half resolution rebuilds each point's position and normal from the depth (the normal from
  the neighbors on the nearer side, so an edge does not bend it) and looks at `samples` points in
  the hemisphere around the normal, `radius` world units out, turned per pixel; a sample the depth
  buffer has something in front of occludes, fading with how far that something is. A 5x5 blur that
  keeps to one surface smooths the result, and the lit pass multiplies the light from all around
  (the sky's diffuse and reflections, or the flat ambient) by it, never the lights', so a crate sits
  on the floor and a crevice goes dark while the sun still lights both. `render.stats.ao` says it
  ran.
- **Fog** (a `Fog` component: `color`, `density`, `height`, `falloff`, `start`, `max_opacity`):
  exponential height fog applied in the final pass, in HDR before the exposure: the density at a
  height is `density * exp(-falloff * (y - height))`, integrated along the ray from the camera to
  the point in closed form, so a valley fills with fog while the hilltops stay clear and looking up
  it thins; the sky counts as a thousand units away, so its horizon melts into the fog and its
  zenith, with a falloff, stays blue. `start` keeps the first units clear and `max_opacity` lets a
  far mountain show through. `render.stats.fog` says it applied.

`renderer_tests` hold them to pixels: `[ao]` has the floor at a crate's base under sky light alone
come out darker with occlusion on (and like open floor with it off), open floor unchanged, and the
crate still picked through the prepass's ids; `[fog]` has a black block eighty units off turn almost
entirely red in a red fog of 0.05 while one three units off stays far darker, and a falloff from a
base far below leave the far one nearly clear.

## Soft and contact shadows

A real sun is a disc, not a point, so a shadow is sharp where it meets what casts it and softer the
farther it falls. `softness` (`render.shadows {softness}`, `[render] shadow_softness`; 0 by default,
at most 5) is that disc's radius in degrees. With it, the lookup into a cascade first looks for what
stands in front of the point, over the widest penumbra the sun's size allows for casters up to eight
units away (sixteen depth reads on a spiral turned by the point), takes their mean depth, and
filters with sixteen comparisons over a disc as wide as the penumbra at that distance, one to
twenty-four texels (percentage-closer soft shadows); a point with nothing in front of it is lit
without the second step. The spiral's turn changes from point to point, which TAA smooths. At 0 the
sharp 3x3 filter stays as it was, and so does a cascade so coarse that the widest penumbra is under
a texel and a half (far from the camera, where the spiral's thirty-two reads would change nothing
seen). Each step of the spiral turns the last by the golden angle with a 2x2 rotation rather than a
sine and a cosine.

Contact shadows (`contact`, `[render] contact_shadows = true`) catch what the maps are too coarse to
see: a foot on the ground, a cup on a table, a pebble. They are marched in the ambient-occlusion
pass (which then runs, with the depth prepass, even when occlusion itself is off, and keeps its
result in the pass's second channel): from each point, twelve steps toward the sun over
`contact_length` world units (0.3 by default), and a step that the depth buffer has something in
front of, by less than the march's length, puts the point in shadow, fading out over the march's
last third. The lit pass darkens the sun's light there as the maps would, by the shadow `strength`,
and takes whichever of the two is darker. It works with the maps off, sees only what is on screen,
and needs a sun.

`renderer_tests` (`[softshadows]`) hold it to pixels: a post 4 high under a sun at 45 degrees, seen
from above; with the sharp filter the edge of its shadow is equally sharp by its foot and four units
out, and at a softness of 1.5 degrees the far edge widens by more than three 2-centimetre samples
and more than the edge by the foot; with the maps off and contact shadows on, the floor at a small
block's foot on its far side from the sun darkens by more than thirty levels while open floor stays
within six. `tests/evidence/rendering/soft-shadows.png` is `samples/showcase` without and with a
softness of 1.5 degrees and contact shadows of 0.4: the colonnade's and pavilion's long shadows
soften with distance, the posts' stay sharp at their feet. Both cost about 0.2 ms on the showcase's
frame at 960 by 540 on an Apple GPU (`render.stats.gpu`, the `ao` passes, and the lookups in the
scene pass).

## Materials and assets

`MeshRenderer.color` tints, `texture` multiplies, glTF materials bring their base color and texture
(`docs/design/assets.md`; a `KHR_texture_transform` offset and scale on the base color texture are
folded into the uv rectangle, so a tiled texture repeats as exported). Textures are sampled through
their mip chain with 8x anisotropic filtering, so a ground seen at a grazing angle stays sharp
(pixel art, `filter: "nearest"`, keeps its texels instead). A missing asset is drawn as a magenta
cube and named in `render.stats.assets.missing`. Every texture is uploaded with its mip chain (each
level a 2 by 2 average of the one above, weighted by alpha so transparent texels do not darken
edges); the linear sampler filters between levels, so a texture drawn small is smooth rather than
sparkling, and the nearest sampler (`Sprite.filter`, tile layers) stays on the full-size image so
pixel art keeps its texels.

## Translucent meshes

A mesh whose color alpha is under 1 (the `MeshRenderer` color, multiplied by the material's base
color alpha) or whose glTF material has `alphaMode: BLEND` is translucent: it draws after every
opaque mesh, far to near by its entity's depth along the camera, with the same lit shading alpha
blended over what is behind it, depth tested but not written, so translucent things never hide each
other and an opaque wall in front still hides them. It casts a shadow and writes its id like any
other mesh (`render.pick` finds it; the wall in front of it wins as it does on screen).
`render.stats.translucent` counts them; `runtime_tests` (`[translucent]`) holds a half-red pane over
the hello scene and reads the pixel.

`render.oit {enabled}` (or `[render] oit = true`) blends them without sorting instead (weighted
blended order-independent transparency, McGuire and Bavoil): the opaque meshes draw first, then
every translucent surface adds its premultiplied color into an accumulation target weighed by its
nearness and opacity, `10 / (1e-5 + (z/5)^2 + (z/200)^6)` times its alpha held between 0.01 and
3000, and multiplies a revealage target by what it lets through; a composite divides the one by its
weight and lays it over the scene by the other, and sprites and lines follow. Two panes that
interleave, or share a center so no sort can order them, blend alike whichever was drawn first; the
nearer counts for more but not wholly, as the method trades exact layering for order independence.
It is off by default. With MSAA the accumulation runs at the scene's four samples against its
multisampled depth and is resolved (averaged) before the composite, which is laid over every sample
of the scene before the sprites, the lines and the final resolve, so translucent edges are smoothed
like solid ones. `render.stats.oit` says it ran. `renderer_tests` (`[oit]`): a red pane inside a
blue box with the same center comes out (63, 189) or (85, 139) in red and blue with sorted blending
depending on which was made first, and (74, 169) either way with OIT, with MSAA as without
(`tests/evidence/rendering/oit-msaa.png`).

## Cloth, specular and brushed metal

Four more of glTF's material extensions, which Blender's exporter writes from its Principled BSDF,
and the same on `MeshRenderer` (a negative or black value keeps the asset's):

- **Specular** (`KHR_materials_specular`; Blender's Specular IOR Level and Tint;
  `MeshRenderer.specular`): how strongly a non-metal reflects, 0 none (chalk, powder, a matte print)
  to 1 the usual, and its tint, for the sun, the lights and the sky alike; a metal is unchanged.
- **Sheen** (`KHR_materials_sheen`; Blender's Sheen; `MeshRenderer.sheen`, `sheen_roughness`):
  cloth's soft glow toward its rim, velvet, felt, a peach's fuzz: the Charlie distribution with
  Neubelt's visibility, in the sheen's colour, over the surface below, which gives up what the sheen
  takes. It answers the sun and the lights; the sky's light does not give it yet.
- **Anisotropy** (`KHR_materials_anisotropy`; Blender's Anisotropic and its rotation;
  `MeshRenderer.anisotropy`, `anisotropy_rotation` in degrees): brushed metal, a highlight stretched
  along the surface's uv `u` direction turned by the rotation: anisotropic GGX with its
  height-correlated visibility for the lights, and a normal bent across the grain for the sky's
  reflection. The direction is the vertex's tangent (Materials, below), smooth across the triangles
  of a curved mesh.
- **Unlit** (`KHR_materials_unlit`, which Blender writes for a shadeless material;
  `MeshRenderer.unlit`): the colour and texture as they are, no light and no shadow on it, for
  stylised looks and markers.

`assets.describe` reports each material's `sheen`, `specular`, `anisotropy` and `unlit`.
`renderer_tests` (`[materials]`) holds them to pixels: a dark ball's highlight falls from 255 to 29
at a specular of 0; a white sheen lifts its rim from 21 to 128; unlit, a ball lit from one side is
the same on both (153 and 153, where lit it is 25 and 255); and a polished metal floor under a lamp
seen from above shows a round highlight (its reach over half its peak within a third either way),
one stretched along `u` at an anisotropy of 1 (more than 1.8 times as far one way as the other), and
the same turned a quarter when the rotation is 90. `runtime_tests` (`[blenderlevel]`) reads a
Blender velvet's sheen and a brushed metal's anisotropy and specular from the `.blend`.
`tests/evidence/rendering/materials.png` shows a plain, a matte (specular 0), a velvet, a
brushed-metal and an unlit ball under a sky and a low sun.

## Glass

A mesh with `transmission` above 0 (`MeshRenderer.transmission`, or a glTF material's
`KHR_materials_transmission`) lets light through: glass, a bottle of water, a window. What is behind
it shows through, less what its surface reflects (the Fresnel term of its index of refraction,
`ior`, 1.5 unless `KHR_materials_ior` or the MeshRenderer says otherwise), tinted by its colour, and
in place of the diffuse light it would have scattered. The light is bent: the scene is seen where
the ray refracted at the surface leaves the far side, `thickness` units on (`KHR_materials_volume`'s
thickness, scaled with the entity; the MeshRenderer's in world units), so a thick ball of glass
shows the world magnified and upside down while a thin pane (`thickness` 0) bends nothing. A
volume's `attenuationColor` and `attenuationDistance` absorb what goes through over its thickness.
Rough glass is frosted: what shows through is blurred over a disc as wide as the roughness squared
asks. The glass keeps its specular reflections, of the sun, the lights, the sky and the probes, and
casts a whole shadow.

It is drawn in a pass of its own after the solid meshes: their colour is copied
(`render.stats.glass` counts the glass), and the glass draws over it reading the copy, before the
translucent meshes (which then show over it) and the water. What the glass shows is the scene
without the other glass and without what is translucent, so glass behind glass shows as the first
pane alone, and an object in front of the glass that the bent ray happens to find shows through it
as if behind. In a reflection probe's capture glass shows nothing through. A scene with glass keeps
the depth prepass on (as water does).

A clear coat (`MeshRenderer.clearcoat` and `clearcoat_roughness`, or `KHR_materials_clearcoat`) is a
second, dielectric reflection over the surface on its geometric normal, with its own roughness (0.03
unless set): car paint, lacquered wood, a wet stone. It reflects the sun, the lights and the sky by
its own Fresnel, and what it reflects the surface below does not get, so a rough red ball under a
clear coat keeps its matte colour and gains a sharp highlight. Blender's Principled BSDF writes its
Transmission, IOR and Coat into these extensions, so a glass or a varnished part made there comes in
as it was made.

## Levels of detail

A mesh far away covers a few pixels and needs few triangles. `MeshRenderer.lods` lists levels of
detail, each a `screen` (0.25) and either a `ratio` (0.5) or a `mesh`: when the entity's bounds
cover less than `screen` of the view's height (their radius times the projection's vertical scale
over their depth), the smallest such level is drawn instead of the mesh. A level with a `ratio` is
the entity's own mesh simplified to that share of its triangles, made the first time it is needed
(meshoptimizer's simplifier weighing position, normal and uv; every part of a mesh of several
materials simplified on its own with the edges where they meet kept, so no crack opens) as a second
index buffer over the same vertices, so it costs no vertex memory and a skin or morph targets still
fit; one with a `mesh` draws that mesh (a primitive, or a file whose parts match: a Blender-made
`_LOD1`). Below `cull_screen` the mesh is not drawn at all, nor its shadow. The choice is per draw,
after a Scatter's copies are made, so every copy of a field of grass takes its own level by its own
distance; skinned meshes keep their detail. `render.stats.lod` counts the draws simplified and
culled, and `render.stats.triangles` the triangles drawn (every copy and level counted, before the
view's culling): the hills' scatters with two levels draw 1.15 million triangles where they drew
1.46 million (`tests/evidence/rendering/lod-levels.png` shows a sphere whole and at a quarter, a
twelfth and a fiftieth of its triangles). The shadow passes draw the level the camera chose. A
terrain has levels of its own, per square of its grid and chosen by their measured height error
(`docs/design/terrain.md`, Levels of detail).

## Cut-outs

A textured mesh with a `cutoff` (`MeshRenderer.cutoff`, or a glTF material with `alphaMode: MASK`
and its `alphaCutoff`) is cut out where its picture's alpha is under the cutoff: those texels are
neither drawn nor picked, so leaves, fences and grates come from one quad and a picture with
transparent parts, and `render.pick` through a hole finds what is behind. The cut is per texel and
unblended (the edge is as sharp as the picture); a soft edge is a translucent mesh's job. Cut-out
parts still cast their mesh's whole shadow. Their shadows keep the holes: the shadow passes (the
sun's cascades and the light faces) draw a cut-out through a pipeline of their own that samples the
texture's alpha with the material bound after the cascade and drops what the cutoff cuts away, so
leaves and grates throw dappled shadows (`renderer_tests` `[cutout]`: under a roof whose texture is
solid on one half and clear on the other, the floor under the clear half is lit and under the solid
half shadowed; with the cutoff at 0 both are shadowed); a skinned cut-out still casts whole. In the
camera's passes too the cut is a pipeline of its own (plain and skinned, color and ids), drawn after
the solid meshes: a shader that may discard costs every mesh drawn with it the GPU's early depth
test (and on Apple's GPUs the hidden-surface removal), so the solid meshes are drawn by pipelines
without one. At 1920x1080 that took the scene pass from about 1.43 to 1.34 ms and the id pass from
0.26 to 0.23 ms in the showcase, and in the hills 2.1 to 2.0 and 0.55 to 0.48
(`tools/scripts/dev/gpu_probe.py`, Apple M5).

## A frame for a model that reads

`capture {ascii}` (true for 64 characters across, or a width) adds `look`: the frame as rows of
characters from dark to light (`" .:-=+*#%@"`, half as many rows as the frame's shape would give
columns, as a terminal's cells are twice as tall as wide), the same rows as letters of each cell's
commonest colour (`ascii_key` names them: `B` blue, `E` green, `N` brown, `G` gold ...), and the
frame's colours by name with their shares. A text-only model reads where the sky, the ground, a
platform and the coins are; the pi extension's `pocket_look` and the MCP `capture` tool include it
beside the picture. The sprites sample at 64 across shows its sky, its green ground and brown soil,
a floating platform and gold coins. `runtime_tests` (`[capture][look]`).

## Comparing frames

`render.compare {path, tolerance?, threshold?, diff?, update?}` holds the last frame against a
reference PNG in the project: a pixel differs when a channel is off by more than `threshold` (16 of
255), and the frame matches when at most `tolerance` (0.01) of the pixels differ. The answer carries
`match`, `differing`, `fraction`, the `bounds` rectangle of the differing pixels, and `diff` when a
PNG of the differences was asked for (the differing pixels in red over the dimmed reference), so an
agent can say where a frame changed and look. A missing reference is written from the frame and
reported as `written` (the first run records, later runs compare; delete the file to record again),
and `update: true` rewrites it. Scenarios use it as a step (`g.match(path, options)`,
`docs/design/scenarios.md`), so a level's look is checked like its score is, and
`render.compare(path, options)` is the same call from a script. Frames are deterministic on one
machine (the same GPU draws the same pixels for the same world), so a tolerance is for other GPUs
and drivers, not for the engine.

## Screen-space reflections

`render.ssr {enabled, max_distance, max_roughness, steps, thickness, intensity}` (or
`[render] ssr = true`, `[render.ssr]`) lets glossy surfaces reflect what is on screen. The id pass
(reflections turn the depth prepass on) also writes each pixel's surface: its normal (octahedral,
the normal map's bend included), roughness and metallic in a half-float target and its albedo in an
8-bit one. A pass after the scene pass takes every pixel no rougher than `max_roughness`, turns the
view ray about its normal, and marches the mirror ray in screen space from the point out to
`max_distance` world units: `steps` samples, the depth interpolated as 1/w, each pixel starting at
its own offset; a hit is where the ray passes behind the depth buffer by less than `thickness` (more
at a distance), found again by bisection. The scene's color there, spread a little on rougher
surfaces, takes the place of what the sky's reflection gave the pixel: the difference goes through
the same split-sum BRDF the lit pass used (so a dielectric floor reflects faintly head-on and
strongly at a grazing angle, a metal in its own color), faded toward the view's edges, the ray's end
and `max_roughness`. What the ray does not find (off screen, behind something) keeps the sky's
reflection. With TAA on, which blends the frames after this pass, each frame marches half the
`steps` (at least 8) from an offset that moves by the golden ratio, so the frames sample between
each other's steps: in the showcase at 1920x1080 the pass costs about what 24 steps cost (1.0 ms
against 1.55 for 48), and after 60 frames the picture differs from one marching 64 a frame by a mean
of 0.04 levels in eight bits, more than four levels at 0.12 percent of the pixels, on the edges of
reflected objects. `render.stats.ssr` says it ran.

`renderer_tests` (`[ssr]`): on a mirror-polished metal floor the pixel where a red glowing box's
front appears mirrored goes from the dim ambient (46, 48, 56) to red (245, 21, 22) with reflections
on, and back toward the ambient once the floor is made rough. `tests/evidence/rendering/ssr.png` is
four boxes and a glowing orb on a glossy dielectric floor under the procedural sky, without and
with.

## Screen-space global illumination

`render.ssgi {enabled, distance, rays, steps, thickness, intensity}` (or `[render] ssgi = true`,
`[render.ssgi]`) lets light bounce once off what is on screen onto what is near it: a red wall
reddens the floor beside it and the ceiling above it, a sunlit floor lights the underside of a
table, without placing a probe or a volume. It turns the depth prepass on. A pass at half size,
after the scene pass, takes every pixel's point and normal from the prepass and the surface target
and sends `rays` rays (2) over the hemisphere about the normal, cosine-weighted, each pixel and
frame its own (a hash of the pixel, the frame and the ray). Each is marched in screen space as a
reflection is, `steps` samples (12) out to `distance` world units (3), denser near the point; a hit
is where it passes behind the depth by less than `thickness` (0.5) and meets a surface facing it,
and the scene's lit color there is light arriving. What a pixel gathers is blended with what it
gathered last frame (a tenth each frame) where its point was seen then at the same depth, so the
rays of many frames add up and a newly uncovered pixel starts afresh (a frame with nothing to blend
with sends four times the rays). A full-size pass then adds albedo times the light gathered (the
half-size pixels around, by nearness and by how near their depth is) to the frame, times
`intensity`, none on metals, and none on unlit sprites, which the id pass marks. The reflections,
TAA and the post effects come after it.

It adds to the frame's own ambient light rather than replacing it, so a bright sky over an open
scene shows little of it and a room lit by a lamp a lot. Light from what is off screen or hidden
behind something nearer does not arrive (a face turned to a wall behind the camera stays dark), the
bounce is one, and a thin wall marched past at an angle can let some light through. At 1920x1080 in
the showcase it costs about 1.1 ms (0.65 the gathering, 0.47 the adding,
`tools/scripts/dev/gpu_probe.py`, Apple M5). `render.stats.ssgi` says it ran.

`renderer_tests` (`[ssgi]`): beside a wall glowing red, the floor's red-minus-green rises by more
than 20 levels with it on and its green barely moves, while the floor 4.5 units off keeps its hue;
off again, the bounce goes. `tests/evidence/rendering/ssgi.png` (`tools/scripts/ssgi_evidence.py`)
is a box with a red and a green wall lit by one lamp, no ambient light and no probes, without and
with.

## Reflection probes

A `ReflectionProbe` (`size`, `intensity`, `box_projection`, `realtime`, `enabled`) makes surfaces
within its box lit by what the probe sees instead of by the sky: glossy ones reflect it, so a room's
polished floor reflects the room, and every one takes its diffuse light from it, so a closed room is
lit by its lamps and walls rather than by the sky above its roof. When a probe appears, moves or
changes size (or `realtime` asks it to every frame, or `render.probes {refresh: true}`), a frame
draws the scene six ways from its position (90-degree views along the axes, 128 texels square) with
the frame's own shading: the sky behind, the sun shadowed by the camera's cascades (the finest whose
map holds the point; beyond them unshadowed), the point and spot lights whose range reaches the box
(all of them for every pixel, without their shadows, whose maps are drawn later in the frame), what
glows, and the probes' diffuse light. Its own diffuse light in a capture is its previous capture's,
none the first time, and it is captured three times in a row (three frames), so light bounces three
times off the walls: the first capture sees only what the lights reach directly, each next one what
that lit. A refresh starts from the light it had, so a lamp switched off fades over the three
captures instead of flashing dark. Ambient occlusion is left out. A compute pass turns the six views
into a 256 by 128 panorama, each direction looking up the view it falls in through that view's own
projection (so no cube map convention is involved), and the sky's GGX prefilter makes four more
levels from it, all in a layer of an eight-layer array; at most one probe is captured a frame,
realtime ones taking turns. The lit pass takes the first probe (by entity id) whose box, grown by
half a unit, holds the point: the mirror direction is box-projected (where the reflected ray leaves
the box, seen from the probe's center, so a reflection sits where the wall is rather than at
infinity) and the probe's level for the roughness takes the place of the sky's reflection, fading
out over that half unit outside the box. Screen-space reflections, where they find nothing, fall
back to the probe too. The diffuse light comes from the same probe: after the prefilter a compute
pass projects the 32 by 16 level onto nine spherical harmonics (the sky's irradiance pass, into the
probe's slot of a small storage buffer), and the lit pass evaluates them at the normal in place of
the sky's (or of the flat `ambient`), with the same fade. `render.probes` lists the probes in use
(entity, layer, center, size, captured, the captures still to make as `bounces`, the frame of the
last capture); `render.stats.probes` reports how many are `in_use` and whether one was `captured`
this frame. A probe is used from the frame after its first capture.

`renderer_tests` (`[probes]`): in a closed room under a bright sky, the mirror floor in front of a
red glowing wall reflects the sky's blue (166, 192, 239) without a probe and the wall (231, 51, 65)
with one; it is captured three times and then left alone, again on `refresh` and when moved, and
disabled the sky's reflection returns. In a second closed room (`[probediffuse]`, black shadows) the
top of a white block lit by the sky is (191, 215, 255); with the room's probe it is (101, 53, 18),
lit only by a warm lamp in a far corner whose range stops short of the block, through the walls the
probe saw it light; with the lamp off and the probe refreshed, (30, 12, 2) and falling.
`tests/evidence/rendering/probes.png` is such a room without and with its probe.

## Irradiance volumes

A reflection probe gives everything in its box the same diffuse light, whatever part of the box it
stands in. An `IrradianceVolume` (`size`, `probes`, `intensity`, `enabled`) fills a box with probes
in a grid instead, `probes` along x, y and z (each held to 2..16, the outer ones on the box's walls,
at most 1024 across all volumes), and every surface in the box takes its diffuse light from the
eight around it: each weighed by how near it lies along each axis, and by whether it is in front of
the surface, the weight falling to a fifth for one straight behind (a probe on the far side of a
wall sees the other room). So light that bounces off a red wall reddens the floor beside it and not
the far side of the room, and the green wall across tints its own corner. Its light takes the place
of the sky's diffuse light and a reflection probe's (and, with neither, of the flat `ambient`'s),
fading over half a unit outside its box; reflections stay the probe's and the sky's. Water's
scattered light takes it too.

A wall the box crosses would otherwise let light through: a probe in the lit room is among the eight
around a point just behind the wall, and is weighed only by nearness and facing. So each probe also
keeps how far it saw in every direction, and with `visibility` (on by default) a probe that saw a
surface short of a point counts for little there: per texel of a sixteen by sixteen octahedral map
around it, the mean distance to what it saw and the mean of its square (nine directions a texel,
read from its views' depth, stopping at twice the widest gap between probes; 2 KB a probe), and a
point farther than the mean along that direction is weighed by Chebyshev's bound on its being in
sight, cubed (as DDGI weighs its probes). The point is looked at from a quarter of the narrowest gap
off its surface, so the floor under a probe does not hide it. An eight by eight map left a thin line
of light where the floor meets the wall, its texels there holding both the wall's distance and the
floor's; at sixteen by sixteen it is gone from the evidence below.

Each probe is captured as a reflection probe is, six views of the scene from its place with the
frame's own shading (the sky, the sun with the camera's cascades, the point and spot lights whose
reach touches the volume's box, without their shadows, what glows, and the volume's own light as its
probes so far give it), and kept only as nine spherical harmonics, projected straight from the views
(64 by 32 directions) by a compute pass into a slot after the reflection probes'. Two probes are
captured a frame; a volume is used once every probe has been captured, and its probes are captured
three times over in all, so light bounces three times. A volume starts over when it appears, moves,
changes size or probe counts, or on `render.probes {refresh: true}` (from the light it has, as a
probe does). Up to four volumes at once, the first by id where boxes overlap. `render.probes` lists
them (`volumes`: entity, center, size, probes, `ready`, the `passes` still to make, the `next`
probe); `render.stats.probes` reports `volumes` ready and `volume_captures` this frame.

`renderer_tests` (`[irradiance]`, `[visibility]`): in two closed rooms side by side under one
volume, the only light a panel glowing in the left room's ceiling, the right room's floor by the
wall reads 0 with visibility and 111 (the sum of its channels) without, and its far side 0 either
way; in a closed room lit only by a red glowing panel along its left wall and a white one in its
ceiling, under a sky that cannot get in, one reflection probe lights the floor alike by the panel
and across the room (187, 175, 175 and 186, 175, 175); a volume of six by two by six probes reddens
the floor by the panel (189, 132, 132) against the far side (149, 136, 136); it reports two probes
captured a frame, ready after the first pass, done after three; switched off, the probe's light
returns. `tests/evidence/rendering/irradiance.png` (`tools/scripts/irradiance_evidence.py`) is a box
with a red and a green wall lit by a lamp, with one reflection probe (left) and with a volume of
eight by six by eight probes (right); `tests/evidence/rendering/irradiance-walls.png`
(`tools/scripts/probe_visibility_evidence.py`) is the right room of the two, lit by a dim warm lamp
of its own, looking at the wall the white light of the left room comes through without visibility
(left) and does not with it (right).

## Decals

A `Decal` lays an image onto whatever lies in a box: a puddle, a stain, a painted arrow, a glowing
sigil on a floor. The box is `size` across (x, y, z; 2, 1, 2) centred on the entity and turned and
scaled with it; the image spans the box's x and z (its top row toward -z) and is projected along the
entity's -y, so an unturned decal paints the floor under it and a decal turned on its side paints a
wall. `texture` names a project image (its alpha is where it paints); empty is a soft round spot,
which with a dark `color` and a low `roughness` is a puddle. `color` multiplies the image, its alpha
the decal's opacity; `roughness` (when not negative) replaces what the decal covers (0.05 makes the
floor under a puddle a mirror for reflections); `emissive` makes the image glow on its own; `angle`
(60 degrees) is how far a surface may turn from facing the projection before the decal fades from
it, so the sides of a crate standing in a floor decal stay clean; `order` sets which of two
overlapping decals paints over the other (higher over lower, then the later entity).

Every frame the enabled decals whose boxes reach into the view are gathered, the 64 nearest kept and
sorted by order into a storage buffer (the box's inverse matrix, the projection's direction, the
tint, the image's layer, the roughness and glow, a bounding sphere). Their images are layers of one
array (256 texels square with mips, fifteen images besides the spot; more are reported missing),
resampled on first use. The lit shader, before it lights a surface, walks the list: for each decal
whose sphere and then box hold the point it samples the image at the point's place in the box, with
gradients from the surface's own screen-space derivatives (so the image is filtered like any
texture), fades it with the surface's turn and toward the box's two ends, and mixes the colour in
(metals become paint, since what is painted is not metal), the roughness, and the glow. The id pass
does the same for the surface target, so screen-space reflections see the puddle's roughness. Decals
paint every mesh in their boxes, moving ones included; sprites, particles and water are not painted.
`render.stats.decals` reports how many were `drawn` and how many `images` are loaded.

A `normal_map` (a project image of tangent-space normals, +y up the image as glTF has them) bends
the light on what the decal covers, where its image paints: its x along the box's x and its y along
the box's -z, laid flat onto the surface and mixed in by the decal's coverage, times `bumpiness`
(1). A puddle's ripples, cracks in a floor, a carved rune catch the light and cast their shading
without any geometry. The map is kept in the same image array as the colours, stored so the array's
sRGB decoding hands its values back as they are.

`samples/showcase` has three puddles on its courtyard floor, rippled by `assets/ripples.png` (rings
from three drops, `make_sample_assets.py --decals`), a yellow arrow painted toward the pavilion and
a glowing compass rose on the pavilion's floor (`assets/sigil.png` and `assets/arrow.png`, from
`tools/scripts/make_sample_assets.py --decals`). `renderer_tests` (`[decalnormals]`) light three
decals of the floor's own grey under a low sun from the east: one whose normal map tilts toward the
sun (510, summed channels), one flat (366), one tilted away (59);
`tests/evidence/rendering/decal-normals.png` shows a showcase puddle flat and rippled. (`[decals]`)
paint a floor red under a spot decal and leave it grey outside the box, paint the top of a block
standing in the box and not its side, lay a blue decal over the red by a higher order and under it
by a lower, make a decal glow with the sun out, show a checker image's two colours, and drop a decal
that is out of view. `tests/evidence/decals` holds the showcase's pavilion and a puddle.

## Temporal anti-aliasing

`render.taa {enabled, feedback}` (or `[render] taa = true`, `[render.taa]` with `feedback`) resolves
every frame against the frames before it. The projection is shifted by a different point inside the
pixel each frame, eight Halton (2, 3) points in turn, so over eight frames each pixel is sampled
across its area. The id pass (TAA turns the depth prepass on) also writes each pixel's motion since
the last frame into a two-channel half-float target: every object record carries its model matrix of
the frame before (found by its entity and the order of its parts; new objects and sprites move only
with the camera), and the vertex stage places the point now and then with the unjittered
view-projections of both frames. The resolve pass, after the scene pass, takes this frame's color
and looks up the history where the pixel was, following the motion of the nearest surface in its 3x3
neighbourhood (or, where nothing was drawn, the camera's own motion); it clips that history to the
range of the neighbourhood's colors in YCoCg, so what moved away or was uncovered does not linger as
a ghost, and blends it with the new color (`feedback` 0.9 keeps nine tenths), colors weighed down by
their luminance meanwhile so a single bright pixel does not flicker through the average. The result
becomes the HDR target the fog, bloom and the final pass read, and the next frame's history. A
skinned mesh's joints are followed only as far as its entity moves. `render.stats.taa` says it ran;
it is off by default, so a single frame (and every test that captures one) is what the frame alone
draws.

`renderer_tests` (`[taa]`): a glowing card turned 20 degrees over a dark floor has no pixel between
the two at one sample and 98 after sixteen frames with TAA, the floor and the card's middle keep
their exact values, and a block that slid across the floor for twelve frames leaves the floor behind
it as it was while it stays white itself. `tests/evidence/rendering/taa.png` is the posts of
`light-shadows.png` three times enlarged without (left) and with TAA (right).

## Depth of field and motion blur

Two passes after the TAA resolve, each from the HDR target into a scratch target copied back, over
the depth prepass and the motion target (either turns them on).

- **Depth of field** (`render.dof {enabled, focus, aperture, max_blur}`, or `[render.dof]`): a thin
  lens focused at `focus` world units. A point's circle of confusion is `aperture` (the blur of
  something at infinity, a fraction of the view's height: 0.01 is about 11 pixels at 1080) times how
  far it is from the focus relative to its own distance, capped at `max_blur`. Each pixel gathers 24
  texels of a golden-angle disc that wide; a texel nearer than the pixel counts only as far as its
  own circle reaches, so a sharp foreground does not bleed into the blurred background behind it,
  and the texel whose depth decides the weight is the one whose color is taken.
- **Motion blur** (`render.motion_blur {enabled, strength, samples}`, or `[render.motion_blur]`):
  each pixel is smeared along the largest motion around it (a 5x5 of samples six pixels apart, so a
  moving thing smears past its own edge), over `strength` of the frame's motion (0.5 is a shutter
  open half the frame), the sky's motion taken from the camera. A pixel moving with that motion is
  averaged along it, what lies behind showing through; a pixel that stays takes only the samples
  that move and are nearer than it, keeping its own color for the rest, so a post beside a thrown
  ball is not smeared itself and a ball behind the post does not smear over it.

`render.stats` reports `dof` and `motion_blur`. `renderer_tests` (`[dof]`): a glowing card focused
has no pixel between it and the floor, and 559 once the focus is a unit from the camera; a block
sliding across the floor is crisp without motion blur and trails 624 in-between pixels with it, and
none once it stops. `tests/evidence/rendering/dof.png` focuses the scene of `light-shadows.png` on
its ring of posts (the floor near the camera and the far wall blur); `motion-blur.png` throws the
blue ball across it, smeared behind the post it passes.

## Volumetric light

A `Fog` with `volumetric: true` is lit instead of flat. A pass at half resolution marches each
pixel's view ray through the fog, `steps` samples (16 by default) out to the depth the prepass left
or, for the sky, `distance` units; the first sample of each pixel is offset by interleaved gradient
noise, so steps show as fine grain rather than bands, and the offset moves every frame by the golden
ratio. Each frame's result is blended with the last one's (a quarter of the new, at the point's
place last frame, found through last frame's view and projection), so the frames sample between each
other's steps and 16 a frame converge to what 40 gave alone: in the showcase at 1920x1080, 16 steps
a frame differ from 40 by at most two levels in eight bits after the blend, for 1.9 ms of the pass
against 4.3 (measured beside another workload; the ratio is what holds). A frame with nothing before
it (the first, after a cut, the one an agent's `capture` draws after an undrawn `step`) marches four
times the steps, up to 128, so a single picture looks like the blend. At every sample the fog's
density (its `density` at `height`, thinning with `falloff`) takes some of what lies behind and adds
the light arriving there: the flat ambient or the sky's, the sun's through the cascaded shadow maps,
so something that blocks the sun cuts a dark shaft through the lit air, and the point and spot
lights of the sample's cluster through their shadow faces, each scattered toward the eye by the
Henyey-Greenstein phase with `anisotropy` (0 scatters evenly; toward 0.9 the air glows around a
light you look toward), scaled so that evenly scattering fog in a light of one is as bright as a
white surface in it. The fog's `color` tints what it scatters, `start` keeps the first units clear
and `max_opacity` caps what it hides. The final pass lays the result over the scene in HDR before
the exposure (what shows times the scene, plus the light gathered), upsampling it by the four
half-size texels around each pixel weighed by distance and by how close the depth each was marched
to is to the pixel's, so a thin post's edge stays sharp against glowing fog.
`render.stats.volumetric` says it ran. The cost is the samples times the lights of each sample's
cluster, over a quarter of the pixels.

`renderer_tests` (`[volumetric]`) holds it to pixels: a spot shining down through fog shows as a
beam against the air beside it, a slab over half the beam darkens that half below it while the same
side above stays lit, and with `volumetric: false` the beam is gone.

## Many lights

A `Light` of kind 1 is a point light and kind 2 a spot: a cone along the entity's -Z, full inside
`inner_angle` degrees of its axis and gone at `outer_angle` (smoothly between); both fade to nothing
at `range`. There is no fixed count. Each frame the renderer gathers them into a storage buffer and
assigns them to clusters (clustered forward shading): the view is cut into 16 tiles across, 9 down
and 24 slices of depth, the slices spaced by the logarithm of the depth from the camera's near plane
to its far one so near slices are thin. A light's sphere of reach is projected to a rectangle of
tiles (the whole screen when part of it is behind the eye) and a run of slices, and of the clusters
there, those whose box in view space the sphere touches list it; a light wholly outside the view is
left out. The lit pass finds its pixel's cluster from the pixel's place in the viewport and its view
depth, and loops over that cluster's lights alone, so a hall of a hundred lamps costs each pixel the
few that reach it. Lights are taken nearest first; past 1024 in view or 262144 cluster entries the
farthest are dropped, and `render.stats` says so. The assignment runs on the CPU in the order the
world holds the lights, so the same world gives the same frame.

**Their shadows.** A point or spot light with `shadows: true` casts them from a depth atlas of 4096
by 4096 texels cut into 64 faces of 512: a spot takes one face, a perspective view down its cone a
little wider than the cone, and a point light six, the faces of a cube around it, each a little
wider than 90 degrees so the filter at a face's edge still reads that face. Faces go to the shadowed
lights in the order the clusters kept them, nearest first, until the 64 are used; the atlas is made
the first time a light asks. One pass draws every face into its own square, each face only the
casters whose bounding sphere meets the light's reach (so a lamp among forty blocks draws the two
beside it, twelve instances over its six faces, not 240; `render.stats.shadow_instances` counts what
the shadow passes drew), and the lit pass, for a light with faces, picks the face (a point light's
by the axis the direction from the light runs along), moves the point off the surface and toward the
light by a texel's size there, and filters 3x3 inside the face. `render.shadows {enabled: false}`
turns these off with the sun's. A mesh with `cast_shadows: false` casts none, for the sun or the
lights: a lamp's bulb around its own light, glass. `render.stats.light_shadows` reports the shadowed
`lights` and the `faces` they drew.

`render.stats` reports `point_lights` and `spot_lights` in view, and `lights` with the grid
(`clusters: [16, 9, 24]`), how many were `culled` and `dropped`, the cluster `entries` and the most
one cluster lists (`max_per_cluster`). glTF files' spot lights (KHR_lights_punctual), and so
Blender's, come in as spots with their cone angles (`world.instantiate {mesh}`).

`renderer_tests` (`[lights]`) holds it to pixels: a spot 3 units over a white floor lights the floor
under it as a point light at the same height would and leaves it dark 2 units aside, where the point
light still lights it, and turned to shine sideways leaves the floor under it dark; four hundred
colored lights 2 units apart are culled to the view (the rest counted), the floor under a red, a
green and a blue one comes out in that color and so does one near the view's corner; and with a
thousand more in view the 1024 nearest are kept, the rest counted as dropped. `[lightshadows]` puts
a block between a light and the floor: with `shadows` on, the floor behind it darkens by more than
60 levels while open floor moves by less than 6, for a spot (one face) and a point light (six), the
block with `cast_shadows: false` lets the light through, and `render.shadows {enabled: false}` draws
no faces. The tighter per-cluster test changes no pixel: a hall of 75 lamps and three spots renders
identically with the rectangle-only assignment, from about a fifth of the entries.

## Materials a project writes

A mesh can be shaded by a project's own WGSL over the engine's light: `MeshRenderer.material` names
a file in the project that defines `fn material(lit: vec4f, s: Surface) -> vec4f`, the final colour
(linear, before the tonemap) from `lit`, the engine's lit colour with every light, shadow and
reflection, and `s`: the surface's colour (`base`, texture times colour), its `normal`, `position`,
`view` (toward the eye), `uv`, the sun's direction (`sun_dir`) and light (`sun_color`), the entity's
four numbers (`params`, from `MeshRenderer.material_params`) and the simulation's seconds (`time`).
Toon steps over the sun's light, a rim where the surface turns from the eye, a hologram's moving
lines, a hit's flash: the engine's light is there to keep or to replace. Opaque, unskinned meshes in
the lit pass take it (a translucent, glass, cut-out or skinned one is shaded as usual), and the
depth, id, shadow and reflection passes see the mesh as it is, so the material changes colour, not
shape. Each material is a variant of the opaque lit pipeline, made for the scene's sample count, and
the draws are sorted so a material's meshes are drawn together. The file is read the first time a
mesh names it and again after `assets.reload`, compiled inside a GPU error scope: one that does not
compile leaves its meshes shaded as usual, and `world.lint` names them with the compiler's message,
its line numbers counted in the material's own file (`materials/toon.wgsl:3:12`), as for sprite
materials and post effects.

```wgsl
// samples/hello/materials/toon.wgsl: three flat steps of the sun's light, a rim, and params.x
// laying a flat colour (params.yzw) over it.
fn material(lit: vec4f, s: Surface) -> vec4f {
    let ndl = max(dot(s.normal, s.sun_dir), 0.0);
    let steps = select(select(0.35, 0.7, ndl > 0.2), 1.0, ndl > 0.6);
    let rim = pow(1.0 - max(dot(s.normal, s.view), 0.0), 3.0) * 0.4;
    let c = s.base.rgb * s.sun_color * steps + s.base.rgb * 0.15 + vec3f(rim);
    return vec4f(mix(c, s.params.yzw, clamp(s.params.x, 0.0, 1.0)), 1.0);
}
```

`runtime_tests` (`[material]`): a block drawn through the toon material comes out green and then red
as its params lay those colours over it, grey and stepped with none, and a material that is not
there is named by the lint.

## Post effects

A project's own look after the tonemap: an old screen's lines, a pixelated world, a hit's red flash,
a grey for a flashback, a vignette that closes in. An effect is WGSL that defines one function,
`fn effect(uv: vec2f) -> vec4f`, the colour at a point of the view (0..1 across and down), with four
to call: `sample_frame(uv)` (the frame so far, colours as they will be shown, 0..1), `param(i)`
(eight numbers the project gives), `resolution()` (the view's size in pixels) and `time()` (seconds
since the effects were set).
`[render] post = ["effects/crt.wgsl", {shader = "effects/flash.wgsl", params = [0.5]}]` in
`project.toml` sets them when the game starts and on `project.reload`; `render.post {effects}`
(`render.post(effects)` in scripts) sets them now, from files in the project or code given whole
(`{code, name}`), and an empty list takes them away. They run in order, each reading what the one
before drew, through two targets of the frame's size, the last into the frame; the interface is
drawn after them, so the HUD stays sharp. Each compiles on its own inside a GPU error scope: the
answer gives per effect `ok` or the compiler's message, and only those that compiled run, so a
mistake costs the effect, not the frame. `render.stats.post_effects` counts what ran.

```wgsl
// Grey, turned over by param(0).
fn effect(uv: vec2f) -> vec4f {
    let c = sample_frame(uv);
    let g = dot(c.rgb, vec3f(0.299, 0.587, 0.114));
    return vec4f(mix(vec3f(g), vec3f(1.0 - g), param(0)), 1.0);
}
```

`samples/crates` draws through `effects/scanlines.wgsl`. `runtime_tests` (`[post]`): a block's
colour turned grey to its luminance, turned over by a parameter, two effects in order, a broken one
answered with the compiler's words while the other runs, and none giving the frame back as it was.

## Not yet

Heights on decals (a decal's normal map bends the light, but nothing it covers is displaced or
occludes itself), glass behind glass and light through glass (a glass shows the scene without the
other glass, and casts a whole shadow; caustics come only from water), dispersion and textures for
transmission and the clear coat (their factors only), shadows of point and spot lights in a
reflection probe's capture (and an irradiance volume's), holes in a skinned cut-out's shadow (a
skinned mesh casts whole), and output to HDR displays (the frame is encoded for an sRGB screen).
Each is a renderer-internal change: the commands and components stay.

## Debug lines

`render.debug {colliders, joints, bounds, axes, nav, lights}` draws the engine's own overlays as
lines over the scene: colliders in green (dynamic; dim green asleep), gray (static), blue
(kinematic) or yellow (triggers), with boxes, spheres and capsules in their exact physics shape, and
2D ones (`Collider2D`) as outlines in their plane in the same colours (a circle with a spoke that
shows its turn); joints as a line between the two anchors (orange for distance joints, pink for ball
joints); the engine's world-space `Bounds`; the world axes; lights in their own colors (the sun as
an arrow along its direction from where its entity stands, a point light as the sphere its `range`
reaches, a spot as its cone out to its range, the outer edge full and the inner one faint, so where
a light reaches can be read without rendering it). Scripts and agents add their own with
`debug.line {a, b, color, ticks}`, `debug.box {center, half, rotation, color, ticks}` and
`debug.sphere {center, radius, color, ticks}`, which stay for `ticks` ticks (one by default);
`debug.clear` drops them and `debug.stats` counts them. The lines go through a small pipeline of
their own after sprites: depth tested against the scene (a collider inside a wall is hidden by it)
but pulled a hair toward the camera so lines on a surface win, alpha blended, and without writes to
the id buffer, so `render.pick` and `render.visible` still see the entity under a line.
`render.stats.debug_lines` counts what was drawn. The editor's Overlays button turns colliders,
joints and lights on in the scene pane. `tests/evidence/rendering/overlays.png` is the physics
sample at tick 230 with colliders and joints on; `light-overlays.png` is the scene of
`light-shadows.png` with `lights` on.

## Materials

Every mesh draw carries a metallic, a roughness, an emissive color and up to four maps: base color,
metallic-roughness (glTF layout: roughness in green, metallic in blue, multiplied into the factors),
a tangent-space normal map (+Y up, the glTF convention) and an emissive map. glTF materials bring
all of them (`assets.describe` lists the paths and factors); `MeshRenderer.metallic`, `.roughness`,
`.emissive` and `.normal_map` override or add to them per entity, so a primitive sphere becomes a
polished metal with two numbers and a plane gets a bump map with one path. Normal maps and brushed
metal use each vertex's tangent: glTF's `TANGENT` when the file has one (the Blender import exports
Blender's own, the ones its normal maps were baked against; `assets.describe` says
`tangents: "file"`), else one made from the uvs when the mesh loads (`"uv"`; the primitives and
terrains get theirs the same way): the direction the uv's u runs across the triangles around the
vertex, square to its normal, the bitangent toward the texture's up. It travels as four signed bytes
beside the vertex colour and is turned into the world with the model (and the skin). A vertex with
none, for want of distinct uvs, falls back on a frame built from the screen-space derivatives of
position and texture coordinates. Until 2026-09-30 that fallback was all there was, and in WebGPU,
whose `dpdy` runs down the screen, it turned the map's x and y over: a map's bumps were lit from the
wrong side. Both frames now follow glTF (+Y up the image). The maps of one draw form one bind group,
cached per set of paths; `render.stats.materials` counts them. `samples/assets` shows a plate with
all three maps and a gold orb; `tests/evidence/rendering/pbr.png` is that scene, and
`renderer_tests` (`[pbr]`) pins the normal map's sign (a map bent toward the image's top, on a floor
whose image's top is toward a light low in the far side, comes out brighter), a metal's highlight
and an emissive surface with pixel probes; `core_tests` (`[tangents]`) the made tangents' direction
and side, and `assets_tests` (`[pbr]`) that a loaded mesh has one per vertex, of unit length and
square to its normal.

A terrain with textured layers (`docs/design/terrain.md`, Layers) draws with two more resources in
its material group: a small splat texture of each layer's share at the grid's samples, made with its
mesh, and a 2D array of its layers' images, each resampled to 512 square (still tiling) and tinted
in linear light, with mips. The scene pass (and the id pass that feeds reflections) mixes the four
images, each at its own scale, by the shares at the pixel, with explicit gradients so the branch
costs other meshes nothing; the painted colour rides in the vertex colour's alpha and is laid over
them. Other materials bind 1 by 1 stand-ins.
