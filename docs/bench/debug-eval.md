# Agent-native debugging benchmark

An LLM agent receives a player's symptom, locates a planted TypeScript defect through the running
host, applies a fix, and has that fix checked on a fresh host. The benchmark measures engine-side
debugging usability; it does not compare models.

## Method

- Harness: [tools/eval/debug_eval.py](../../tools/eval/debug_eval.py). The six planted defects cover
  steering sign, radians/degrees, squared collection distance, stale anchor state, score order,
  and heading wrap. Fixtures live in `tools/eval/debug_eval/scripts/`.
- The game is a temporary copy of `samples/sailing` with a TypeScript helm layer. Agents receive
  the symptom and native CLI documentation, without evaluator code or the clean reference game.
- Conditions: free CLI, debugger-required CLI, and free MCP. Each run has 60 tool calls and a
  15-minute limit. A fresh host checks behavior, unchanged rule constants, exact defect location,
  and deterministic state parity over a 720-tick scenario. All six planted defects alter that
  scenario; negative controls check that changing rule constants cannot pass.
- The current harness isolates opencode configuration, skills and MCP settings and uses macOS
  `sandbox-exec` to confine shell access to the scratch project and loopback host. This sandbox is
  stricter than an ordinary developer checkout.
- Prompt, transcripts, diffs, state chains and raw results stay in ignored `out/debug-eval/` or an
  explicitly selected external output directory. They are not source files.

```sh
cargo build --release -p pocket-app
python3 tools/eval/debug_eval.py --selftest
# A real model run uses the configured provider and incurs API usage:
python3 tools/eval/debug_eval.py --bugs steer-sign --trials 1
python3 tools/eval/debug_eval.py --report
```

## Recorded results

Historical run date: 2026-10-04. Apple M5, macOS 27.0.1, release host `1e5b94f`, opencode 1.18.2,
`zai-coding-plan/glm-5.3-flash`. Other builds ran concurrently (load average 40–60), so wall times
are provisional. These runs predate the current isolation; the agent was not rerun after the
harness review. Recorded fixes were rechecked with the stronger scenario and location criterion.

| Condition | Runs | Fixed | Located | Median agent time | Median tool calls | Median tokens |
|---|---|---|---|---|---|---|
| Free CLI | 13 | 11 | 11 | 444 s | 30 | 634 k |
| Debugger-required CLI | 7 | 7 | 6 | 477 s | 31 | 404 k |
| Free MCP | 3 | 3 | 1 | 453 s | 32 | 492 k |

21 of 23 fixes passed; 18 named the exact planted statement. Every passing fix matched the clean
reference's state chain on the checked scenario. Matching state does not prove a minimal patch or
coverage beyond that scenario.

## Limits and engine follow-up

The historical agent sandbox allowed path escapes through shell filters, kept a clean game nearby,
and exposed bug names through temporary-directory names. Review found no recorded read of the
reference game, but absence in recorded traces is not proof of isolation. The current harness
removes those opportunities. The sample is small, there are only two or three trials per defect,
and the debugger condition prompts agents to use debugging tools; success rates remain indicative.

Pioneer's debugger work addressed held-loop requests, pause/stop races, and lexical-scope
evaluation. Its integration tests are retained in `pocket-debug`, `pocket-runtime`, `pocket-app` and
the editor. See [agent-debug.md](agent-debug.md) for the API changes and bounded response
measurements.
