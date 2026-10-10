# Courier Arena development benchmark

Version: `courier-v1`. The provider runner owns model selection, independent repetitions,
API call budgets, token accounting, transcript retention and measured costs. This suite makes
no provider requests and reads no credentials.

Six independent tasks use one small, stateless ECS game. Each starts in a fresh project with
one defect or unimplemented feature; the other five features already work. Two tasks repair
bugs and four implement gameplay features.

| Task | Change allowed | Required behavior |
| --- | --- | --- |
| `collection-range` | `scripts/collect.ts` | Inclusive horizontal radius 3 and vertical reach 2; reject out of range; consume intent |
| `score-values` | `scripts/collect.ts` | Add cargo values exactly once, including zero; count parcels independently |
| `objective-victory` | `scripts/progress.ts` | Emit and persist one victory per nonempty completed objective |
| `dash-cooldown` | `scripts/dash.ts` | Four times movement; clamped axes; five-tick cooldown stored in a component |
| `milestone-event` | `scripts/progress.ts` | One score-ten milestone per courier, stable across recrossing and reload |
| `damage-and-defeat` | `scripts/damage.ts` | Consume armored damage, clamp health, and persist one defeat event |

`manifest.json` freezes the task prompts and fixture version before provider trials. Each grade
records the task, grader version and actual behavior checks. Do not alter prompts, fixtures or
checks mid-run. A later protocol change requires a new version and new independent trials.

## Candidate boundary

`prepare(task_id, dest)` creates only the candidate game at a fresh destination. It copies the
common template and substitutes the task's faulty or missing implementation. The agent gets
that project's native host URL, allowed module and prompt. Its tools may reach only that host's
approved script/world/time/play/developer documentation methods. They must not read this
repository, the golden template, the grader, other runs or saved evidence. The template is
reference code for fixture preparation and self-test; it is never served directly to a model.
The public `checks/scenario.jsonl` is a normal game-input scenario, not the hidden behavioral
assertion code. Agent edits are restricted to the task's one module.

The intended tool path is the same typed native command catalog used by the CLI, MCP and
editor. Script edits must be written and applied to the actual native host. This suite grades
what is left on disk on a fresh host, independently of the agent's completion claim.

## Grading

A task passes only when all of these pass:

- Its target behavior, including positive, negative and boundary cases.
- All five other feature suites, so a repair that breaks unrelated gameplay fails.
- Native script lint, declaration generation and TypeScript checking.
- Native determinism, fork, replay and reload checks on a real collection/dash/damage scenario.
- File scope: only the allowed module differs from the task's initial fixture.

Behavioral assertions use `/api/call` on an owned `pocket serve` process. They inspect component
state and emitted events, not source patterns. The dash check force-reloads during cooldown,
uses Play's explicit fork, then proves the edit world is unchanged after stopping Play. Other
checks force-reload once-only flags and test independent couriers. Every host has bounded
startup/API timeouts and is stopped after grading, including failed checks.

The strict all-or-nothing success rate is the primary metric. `feature_passed`,
`regressions_passed`, `types_passed`, `integrity_passed` and `scope_passed` are diagnostic metrics,
not alternate definitions of success. The calling runner should report each task and trial,
wall time, native tool calls, provider tokens and actual billed cost. Failed and budget-stopped
trials stay in the denominator. Do not turn these six small tasks into a claim about general
software-engineering ability or a model comparison without matched independent baselines.

## Run

```sh
python3 tools/eval/agent_dev_bench.py list
python3 tools/eval/agent_dev_bench.py prepare --task dash-cooldown --project /absolute/fresh/game
python3 tools/eval/agent_dev_bench.py grade --task dash-cooldown \
  --project /absolute/fresh/game --pocket /absolute/path/to/pocket \
  --output /absolute/fresh/grade
python3 tools/eval/agent_dev_bench.py selftest --pocket /absolute/path/to/pocket \
  --output /absolute/fresh/selftest
```

The Python API exposes `TASKS`, `prepare(task, dest)` and
`grade(task, project, pocketbin, output=None)`. The CLI exits nonzero for a failed grade or
self-test. Preparation refuses an existing candidate directory.

## Validation evidence

`selftest` proves each initial fixture passes types, native integrity and unrelated regressions,
fails its intended target, and passes the entire suite after the golden target module is
restored. `selftest.json` records these controls. Provider-run results belong in a separate
results artifact; controls are not model successes.
