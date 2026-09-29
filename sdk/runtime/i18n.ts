// Localization (docs/design/localization.md): the game's words in one file per language,
// locales/<lang>.json, looked up by key with `t`. The language is the session's (project.toml
// [locale] default, `locale.set` from an agent, `i18n.use` from a script); the interface draws
// again when it changes.
import { command } from "./world";
import { invalidate } from "./ui";

type Vars = Record<string, string | number | boolean>;

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

function plural(lang: string, n: number): string {
    const intl = (globalThis as unknown as { Intl?: { PluralRules?: new (l: string) => { select(n: number): string } } }).Intl;
    if (intl?.PluralRules) {
        try { return new intl.PluralRules(lang).select(n); } catch { /* an unknown language: English's rule */ }
    }
    return n === 1 ? "one" : "other";
}

// {name}, {n, plural, =0 {none} one {# coin} other {# coins}} and {g, select, a {..} other {..}}.
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
            if (chosen !== undefined) chosen = format(chosen, vars, lang).replace(/#/g, String(n));
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
};
