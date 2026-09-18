# Research note: agents understanding gameplay without frames

Desk research by a Claude research agent on 2026-09-18 (all URLs opened that day).

## 1. LLM agents playing games

- **Voyager** (May 2023, https://arxiv.org/html/2305.16291): GPT-4, text only; observations are Mineflayer-derived text: biome, time, nearby blocks within 32 blocks, nearby entities by distance, chests, health and hunger, position, equipment, inventory. Actions are generated JavaScript. The authors state it does not support visual perception. A semantic world query was enough for open-ended play.
- **Claude Plays Pokémon** (Anthropic, February 2025): screen pixels plus a knowledge base and button tools; three badges. A stream analysis describes a RAM-derived overlay (coordinates, badges, inventory, party, walkable tiles), a navigator tool and summarization near 200k tokens; failures were visual (confusing its own sprite with NPCs, walking into walls). In June 2026 Anthropic stated Claude Fable 5 beat FireRed with a minimal vision-only harness on raw screenshots; no step or token counts were published. https://www.anthropic.com/research/visible-extended-thinking ; https://www.anthropic.com/news/claude-fable-5-mythos-5
- **Gemini Plays Pokémon** (2025): a Lua bridge reads RAM (party, inventory, position) plus on-screen text, a grid overlay and a JSON fog-of-war map given as text; pathfinder sub-agents; summaries every 100 turns. Author: both models "struggled to utilize the raw pixels of the Game Boy screen directly"; "the current harness extracts enough state from RAM that an image of the Game Boy screen is almost redundant." Gemini 3 Pro finished Crystal in 24,178 turns and 1.88 billion tokens. https://blog.jcz.dev/the-making-of-gemini-plays-pokemon
- **SmartPlay** (2023): six games rendered as text because most LLMs lacked vision. https://arxiv.org/html/2310.01557v5
- **Cradle** (2024): screenshots in, keyboard and mouse out; the model "struggles with accurately recognizing and locating objects near the player", games must be paused for inference latency. https://arxiv.org/html/2403.03186v3
- **BALROG** (ICLR 2025): six environments rendered to text; adding images hurt GPT-4o (32.3 to 22.6 percent); the September 2026 leaderboard's top five are all language-only. https://arxiv.org/html/2411.13543v2 ; https://balrogai.com
- **2025 to 2026 successors**: VideoGameBench (raw frames only; best models complete 0.48 percent, 1.6 percent paused; bottlenecks are latency, misperception, screen-to-action mapping, memory) https://arxiv.org/html/2505.18134v3 ; lmgame-Bench adds a perception module converting screens to symbolic state because of "brittle vision perception" https://arxiv.org/abs/2505.15146 ; PokeAgent Challenge (NeurIPS 2025): raw frontier models scored effectively zero; winners used LLM subgoals plus scripted policies https://arxiv.org/html/2603.15563v1 ; OmniGameArena (UE5) observes only an RGB frame plus timestamp https://arxiv.org/html/2606.09826.

Pattern: every system that made real progress before mid-2026 was fed structured state; vision-only became viable only with the strongest 2026 models, at far higher step and token cost.

## 2. Structured observation frameworks

- Unity ML-Agents (4.1.0, July 2026): vector, ray perception, grid, camera and buffer sensors, goal signals, action masking. Guidance: "Visual observations should generally only be used when vector observations are not sufficient"; they are slower to train and sometimes fail. https://github.com/Unity-Technologies/ml-agents/blob/main/docs/Learning-Environment-Design-Agents.md
- Godot RL Agents: observation dictionaries, raycast sensors, TCP and JSON bridge. https://github.com/edbeeching/godot_rl_agents
- Unreal Learning Agents (UE 5.8, experimental): a typed observation schema (Location, Direction, Rotation, Velocity, Transform, Float, Bool, Enum, Count, Set, Array, Struct, ExclusiveUnion, Pair) with observation logging. https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Plugins/LearningAgents/ULearningAgentsObservations
- Gymnasium and PettingZoo: Box, Discrete, MultiBinary, MultiDiscrete and Text spaces; composite Dict, Tuple, Sequence, Graph, OneOf; step returns observation, reward, terminated, truncated, info; per-agent spaces. https://gymnasium.farama.org/api/spaces/
- Precedents: OpenAI Five observed Dota through the bot API as about 20,000 numbers; PySC2 exposes feature layers plus raw unit lists.

## 3. AI playtesting

- modl.ai: black-box vision and OCR; the Riot shooter bots instead used in-engine low-resolution raycast sensors and reported movement and accuracy comparisons and spatial heatmaps. https://modl.ai/riot-games-and-modl-shooter-bots
- EA SEED (2020, 2023): vector observations (relative goal, velocity, rotation, ground and climb flags, cooldowns, twelve raycasts); outputs coverage heatmaps, exploits such as missing collision, stuck areas. https://ar5iv.labs.arxiv.org/html/2103.15819 ; https://arxiv.org/abs/2307.11105
- Ubisoft La Forge: RL bots run balance tests overnight. https://www.ubisoft.com/en-us/studio/laforge/news/4bmoklgq9Hynfa87doKFfQ/artificial-intelligence-through-learning-or-pavlovian-algorithm
- LLM testers: TITAN (NetEase and ZJU, September 2025) discretizes API game state into levels and uses screenshots only for reflection; 95 percent task completion versus 82 percent for deep RL, 15 bugs found, deployed in eight QA pipelines https://arxiv.org/html/2509.22170v1 ; CA2 (May 2026) feeds function call traces as observations https://arxiv.org/abs/2605.13918 ; GBQA (2026): 30 games, 124 bugs, the best model finds 48.4 percent https://arxiv.org/abs/2604.02648.

## 4. Recording, replay, time travel

- **Rerun** (0.38.1, 17 Sep 2026; Apache-2.0 and MIT; 11.5k stars; Python, Rust and C++ SDKs): entity paths are folder-like, components are Arrow arrays, archetypes bundle components; timelines are built-in log time and tick plus user sequence, timestamp and duration indices; data may sit on several timelines; static data shadows all; chunks are per-entity Arrow batches; queries are latest-at or range; a dataframe API (DataFusion) exists on the Python and catalog side; the .rrd format is stable with migration tooling. Positioned as "the data layer for physical AI" with a commercial hub. https://github.com/rerun-io/rerun ; https://rerun.io/docs/concepts/logging-and-ingestion/entity-component ; https://rerun.io/docs/concepts/logging-and-ingestion/timelines
- **Tomorrow Corporation** (2023): custom VM; scrub a recording like video and reverse-step; state capture is mostly a memcpy of the game heap at frame boundaries, with coarse snapshots every two minutes and fine ones spaced exponentially. https://tomorrowcorporation.com/posts/how-we-make-games-at-tomorrow-corp-our-custom-tools-tech-demo
- **Unreal Rewind Debugger** (5.8): records Trace files; tracks for animation states, notifies and property changes; custom tracks possible.
- **Tracy** (0.14.1, August 2026, BSD-3): nanosecond hybrid profiler with frames, GPU, memory, locks and screenshots attached to frames. **Perfetto**: SQL over trace tables. https://github.com/wolfpld/tracy ; https://perfetto.dev/docs/analysis/trace-processor
- **Determinism**: Factorio's lockstep sends only inputs; a per-tick map CRC during replay finds the first divergent tick; desync reports dump both peers' state; known causes include Lua table iteration order and unsaved cached values. https://wiki.factorio.com/Desynchronization. Overwatch (GDC 2017): ECS, 16 ms command frames, clients re-simulate buffered inputs after server corrections because movement is highly deterministic. https://www.gdcvault.com/play/1024001/-Overwatch-Gameplay-Architecture-and

## 5. Event segmentation and summarization

- Event Segmentation Theory (Zacks et al. 2007): boundaries are transient prediction-error spikes; segmentation is hierarchical; boundary content is better remembered. https://pmc.ncbi.nlm.nih.gov/articles/PMC2852534/ Kumar et al. 2023: transient Bayesian surprise predicts human boundaries. EM-LLM (ICLR 2025) segments token streams online with surprise plus graph refinement. https://arxiv.org/abs/2407.09450
- Change-point methods: PELT is offline; Bayesian online change-point detection (Adams and MacKay 2007) runs online via a run-length posterior, cheap per tick on a few scalar channels. https://arxiv.org/abs/0710.3742
- Trajectory to text: the Pokémon harnesses summarize on context reset; a Microsoft CoG 2024 paper turns game logs into narrative node graphs with GPT-4. https://arxiv.org/abs/2404.17027

## 6. Token efficiency

- Claude vision pricing: 28-pixel patches; a 1920x1080 image costs about 1,560 tokens (standard) or 2,691 (high resolution). https://platform.claude.com/docs/en/build-with-claude/vision
- OSWorld (2024): accessibility-tree GPT-4 12.24 percent versus screenshot GPT-4V 5.26 percent; about 6,000 tokens covers 90 percent of accessibility observations; unfiltered trees run to millions of tokens. https://arxiv.org/html/2404.07972 VisualWebArena: text-only 7.25 to Set-of-Mark 16.37 percent, so visually grounded tasks still need pixels or captions.
- Practitioner numbers: a full-page accessibility tree about 840 tokens versus about 950 for a screenshot, but a scoped subtree about 50 tokens; each screenshot adds about 0.8 s of inference latency; a vision agent used about 10.4k tokens per step over 53 steps versus about 1.5k tokens per call over 8 structured calls, 45 times cheaper and 20 seconds instead of 17 minutes. https://reflex.dev/blog/computer-use-is-45x-more-expensive-than-structured-apis/ (April 2026)

## Lessons for a new engine (agent's assessment)

1. Make a typed semantic snapshot plus event stream the primary agent interface; frames are optional.
2. The saving is scoping, not format: a 50-token subtree beats an all-or-nothing frame. Offer radius, visibility and relevance filters.
3. Expose what a player could know, in tiers. Unlimited RAM access made Gemini "conditioned to ignore" vision; PokeAgent deliberately hides puzzle state.
4. Use Gymnasium or Learning Agents style schemas (Dict, Sequence, Graph, per-agent spaces, action masks) and ship a text renderer per archetype.
5. Provide pathfinding and macro actions as tools; LLMs burn tens of thousands of steps on movement.
6. Treat determinism as a feature: fixed tick, input log, per-tick state hash, quantized floats. Heap snapshots cover the non-deterministic remainder.
7. Log on multiple indices (tick, wall clock, agent turn) in columnar form so a viewer and dataframe queries share one store.
8. Rerun can be the backbone for storage, viewer and query semantics. It is not a replay system: it stores what was logged and cannot re-simulate, its query API is Python and catalog side rather than in-engine, it has no gameplay semantics (events, causality, entity lifetimes), no segmentation or summarization, and its roadmap targets robotics.
9. Keep a discrete domain-event layer (damage, pickup, objective) beside state; designers want coverage heatmaps, stuck areas, exploit lists and pass or fail per natural-language task.
10. Segment sessions online with Bayesian change-point detection on a few channels plus model surprise; use boundaries to trigger summaries, memory writes and the decision to send a frame at all.
11. Never put the LLM in the tick: pause-on-decision or asynchronous command frames.
12. Give agents and designers the same replay handle (seek to tick) so every summary is auditable.
