// Randomness a game can replay, and noise for what it generates (docs/sdk.md, Randomness and noise).
// `rng` draws from the run's own stream (`random()`, seeded by the run, so replays, scenarios at
// many seeds and lockstep play agree); `new Rng(seed)` is a stream of its own, for a level made
// from a level's seed without moving the game's; `noise` is gradient noise (Perlin's improved
// noise) over a permutation made from a seed, the same numbers on every engine.
// The run's stream itself (what pocket's random() returns), read from the native directly: this
// module is exported from pocket, so it does not import from it.
declare const __pocket: { random(): number };
const random = (): number => __pocket.random();

/** What both the run's stream and a seeded one offer. */
export interface RandomSource {
    /** A number in [0, 1). */
    next(): number;
}

function int(src: RandomSource, lo: number, hi: number): number {
    const a = Math.ceil(Math.min(lo, hi)), b = Math.floor(Math.max(lo, hi));
    return a + Math.floor(src.next() * (b - a + 1));
}

function shuffle<T>(src: RandomSource, items: T[]): T[] {
    for (let i = items.length - 1; i > 0; i--) {
        const j = Math.floor(src.next() * (i + 1));
        const t = items[i];
        items[i] = items[j];
        items[j] = t;
    }
    return items;
}

const helpers = (src: RandomSource) => ({
    /** A whole number from lo to hi, both included. */
    int: (lo: number, hi: number) => int(src, lo, hi),
    /** A number in [lo, hi). */
    range: (lo: number, hi: number) => lo + src.next() * (hi - lo),
    /** One of the items (undefined for none). */
    pick: <T>(items: readonly T[]): T | undefined => (items.length ? items[Math.floor(src.next() * items.length)] : undefined),
    /** The items in a random order, in place (Fisher-Yates); returns them. */
    shuffle: <T>(items: T[]) => shuffle(src, items),
    /** True with probability p. */
    chance: (p: number) => src.next() < p,
    /** One of the keys, each as likely as its weight: weighted({ common: 8, rare: 1 }). */
    weighted: <K extends string>(weights: Record<K, number>): K => {
        const entries = Object.entries(weights) as Array<[K, number]>;
        const total = entries.reduce((s, [, w]) => s + Math.max(0, w), 0);
        let r = src.next() * total;
        for (const [k, w] of entries) {
            r -= Math.max(0, w);
            if (r < 0) return k;
        }
        return entries[entries.length - 1][0];
    },
});

/** The run's stream (what random() draws), with helpers: rng.int(1, 6), rng.pick(items), rng.shuffle(deck). */
export const rng = { next: () => random(), ...helpers({ next: () => random() }) };

/** A stream of its own from a seed (sfc32: the same numbers on every engine): new Rng(levelSeed).int(0, 9). */
export class Rng implements RandomSource {
    private a: number;
    private b: number;
    private c: number;
    private d: number;
    constructor(seed: number) {
        this.a = seed | 0;
        this.b = Math.floor(seed / 4294967296) ^ 0x9e3779b9;
        this.c = 0x243f6a88;
        this.d = 1;
        for (let i = 0; i < 12; i++) this.next();
    }
    next(): number {
        const t = (((this.a + this.b) | 0) + this.d) | 0;
        this.d = (this.d + 1) | 0;
        this.a = this.b ^ (this.b >>> 9);
        this.b = (this.c + (this.c << 3)) | 0;
        this.c = (this.c << 21) | (this.c >>> 11);
        this.c = (this.c + t) | 0;
        return (t >>> 0) / 4294967296;
    }
    int(lo: number, hi: number): number { return int(this, lo, hi); }
    range(lo: number, hi: number): number { return lo + this.next() * (hi - lo); }
    pick<T>(items: readonly T[]): T | undefined { return items.length ? items[Math.floor(this.next() * items.length)] : undefined; }
    shuffle<T>(items: T[]): T[] { return shuffle(this, items); }
    chance(p: number): boolean { return this.next() < p; }
}

/** Options of fractal noise: octaves (4), each `lacunarity` (2) times finer and `gain` (0.5) times weaker. */
export interface FbmOptions {
    octaves?: number;
    lacunarity?: number;
    gain?: number;
}

const fade = (t: number) => t * t * t * (t * (t * 6 - 15) + 10);
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
function grad3(h: number, x: number, y: number, z: number): number {
    const k = h & 15;
    const u = k < 8 ? x : y;
    const v = k < 4 ? y : k === 12 || k === 14 ? x : z;
    return ((k & 1) === 0 ? u : -u) + ((k & 2) === 0 ? v : -v);
}

/** Gradient noise from a seed: perlin2/perlin3 in about -1..1, smooth, 0 on whole numbers; fbm adds octaves. */
export class Noise {
    private readonly p: Uint8Array;
    constructor(seed = 0) {
        const r = new Rng(seed);
        const perm = r.shuffle(Array.from({ length: 256 }, (_, i) => i));
        this.p = new Uint8Array(512);
        for (let i = 0; i < 512; i++) this.p[i] = perm[i & 255];
    }
    perlin3(x: number, y: number, z: number): number {
        const p = this.p;
        const X = Math.floor(x) & 255, Y = Math.floor(y) & 255, Z = Math.floor(z) & 255;
        x -= Math.floor(x);
        y -= Math.floor(y);
        z -= Math.floor(z);
        const u = fade(x), v = fade(y), w = fade(z);
        const A = p[X] + Y, AA = p[A] + Z, AB = p[A + 1] + Z, B = p[X + 1] + Y, BA = p[B] + Z, BB = p[B + 1] + Z;
        return lerp(
            lerp(lerp(grad3(p[AA], x, y, z), grad3(p[BA], x - 1, y, z), u), lerp(grad3(p[AB], x, y - 1, z), grad3(p[BB], x - 1, y - 1, z), u), v),
            lerp(lerp(grad3(p[AA + 1], x, y, z - 1), grad3(p[BA + 1], x - 1, y, z - 1), u), lerp(grad3(p[AB + 1], x, y - 1, z - 1), grad3(p[BB + 1], x - 1, y - 1, z - 1), u), v),
            w,
        );
    }
    perlin2(x: number, y: number): number {
        return this.perlin3(x, y, 0.5);
    }
    /** Octaves of perlin2 summed and scaled back to about -1..1: hills, clouds, caves. */
    fbm2(x: number, y: number, options: FbmOptions = {}): number {
        const { octaves = 4, lacunarity = 2, gain = 0.5 } = options;
        let sum = 0, amp = 1, freq = 1, norm = 0;
        for (let i = 0; i < octaves; i++) {
            sum += amp * this.perlin2(x * freq, y * freq);
            norm += amp;
            amp *= gain;
            freq *= lacunarity;
        }
        return norm > 0 ? sum / norm : 0;
    }
    fbm3(x: number, y: number, z: number, options: FbmOptions = {}): number {
        const { octaves = 4, lacunarity = 2, gain = 0.5 } = options;
        let sum = 0, amp = 1, freq = 1, norm = 0;
        for (let i = 0; i < octaves; i++) {
            sum += amp * this.perlin3(x * freq, y * freq, z * freq);
            norm += amp;
            amp *= gain;
            freq *= lacunarity;
        }
        return norm > 0 ? sum / norm : 0;
    }
}

/** Noise from seed 0, ready to use: noise.perlin2(x * 0.1, y * 0.1) > 0.2 is rock. */
export const noise = new Noise(0);
