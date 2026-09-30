import { i18n } from "pocket";
import { expect, test } from "pocket/test";

test("numbers as a language writes them", () => {
    expect(i18n.number(1234.5, {}, "en")).toBe("1,234.5");
    expect(i18n.number(1234.5, {}, "de")).toBe("1.234,5");
    expect(i18n.number(-0.25, { percent: true }, "en")).toBe("-25%");
    expect(i18n.number(3.14159, { digits: 2 }, "en")).toBe("3.14");
    expect(i18n.number(1234567, { grouping: false }, "en")).toBe("1234567");
    // Placeholders: a number, a whole one, a percentage, and the number a plural stands for.
    expect(i18n.format("{n, number} of {total, number, integer} ({p, number, percent})", { n: 1500.25, total: 2000.4, p: 0.75 })).toBe("1,500.25 of 2,000 (75%)");
    expect(i18n.format("{n, plural, one {# coin} other {# coins}}", { n: 1200 })).toBe("1,200 coins");
    expect(i18n.format("{n, number}", {})).toBe("{n, number}");   // nothing to fill it with
});

test("dates and times as a language writes them", () => {
    const d = new Date(2026, 8, 29, 14, 5);
    expect(i18n.date(d, "long", "en")).toBe("September 29, 2026");
    expect(i18n.date(d, "long", "zh")).toBe("2026年9月29日");
    expect(i18n.date(d, "short", "de")).toBe("29.09.26");
    expect(i18n.date(d, "time", "de")).toBe("14:05");
    expect(i18n.format("{d, date, long}", { d })).toBe("September 29, 2026");
    expect(i18n.format("{d, date, long}", { d: d.getTime() })).toBe("September 29, 2026");   // milliseconds too
});
