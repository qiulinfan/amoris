# Localization evidence

`ui-zh.png` (2026-09-29): `samples/ui` at 960 by 540 after `locale.set {lang: "zh"}` from the control server three ticks in: the HUD (`得分 0`, `生命 100`, and the plural `还没有金币` for no coins), the hint and the buttons from `locales/zh.json`, redrawn from the next tick's language without the script doing anything.

Reproduce: run the sample served (`pocket run ui -- --serve 4711 --paused`), call `locale.set {lang: "zh"}`, `step {ticks: 2}`, `capture {path}`; `pocket scenario ui` and `runtime_tests "[locale]"` check it.
