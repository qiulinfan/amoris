# Native agent showcase evidence

Snapshot generated 2026-10-05T09:18:50.302067+00:00. Only completed agent summaries are counted.

Developer trials: 18 observed / 18 planned; 18 independently graded. Gameplay trials: 6 formal observed / 6 formal planned; 3 pretests retained separately.

Behavioral acceptance and agent completion are reported separately. Every recorded trial, including timeouts and transport errors, remains in the denominator.

| Task | n | Behavioral pass | Types | Integrity | Regressions | Scope | Wall mean / median (s) | API mean / median | Tool mean / median |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Repair collection radius | 3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 71.97 / 67.77 | 32.67 / 32.00 | 49.67 / 49.00 |
| Repair parcel value scoring | 3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 88.78 / 93.48 | 33.33 / 34.00 | 57.00 / 59.00 |
| Implement collection victory | 3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 101.90 / 101.83 | 40.67 / 41.00 | 66.33 / 66.00 |
| Implement persistent dash cooldown | 3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 84.47 / 84.51 | 33.33 / 35.00 | 53.33 / 53.00 |
| Implement once-only score milestone | 3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 81.83 / 88.48 | 36.33 / 39.00 | 61.00 / 68.00 |
| Implement armored damage and defeat | 3 | 3/3 | 3/3 | 3/3 | 3/3 | 3/3 | 96.81 / 98.90 | 42.00 / 41.00 | 66.00 / 68.00 |

| Task | Prompt tokens mean / median | Cache-hit tokens mean / median | Output tokens mean / median | Known cache hit | Peak-rate known cost upper | Unknown reserved |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Repair collection radius | 438480.00 / 406214.00 | 425214.00 / 393597.00 | 9709.00 / 10405.00 | 96.97% | $0.054545652 | $0 |
| Repair parcel value scoring | 594017.00 / 477111.00 | 577322.67 / 464256.00 | 11194.67 / 11247.00 | 97.19% | $0.065717508 | $0.319488 |
| Implement collection victory | 899912.67 / 967723.00 | 877781.33 / 943232.00 | 12166.67 / 12419.00 | 97.54% | $0.079518264 | $0.319488 |
| Implement persistent dash cooldown | 801801.33 / 839915.00 | 777898.67 / 815488.00 | 12052.00 / 11902.00 | 97.02% | $0.078901776 | $0 |
| Implement once-only score milestone | 597069.00 / 616528.00 | 579285.33 / 601088.00 | 9777.67 / 11061.00 | 97.02% | $0.061632036 | $0.319488 |
| Implement armored damage and defeat | 796792.67 / 795731.00 | 778581.33 / 779392.00 | 12839.33 / 13326.00 | 97.71% | $0.076626264 | $0 |

Known provider-token totals use exact response usage; unknown requests contribute no invented tokens.

Known peak-rate cost upper bound across the shared ledger: **$0.694151928**. Unknown/unsettled request reservations: **$1.277952** (4 requests). Actual provider invoice: **unknown**.

Reservations protect the shared budget and are not measured spend.

## Trial outcomes

| Trial | Behavior | Agent finish | API / tool calls | Peak-rate known cost upper | Unknown reserved |
| --- | --- | --- | ---: | ---: | ---: |
| development-v1/collection-range-1 | pass | stop | 25 / 43 | $0.018351840 | $0 |
| development-v1/collection-range-2 | pass | stop | 41 / 57 | $0.021012330 | $0 |
| development-v1/collection-range-3 | pass | stop | 32 / 49 | $0.015181482 | $0 |
| development-v1/score-values-1 | pass | transport_or_tool_error | 31 / 59 | $0.020138436 | $0.319488 |
| development-v1/score-values-2 | pass | stop | 34 / 60 | $0.028428324 | $0 |
| development-v1/score-values-3 | pass | stop | 35 / 52 | $0.017150748 | $0 |
| development-v1/objective-victory-1 | pass | stop | 33 / 66 | $0.021621168 | $0 |
| development-v1/objective-victory-2 | pass | transport_or_tool_error | 41 / 63 | $0.027939492 | $0.319488 |
| development-v1/objective-victory-3 | pass | stop | 48 / 70 | $0.029957604 | $0 |
| development-v1/dash-cooldown-1 | pass | stop | 29 / 53 | $0.025493316 | $0 |
| development-v1/dash-cooldown-2 | pass | stop | 35 / 55 | $0.026438628 | $0 |
| development-v1/dash-cooldown-3 | pass | stop | 36 / 52 | $0.026969832 | $0 |
| development-v1/milestone-event-1 | pass | stop | 39 / 68 | $0.022220928 | $0 |
| development-v1/milestone-event-2 | pass | stop | 48 / 72 | $0.023038020 | $0 |
| development-v1/milestone-event-3 | pass | transport_or_tool_error | 22 / 43 | $0.016373088 | $0.319488 |
| development-v1/damage-and-defeat-1 | pass | stop | 41 / 68 | $0.029784444 | $0 |
| development-v1/damage-and-defeat-2 | pass | stop | 37 / 57 | $0.019616568 | $0 |
| development-v1/damage-and-defeat-3 | pass | stop | 48 / 73 | $0.027225252 | $0 |

## Gameplay

| Protocol / effort / max output | Phase | n | Native success | Wall mean / median (s) | API mean / median | Output tokens mean / median | Known peak upper | Unknown reserved |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| gameplay-v1 / low / 2048 | pretest | 3 | 0/3 | 84.72 / 69.44 | 30.00 / 27.00 | 11069.67 / 9060.00 | $0.055232280 | $0 |
| gameplay-v2 / low / 8192 | formal | 3 | 2/3 | 184.16 / 156.12 | 59.00 / 62.00 | 25166.00 / 17061.00 | $0.130148040 | $0 |
| gameplay-v3 / none / 4096 | formal | 3 | 0/3 | 151.58 / 144.95 | 84.67 / 89.00 | 10536.33 / 10132.00 | $0.091830108 | $0.319488 |

- gameplay-v1/episode-1 (pretest, max output 2048): 1 / 4 collected; native success False; agent finish `length`; 18 API calls and 17 tool calls.
- gameplay-v1/episode-2 (pretest, max output 2048): 1 / 4 collected; native success False; agent finish `length`; 27 API calls and 26 tool calls.
- gameplay-v1/episode-3 (pretest, max output 2048): 2 / 4 collected; native success False; agent finish `length`; 45 API calls and 44 tool calls.
- gameplay-v2/episode-1 (formal, max output 8192): 2 / 4 collected; native success False; agent finish `length`; 45 API calls and 44 tool calls.
- gameplay-v2/episode-2 (formal, max output 8192): 4 / 4 collected; native success True; agent finish `stop`; 70 API calls and 69 tool calls.
- gameplay-v2/episode-3 (formal, max output 8192): 4 / 4 collected; native success True; agent finish `stop`; 62 API calls and 61 tool calls.
- gameplay-v3/episode-1 (formal, max output 4096): 2 / 4 collected; native success False; agent finish `transport_or_tool_error`; 75 API calls and 74 tool calls.
- gameplay-v3/episode-2 (formal, max output 4096): 2 / 4 collected; native success False; agent finish `max_requests`; 90 API calls and 90 tool calls.
- gameplay-v3/episode-3 (formal, max output 4096): 3 / 4 collected; native success False; agent finish `stop`; 89 API calls and 88 tool calls.

Media candidate: **gameplay-v2/episode-2**. Highest recorded native collection score; ties first in protocol/repetition order. Illustrative selected run; all recorded trials and conditions remain visible.

## Frozen protocol

- development-v1: `courier-v1`; task IDs `collection-range, score-values, objective-victory, dash-cooldown, milestone-event, damage-and-defeat`; effort `high`.
- gameplay-v1: course `agent-sailing-downwind-v1`; effort `low`.
- gameplay-v2: course `agent-sailing-downwind-v1`; effort `low`.
- gameplay-v3: course `agent-sailing-downwind-v1`; effort `none`.

Actual response model aliases: `deepseek-flash`

- Six fixed development tasks and three planned sailing runs are a small, engine-specific study.
- Behavior is graded independently from model finish status; a transport interruption can leave a passing patch.
- Gameplay uses a project-level restricted gateway and a disclosed native Helm executor; this is not global player authorization.
- The model never receives the grader, golden files, private native state or shared budget ledger.
- Public summaries contain outcomes and method fields; detailed transcripts and patches are local-only.
- The 2048-token gameplay-v1 pretests, low-thinking 8192-token gameplay-v2 runs and non-thinking 4096-token gameplay-v3 runs are separate frozen conditions; their success rates and performance averages are not pooled.
- Any highest-score media selection is disclosed and does not replace the complete trial table.
