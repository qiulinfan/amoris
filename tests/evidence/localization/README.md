# Localization evidence

`ui-zh.png` (2026-09-29): `samples/ui` at 960 by 540 after `locale.set {lang: "zh"}` from the control server three ticks in: the HUD (`得分 0`, `生命 100`, and the plural `还没有金币` for no coins), the hint and the buttons from `locales/zh.json`, redrawn from the next tick's language without the script doing anything.

`ui-ar.png` (2026-09-30): the same after `locale.set {lang: "ar"}`: the interface turned around by `dir={i18n.direction()}` (the HUD at the right edge through `start: 12`, its row read from the right: the name, `النقاط 0` with the score's number at the left of its right-to-left line, `الصحة 100`, the plural `لا عملات بعد`; the health bar and the hint right-aligned, the English `WASD` in the hint with its full stop at the far left; the buttons from the right), drawn with the bundled Noto Sans Arabic behind the Chinese face.

`ui-ar-menu.png` (2026-09-30): the pause menu in Arabic, the name field clicked: the labels on the right and their controls to their left, the choices' arrows mirrored (the previous one on the right, pointing right), the check box on the right of its label, the Resume button at the row's end (the left), the volume slider left to right as the SDK keeps it, and the English name `Player` at the right of its right-to-left field with the caret after it at the name's left end, the end side of its paragraph (the click fell left of the text).

Reproduce: run the sample served (`pocket run ui -- --serve 4711 --paused`), call `locale.set {lang: "zh"}`, `step {ticks: 2}`, `capture {path}`; `pocket scenario ui` and `runtime_tests "[locale]"` check it.
