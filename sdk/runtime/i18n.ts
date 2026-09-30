// Localization (docs/design/localization.md): the game's words in one file per language,
// locales/<lang>.json, looked up by key with `t`. The language is the session's (project.toml
// [locale] default, `locale.set` from an agent, `i18n.use` from a script); the interface draws
// again when it changes.
import { command } from "./world";
import { invalidate } from "./ui";

type Vars = Record<string, string | number | boolean | Date>;

interface I18nState {
    lang: string;
    rev: number;
    tables: Map<string, Record<string, string>>;
    misses: Set<string>;
}

// Kept on the shared global: every bundle of a project sees the same language and tables.
const state: I18nState = (() => {
    const g = globalThis as unknown as { __pocket_i18n?: I18nState };
    if (g.__pocket_i18n === undefined) g.__pocket_i18n = { lang: "", rev: -1, tables: new Map(), misses: new Set() };
    return g.__pocket_i18n;
})();

function current(): string {
    if (state.lang === "") state.lang = (command("locale.get") as { lang: string }).lang;
    return state.lang;
}

function table(lang: string): Record<string, string> {
    let t = state.tables.get(lang);
    if (t === undefined) {
        try {
            t = (command("locale.table", { lang }) as { strings: Record<string, string> }).strings;
        } catch {
            t = {};
        }
        state.tables.set(lang, t);
    }
    return t;
}

/** Called with every tick's language and files' revision: an agent's locale.set, or an edited file, reaches the scripts this way. */
export function setTickLocale(lang: string | undefined, rev: number | undefined): void {
    let changed = false;
    if (rev !== undefined && rev !== state.rev) {
        if (state.rev !== -1) { state.tables.clear(); changed = true; }
        state.rev = rev;
    }
    if (lang !== undefined && lang !== state.lang) {
        state.lang = lang;
        changed = true;
    }
    if (changed) invalidate();
}

type IntlLike = {
    NumberFormat?: new (l: string, o?: Record<string, unknown>) => { format(n: number): string };
    DateTimeFormat?: new (l: string, o?: Record<string, unknown>) => { format(d: Date): string };
};
const intlApi = (): IntlLike | undefined => (globalThis as unknown as { Intl?: IntlLike }).Intl;

/** How a number is written: `digits` after the point (as many as it has, up to three, when not given), a percentage (0.25 is 25%), grouping off. */
export interface NumberOptions {
    digits?: number;
    percent?: boolean;
    grouping?: boolean;
}

// A number as the language writes it, its decimal mark and grouping (1,234.5 / 1.234,5 /
// 1 234,5) from the platform's Intl when it knows the language, else English's.
function formatNumber(lang: string, n: number, o: NumberOptions = {}): string {
    const opts: Record<string, unknown> = { useGrouping: o.grouping !== false };
    if (o.percent) opts.style = "percent";
    if (o.digits !== undefined) {
        opts.minimumFractionDigits = o.digits;
        opts.maximumFractionDigits = o.digits;
    } else {
        opts.maximumFractionDigits = o.percent ? 0 : 3;
    }
    const intl = intlApi();
    if (intl?.NumberFormat && lang) {
        try { return new intl.NumberFormat(lang, opts).format(n); } catch { /* a language it does not know: as below */ }
    }
    const v = o.percent ? n * 100 : n;
    const digits = o.digits ?? (o.percent ? 0 : Math.min(3, (String(Math.abs(v)).split(".")[1] ?? "").length));
    const [whole, frac] = Math.abs(v).toFixed(digits).split(".");
    const grouped = o.grouping === false ? whole : whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
    return `${v < 0 ? "-" : ""}${grouped}${frac ? `.${frac}` : ""}${o.percent ? "%" : ""}`;
}

/** How a date or time is written: a short, medium or long date, the time of day, or both. */
export type DateStyle = "short" | "medium" | "long" | "time" | "datetime";

function formatDate(lang: string, when: number | Date, style: DateStyle = "medium"): string {
    const d = when instanceof Date ? when : new Date(when);
    const intl = intlApi();
    if (intl?.DateTimeFormat && lang) {
        const opts: Record<string, unknown> =
            style === "time" ? { timeStyle: "short" } : style === "datetime" ? { dateStyle: "medium", timeStyle: "short" } : { dateStyle: style };
        try { return new intl.DateTimeFormat(lang, opts).format(d); } catch { /* as below */ }
    }
    const pad = (x: number) => String(x).padStart(2, "0");
    const date = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
    const time = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
    return style === "time" ? time : style === "datetime" ? `${date} ${time}` : date;
}

function plural(lang: string, n: number): string {
    const intl = (globalThis as unknown as { Intl?: { PluralRules?: new (l: string) => { select(n: number): string } } }).Intl;
    if (intl?.PluralRules) {
        try { return new intl.PluralRules(lang).select(n); } catch { /* an unknown language: English's rule */ }
    }
    return n === 1 ? "one" : "other";
}

// {name}, {n, plural, =0 {none} one {# coin} other {# coins}}, {g, select, a {..} other {..}},
// {n, number[, integer|percent]}, {d, date[, short|medium|long]} and {d, time}.
function format(text: string, vars: Vars, lang: string): string {
    let out = "";
    let i = 0;
    while (i < text.length) {
        const c = text[i];
        if (c !== "{") { out += c; i++; continue; }
        // The placeholder's extent, braces nested.
        let depth = 0, j = i;
        for (; j < text.length; j++) {
            if (text[j] === "{") depth++;
            else if (text[j] === "}" && --depth === 0) break;
        }
        if (j >= text.length) { out += text.slice(i); break; }
        const body = text.slice(i + 1, j);
        i = j + 1;
        const comma = body.indexOf(",");
        const name = (comma < 0 ? body : body.slice(0, comma)).trim();
        const value = vars[name];
        if (comma < 0) { out += value === undefined ? `{${name}}` : String(value); continue; }
        const rest = body.slice(comma + 1);
        const comma2 = rest.indexOf(",");
        const kind = (comma2 < 0 ? rest : rest.slice(0, comma2)).trim();
        if (kind === "number" || kind === "date" || kind === "time") {
            const style = comma2 < 0 ? "" : rest.slice(comma2 + 1).trim();
            if (value === undefined) { out += `{${body}}`; continue; }
            if (kind === "number") out += formatNumber(lang, Number(value), style === "integer" ? { digits: 0 } : style === "percent" ? { percent: true } : {});
            else {
                const when = value instanceof Date ? value : Number(value);
                out += formatDate(lang, when, kind === "time" ? "time" : style === "short" || style === "long" ? style : "medium");
            }
            continue;
        }
        const options = new Map<string, string>();
        const opts = comma2 < 0 ? "" : rest.slice(comma2 + 1);
        for (let k = 0; k < opts.length;) {
            while (k < opts.length && /\s/.test(opts[k])) k++;
            let key = "";
            while (k < opts.length && opts[k] !== "{" && !/\s/.test(opts[k])) key += opts[k++];
            while (k < opts.length && /\s/.test(opts[k])) k++;
            if (opts[k] !== "{") break;
            let d = 0, e = k;
            for (; e < opts.length; e++) {
                if (opts[e] === "{") d++;
                else if (opts[e] === "}" && --d === 0) break;
            }
            options.set(key, opts.slice(k + 1, e));
            k = e + 1;
        }
        let chosen: string | undefined;
        if (kind === "plural") {
            const n = Number(value ?? 0);
            chosen = options.get(`=${n}`) ?? options.get(plural(lang, n)) ?? options.get("other");
            if (chosen !== undefined) chosen = format(chosen, vars, lang).replace(/#/g, formatNumber(lang, n));
        } else if (kind === "select") {
            chosen = options.get(String(value)) ?? options.get("other");
            if (chosen !== undefined) chosen = format(chosen, vars, lang);
        }
        out += chosen ?? `{${body}}`;
    }
    return out;
}

/** The text for `key` in the current language (then the default language, then the key itself), its placeholders filled from `vars`. */
export function t(key: string, vars: Vars = {}): string {
    const lang = current();
    let text: string | undefined = table(lang)[key];
    if (text === undefined) {
        state.misses.add(`${lang}:${key}`);
        const fallback = (command("locale.get") as { default: string }).default;
        text = fallback !== lang ? table(fallback)[key] : undefined;
        if (text === undefined) return key;
    }
    return format(text, vars, lang);
}

export const i18n = {
    /** Switch to a language (locales/<lang>.json must exist); the interface draws again. */
    use(lang: string): void {
        command("locale.set", { lang });
        state.lang = lang;
        invalidate();
    },
    /** The current language. */
    language(): string {
        return current();
    },
    /** The languages the project has a file for, and the default. */
    languages(): { languages: string[]; default: string } {
        return command("locale.get") as { languages: string[]; default: string };
    },
    /** Keys `t` was asked for that the current language lacked, as "lang:key", since the start. */
    misses(): string[] {
        return [...state.misses];
    },
    /** Read the language files again (after they were edited). */
    reload(): void {
        state.tables.clear();
        invalidate();
    },
    /** Every language's keys against the default's: what is missing and what is extra. */
    check(): unknown {
        return command("locale.check");
    },
    /** Fill a text's placeholders without a table (for text made in code). */
    format(text: string, vars: Vars = {}): string {
        return format(text, vars, current());
    },
    /** A number as the current language (or `lang`) writes it: 1,234.5 in English, 1.234,5 in German. */
    number(n: number, options: NumberOptions = {}, lang: string = current()): string {
        return formatNumber(lang, n, options);
    },
    /** A date (a Date, or milliseconds since 1970) as the current language (or `lang`) writes it. */
    date(when: number | Date, style: DateStyle = "medium", lang: string = current()): string {
        return formatDate(lang, when, style);
    },
};
