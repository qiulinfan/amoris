// Number, size and time formatting shared by the panels.

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(2)} GB`;
}

export function ms(n: number): string {
  if (n === 0) return "0";
  if (n < 0.01) return `${(n * 1000).toFixed(1)} µs`;
  if (n < 10) return `${n.toFixed(2)} ms`;
  return `${n.toFixed(1)} ms`;
}

export function num(n: number, digits = 3): string {
  if (!Number.isFinite(n)) return String(n);
  if (Number.isInteger(n)) return String(n);
  const s = n.toFixed(digits);
  return s.replace(/\.?0+$/, "") || "0";
}

export function clock(seconds: number): string {
  const m = Math.floor(seconds / 60);
  const s = seconds - m * 60;
  return `${m}:${s.toFixed(2).padStart(5, "0")}`;
}

export function timeOfDay(ts: number): string {
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}:${String(d.getSeconds()).padStart(2, "0")}`;
}

export function ago(ts: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ts) / 1000));
  if (s < 5) return "now";
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  return `${Math.floor(s / 3600)}h`;
}

/** Compact one-line JSON for tables. */
export function compactJson(v: unknown, max = 120): string {
  let s: string;
  try {
    s = JSON.stringify(v, (_k, x) => (typeof x === "number" && !Number.isInteger(x) ? Math.round(x * 1000) / 1000 : x));
  } catch {
    s = String(v);
  }
  if (s === undefined) return "";
  return s.length > max ? `${s.slice(0, max - 1)}…` : s;
}

/** A stable hue for a name (event kinds, systems). */
export function hue(name: string): number {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0;
  return h % 360;
}
