// Small helpers of the mock host: structured errors in the host's `{code, message, detail}` form,
// "did you mean" suggestions, deep copies and a stable hash.

export class MockError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly detail: Record<string, unknown> = {},
  ) {
    super(message);
  }

  toWire() {
    return { code: this.code, message: this.message, detail: this.detail };
  }
}

function distance(a: string, b: string): number {
  const dp = Array.from({ length: a.length + 1 }, (_, i) => [i, ...new Array<number>(b.length).fill(0)]);
  for (let j = 1; j <= b.length; j++) dp[0]![j] = j;
  for (let i = 1; i <= a.length; i++) {
    for (let j = 1; j <= b.length; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      dp[i]![j] = Math.min(dp[i - 1]![j]! + 1, dp[i]![j - 1]! + 1, dp[i - 1]![j - 1]! + cost);
    }
  }
  return dp[a.length]![b.length]!;
}

/** The nearest names by edit distance, as the runtime's suggestions do. */
export function suggest(unknown: string, valid: Iterable<string>, max = 3): string[] {
  const limit = Math.max(2, Math.floor(unknown.length / 3));
  return [...valid]
    .map((v) => ({ v, d: distance(unknown.toLowerCase(), v.toLowerCase()) }))
    .filter((x) => x.d <= limit)
    .sort((a, b) => a.d - b.d || a.v.localeCompare(b.v))
    .slice(0, max)
    .map((x) => x.v);
}

export function unknownField(field: string, valid: string[], where: string): MockError {
  const did = suggest(field, valid);
  return new MockError(
    "request.unknown_field",
    `${where} has no field '${field}'${did.length ? `; did you mean '${did[0]}'?` : "."}`,
    { field, did_you_mean: did, valid },
  );
}

export function clone<T>(v: T): T {
  return structuredClone(v);
}

/** FNV-1a over a JSON string, as 16 hex digits (two 32-bit halves). */
export function hashJson(v: unknown): string {
  const s = JSON.stringify(v);
  let h1 = 0x811c9dc5;
  let h2 = 0x01000193 ^ 0x9e3779b9;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    h1 = Math.imul(h1 ^ c, 0x01000193) >>> 0;
    h2 = Math.imul(h2 ^ c, 0x01000193 ^ 0x5bd1e995) >>> 0;
  }
  return h1.toString(16).padStart(8, "0") + h2.toString(16).padStart(8, "0");
}

export function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** A deterministic pseudo-random sequence (mulberry32) so mock runs repeat. */
export function rng(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
