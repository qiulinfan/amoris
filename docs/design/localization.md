# Localization

A game's words belong in files, one per language, so a translator (a person, or an agent asked to
"add French") works on text and not on code, and so the game can say what is missing. Pocket keeps
them in `locales/<lang>.json` in the project, looks them up by key with `t`, and switches language
while the game runs.

## The files

```json
{
  "hud": {
    "score": "Score {score}",
    "coins": "{count, plural, =0 {No coins yet} one {# coin} other {# coins}}"
  },
  "menu": { "resume": "Resume", "difficulty": "Difficulty" }
}
```

A file is a JSON object of texts; objects group them, and a key is the path to a text (`hud.score`,
`menu.resume`). Every language has the same keys. A text may hold:

- **Placeholders**: `{name}` is replaced by the value of `name` given to `t`.
- **Plurals**: `{count, plural, =0 {...} one {...} other {...}}` picks the text for the number (`=N`
  for exactly N, else the language's category: `one`, `two`, `few`, `many`, `other`, from the
  platform's plural rules when it has them, else one for 1 and other for the rest), with `#`
  standing for the number; placeholders work inside.
- **Selects**: `{gender, select, female {...} male {...} other {...}}` picks by a string's value.
- **Numbers and dates**: `{n, number}` writes a number as the language does (1,234.5 in English,
  1.234,5 in German, up to three decimals), `{n, number, integer}` rounds it to a whole one and
  `{n, number, percent}` makes 0.75 into 75 %; `{d, date}` writes a date (a `Date`, or milliseconds
  since 1970) in the language's medium form, `{d, date, short}` and `{d, date, long}` shorter or
  longer, and `{d, time}` the time of day. The `#` of a plural is written the same way (1,200
  coins). These come from the platform's own tables (`Intl`), which the native runtime and every
  browser have; where a language is unknown the English form stands in. A bare `{name}` stays as it
  is given, so a year is not written 2,026.

`[locale] default = "en"` in `project.toml` names the language the game starts in and falls back to;
without it, `en` when there is an `en.json`, else the first language by name.

## In scripts

```ts
import { i18n, t } from "pocket";

<Label text={t("hud.score", { score: score() })} />      // Score 20 / 得分 20
i18n.use("zh");                                            // every interface draws again in Chinese
```

`t(key, vars)` answers the text in the current language, else in the default one, else the key
itself (so a missing text shows where it belongs; `i18n.misses()` lists the keys asked for that the
current language lacked). `i18n.use(lang)` switches, `i18n.language()` and `i18n.languages()` say
what there is, `i18n.format(text, vars)` fills a text made in code, `i18n.reload()` reads the files
again, and `i18n.number(n, {digits?, percent?, grouping?})` and `i18n.date(when, style)` write a
number or a date for the current language (or one given last) outside a text; `i18n.direction()`
says which way the language is written, for an interface's `dir` (`docs/design/pocket-ui.md`,
Right-to-left text). The language is the session's, so every bundle of the project agrees, a
language an agent sets reaches the scripts with the next tick, and the files are read again after
`project.reload` or a `project.write` under `locales/`. Pocket UI mounts draw again when the
language changes; text drawn once elsewhere keeps its language until it is drawn again. The default
UI font covers Latin and CJK; a language in another script needs a `font` in `project.toml` that has
it (`docs/design/pocket-ui.md`).

## Commands

| Command | SDK | Purpose |
|---|---|---|
| `locale.get` | `i18n.language()`, `i18n.languages()` | The current language, the default and the languages there are files for. |
| `locale.set {lang}` | `i18n.use(lang)` | Switch; refused (with the languages there are) when the file does not exist. Emits `locale.changed`. |
| `locale.table {lang?}` | | A language's texts by key. |
| `locale.check {base?}` | `i18n.check()` | Every language against the default: the keys it lacks, the keys only it has, the texts whose placeholders differ from the default's, and a file that does not read (a value that is not a text); `complete` when nothing is missing. What an agent asked to translate reads first and last. |

## The sample

`samples/ui` has its words in `locales/en.json`, `locales/zh.json` and `locales/ar.json` (26 texts
each; the Arabic drawn right to left with the fallback font, `docs/design/pocket-ui.md`,
Right-to-left text) and a Language choice in its pause menu; its HUD counts coins with a plural. Its
scenario (`pocket scenario ui`) reads the English interface, scores twenty points, switches to
Chinese and finds `得分 20` and `2 枚金币`, and switches back to `2 coins`; `runtime_tests` (`[locale]`)
read flattened tables, find a French file's missing key, extra key and dropped placeholder and a
file with a number for a text, switch the language and refuse one without a file;
`tests/ts/i18n.test.ts` writes numbers in English and German (grouping, decimals, whole numbers,
percentages, a plural's `#`) and a date long in English and Chinese, short in German and as a time.
The sample's score is written `{score, number}`. `tests/evidence/localization/ui-zh.png` is the
sample in Chinese.

## Not yet

Currencies and units in texts (`i18n.number` writes the number; the symbol is the text's), and
translated assets (a voice line or a texture per language, which a script can choose by
`i18n.language()` meanwhile).
