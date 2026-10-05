# Agents playing and developing games

DeepSeek Flash uses tools to play a native Amoris sailing game and modify TypeScript gameplay
systems. The model makes decisions between simulation steps; the Rust host runs the game.
The gameplay demonstration and development benchmark measure different tasks.

## Native gameplay

The player receives a project-level tool projection with a **30 m observation range**.
It cannot call the developer's arbitrary world edits, script tools or snapshots through this
projection. The boat moves through the engine's native Boat physics, driven by wind, sail,
keel and rudder; tool calls submit intentions and advance fixed ticks.

| Player tool | Purpose |
| --- | --- |
| `observe` | Read the boat's player-facing state and nearby objects within the range limit |
| `helm` | Set sailing intentions for the next simulation steps |
| `navigate` | Select a nearby destination for the game's helmsman |
| `take` | Request a nearby parcel collection under the game's reach rule |
| `wait` | Advance a bounded number of simulation ticks |

`navigate` delegates continuous steering to the disclosed game-side **Helm executor**.
DeepSeek chooses destinations and other actions; it does not compute every rudder update.
This demonstration uses structured observations, rather than a vision model playing from
rendered pixels. The range filter is implemented by the sample's tool projection. Engine-wide
player authorization and pixel-based occlusion are still pending.

The three 2,048-token pretests collected **1/4, 1/4 and 2/4**; every response limit
was recorded. With low effort and an 8,192-token cap, the three formal trials collected
**2/4, 4/4 and 4/4** (one length stop, two clean completions). A separate non-thinking,
4,096-token condition collected **2/4, 2/4 and 3/4** (transport error, request limit,
and a clean but incomplete result). Conditions are reported separately.

The selected video is the **first successful low-effort trial**, repetition 2: **4/4 cargo,
7 worth, 1,577 simulated ticks**. Every action, response and final world hash matched a
fresh native replay. It is encoded at 30 fps from two-tick samples; recorded playback
speed is separate from model wall time or rendering performance.

<video controls playsinline preload="none" style="width:100%" poster="../../media/deepseek-gameplay-poster.jpg">
<source src="../../media/deepseek-gameplay.mp4" type="video/mp4">
<track kind="captions" src="../../media/deepseek-gameplay.vtt" srclang="en" label="English" default>
</video>

## Game-development benchmark

The frozen `courier-v1` experiment has **six tasks × three independent trials = 18 runs**:
two bug fixes and four gameplay features. Each trial starts from a fresh candidate project
with one defect or missing implementation. The agent reads, writes, applies and checks
scripts through the native host's developer API. It may edit only the specified module.

| Task | Kind | Required behavior | Passed / trials |
| --- | --- | --- | --- |
| Collection range | Bug fix | Inclusive horizontal radius 3 and vertical reach 2 | 3 / 3 |
| Parcel value scoring | Bug fix | Score each parcel's actual value exactly once, including zero | 3 / 3 |
| Collection victory | Feature | Persist and emit one victory for a completed nonempty objective | 3 / 3 |
| Dash cooldown | Feature | Four-times movement and a component-based five-tick cooldown | 3 / 3 |
| Score milestone | Feature | One score-ten milestone per courier, preserved across reload | 3 / 3 |
| Damage and defeat | Feature | Armor-reduced damage, clamped health and one defeat event | 3 / 3 |

**18/18 candidate patches passed strict grading**. All also passed types, native
integrity, the five other feature suites and file scope. **15/18 sessions ended cleanly**;
three provider disconnects occurred after valid patches and remain separate in the results.

Median agent wall time: **86.95 s**. Median native tool calls:
**58**. Overall prompt-cache hit fraction across reported usage:
**97.29%**. Recorded developer calls cost at most
**$0.416942** at the published peak rates, plus unsettled requests.

[Every trial, final patch and tool trace](evidence/agents/result.json) ·
[Readable result tables](evidence/agents/result.md).

A trial passes only if its feature, all five unrelated feature suites, TypeScript checking,
file scope and native determinism/fork/replay/reload checks pass. The golden controls exercise
**63 behavioral assertions** through real component state and emitted events. Checks include
positive, negative and boundary cases, forced reload during dash cooldown, and an explicit
Play fork that must leave the edit world unchanged after Stop.

Prompts and grader checks are frozen before model calls. The agent cannot read the grader,
golden source, other trials or saved evidence. Every initial control fails its intended
feature while passing types, native integrity and unrelated features; restoring the golden
target module passes the entire suite. Controls establish that the test can distinguish a
working implementation; they are not model successes.

All 18 scheduled trials belong in the results, including refusals, provider errors, timeouts
and budget stops. Feature-only success is a diagnostic, not a substitute for the strict
all-or-nothing result. These are short ECS gameplay tasks in one small game, with three
repetitions each; they do not establish a general coding ranking or a comparison with other
models.

## Model, tools and cost accounting

The requested alias is `deepseek-flash`; DeepSeek currently maps it to
**DeepSeek-V4.1-Flash**. Model aliases can change, so each run records the request name and
provider response metadata. See DeepSeek's [model and pricing documentation](https://api-docs.deepseek.com/quick_start/pricing/).

The runner uses a direct Chat Completions tool loop, following DeepSeek's
[tool-calling protocol](https://api-docs.deepseek.com/guides/tool_calls/). The client executes
approved native tools and returns their results to the model. This path was chosen because
the inspected OpenCode setup has no suitable per-request budget hook for this experiment.

Gameplay and all development trials share one **$5 budget ledger**. It checks the budget
before each provider request. Credentials are supplied through an environment variable or
an external key file; they are never embedded in game projects or public evidence. Public
records retain actions, tool results and usage totals without `reasoning_content`.

The accounting uses documented peak rates as a conservative upper bound: per million tokens,
**$0.30** cache-miss input, **$0.006** cache-hit input and **$1.20** output, as checked on
5 October 2026. The ledger's token-based upper bound is not an invoice; `billing_cost_usd`
remains `null` unless a run-specific provider bill is available.
[Pricing and billing rules](https://api-docs.deepseek.com/quick_start/pricing/).

All 18 development runs and all nine gameplay trials shared the authorized **$5** ceiling.
Reported usage gives **$0.694152** as the known peak-price
cost upper bound. **4 interrupted requests** have no returned usage;
the ledger retains **$1.277952** of conservative reservations.
The total committed upper bound is **$1.972104**, below $5.
The exact provider bill is unavailable and remains `null`. No automatic retries discard
these unknown charges.

## Reproduce and inspect

The [frozen protocol](https://github.com/qiulinfan/amoris/blob/main/bench/agent-dev/protocol.md),
[task prompts and hashes](https://github.com/qiulinfan/amoris/blob/main/bench/agent-dev/manifest.json),
[control results](https://github.com/qiulinfan/amoris/blob/main/bench/agent-dev/control-results.json)
and [native grader](https://github.com/qiulinfan/amoris/blob/main/tools/eval/agent_dev_bench.py)
define the development experiment independently of the provider runner.

```sh
python3 tools/eval/agent_dev_bench.py list
python3 tools/eval/agent_dev_bench.py selftest \
  --pocket ./target/release/pocket --output out/agent-dev-controls
python3 tools/eval/agent_dev_bench.py prepare --task dash-cooldown \
  --project /absolute/fresh/candidate
python3 tools/eval/agent_dev_bench.py grade --task dash-cooldown \
  --project /absolute/fresh/candidate --pocket ./target/release/pocket \
  --output /absolute/fresh/grade
```

Use fresh candidate paths and run grades sequentially. The script refuses to overwrite an
existing candidate. Provider runs additionally need the frozen prompts, tool restrictions,
trial limits and shared budget ledger; the grader itself makes no model calls.

For the common native command surface, see [CLI and MCP](api.md). For the underlying
stateless systems and generated declarations, see [Script API](sdk.md).

Run the real agents with one shared ledger (the credential file stays outside the repo):

```sh
python3 tools/showcase_assets.py --projects
python3 tools/eval/run_agent_showcase.py --mode development \
  --credential-file /external/deepseek-key.txt --repeats 3 \
  --output out/agent-showcase/development-v1
python3 tools/eval/run_agent_showcase.py --mode gameplay \
  --credential-file /external/deepseek-key.txt --repeats 3 \
  --game-output-tokens 8192 --game-effort low \
  --output out/agent-showcase/gameplay-v2
```

A different run needs fresh output directories. The default shared ledger keeps the
combined ceiling at $5; reusing it does not reset previous charges or reservations.
