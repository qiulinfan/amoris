// Reproducible math (docs/design/networking.md, Determinism): sin, cos, atan2, exp, pow and the
// rest giving the same double on every JavaScript engine. Math.sin and its kind are the engine's
// own (JavaScriptCore in a native build, V8 or another in a browser) and can differ in the last
// bit, so two peers of a lockstep game on different engines would drift apart. These use only
// + - * /, Math.sqrt and Math.floor, which every engine computes exactly as IEEE 754 says. Game
// logic that runs in lockstep and changes the world should use them; drawing and interfaces may
// keep Math. Accurate to a few units in the last place of a double.

const PI = 3.141592653589793;
const HALF_PI = 1.5707963267948966;
// pi/2 and ln 2 in two parts, the first short enough that k times it is exact.
const HALF_PI_HI = 1.57079632673412561417;
const HALF_PI_LO = 6.07710050650619224932e-11;
const LN2 = 0.6931471805599453;
const LN2_HI = 6.93147180369123816490e-1;
const LN2_LO = 1.90821492927058770002e-10;

const bits = new DataView(new ArrayBuffer(8));

/** 2^k for an integer k in [-1022, 1023], made from its bits. */
function pow2(k: number): number {
    bits.setUint32(0, (k + 1023) << 20);
    bits.setUint32(4, 0);
    return bits.getFloat64(0);
}

function ldexp(x: number, k: number): number {
    while (k > 1023) { x *= pow2(1023); k -= 1023; }
    while (k < -1022) { x *= pow2(-1022); k += 1022; }
    return x * pow2(k);
}

/** x > 0 finite as m * 2^e with m in [0.5, 1). */
function frexp(x: number): [number, number] {
    let e = 0;
    if (x < 2.2250738585072014e-308) { x *= 18014398509481984; e = -54; }   // subnormal: times 2^54
    bits.setFloat64(0, x);
    const hi = bits.getUint32(0);
    e += ((hi >>> 20) & 0x7ff) - 1022;
    bits.setUint32(0, ((hi & 0x800fffff) | (1022 << 20)) >>> 0);
    return [bits.getFloat64(0), e];
}

function negative(x: number): boolean { return x < 0 || Object.is(x, -0); }
function withSign(a: number, of: number): number { return negative(of) ? -Math.abs(a) : Math.abs(a); }

// sin and cos of r in [-pi/4, pi/4] (Taylor series to r^17 and r^18).
function sinPoly(r: number): number {
    const r2 = r * r;
    return r + r * r2 * (-1 / 6 + r2 * (1 / 120 + r2 * (-1 / 5040 + r2 * (1 / 362880 + r2 * (-1 / 39916800 + r2 * (1 / 6227020800 + r2 * (-1 / 1307674368000 + r2 * (1 / 355687428096000))))))));
}
function cosPoly(r: number): number {
    const r2 = r * r;
    return 1 + r2 * (-1 / 2 + r2 * (1 / 24 + r2 * (-1 / 720 + r2 * (1 / 40320 + r2 * (-1 / 3628800 + r2 * (1 / 479001600 + r2 * (-1 / 87178291200 + r2 * (1 / 20922789888000 + r2 * (-1 / 6402373705728000)))))))));
}

function sincos(x: number): [number, number] {
    if (!Number.isFinite(x)) return [NaN, NaN];
    const k = Math.floor(x * (2 / PI) + 0.5);
    const r = (x - k * HALF_PI_HI) - k * HALF_PI_LO;
    const quarter = ((k % 4) + 4) % 4;
    const s = sinPoly(r), c = cosPoly(r);
    if (quarter === 0) return [s, c];
    if (quarter === 1) return [c, -s];
    if (quarter === 2) return [-s, -c];
    return [-c, s];
}

// atan of x >= 0: past 1, pi/2 less atan(1/x); the angle halved three times, then its series.
function atanPos(x: number): number {
    let flip = false;
    if (x > 1) { x = 1 / x; flip = true; }
    let t = x / (1 + Math.sqrt(1 + x * x));
    t = t / (1 + Math.sqrt(1 + t * t));
    t = t / (1 + Math.sqrt(1 + t * t));
    const t2 = t * t;
    const a = 8 * t * (1 - t2 * (1 / 3 - t2 * (1 / 5 - t2 * (1 / 7 - t2 * (1 / 9 - t2 * (1 / 11 - t2 * (1 / 13 - t2 * (1 / 15 - t2 * (1 / 17 - t2 * (1 / 19))))))))));
    return flip ? HALF_PI - a : a;
}

function atan2(y: number, x: number): number {
    if (Number.isNaN(x) || Number.isNaN(y)) return NaN;
    if (y === 0) return negative(x) ? withSign(PI, y) : withSign(0, y);
    if (x === 0) return withSign(HALF_PI, y);
    if (!Number.isFinite(x) && !Number.isFinite(y)) return withSign(x > 0 ? PI / 4 : (3 * PI) / 4, y);
    const a = atanPos(Math.abs(y / x));
    return withSign(x > 0 ? a : PI - a, y);
}

function exp(x: number): number {
    if (Number.isNaN(x)) return NaN;
    if (x > 709.782712893384) return Infinity;
    if (x < -745.1332191019412) return 0;
    const k = Math.floor(x / LN2 + 0.5);
    const r = (x - k * LN2_HI) - k * LN2_LO;
    // e^r for |r| <= 0.35 by its series to r^17, from the smallest term up.
    let p = 1;
    for (let n = 17; n >= 1; n--) p = 1 + (p * r) / n;
    return ldexp(p, k);
}

function log(x: number): number {
    if (Number.isNaN(x) || x < 0) return NaN;
    if (x === 0) return -Infinity;
    if (x === Infinity) return Infinity;
    let [m, e] = frexp(x);
    if (m < 0.7071067811865476) { m *= 2; e -= 1; }
    const f = (m - 1) / (m + 1), f2 = f * f;
    let s = 1 / 31;
    for (let n = 29; n >= 1; n -= 2) s = 1 / n + f2 * s;
    return e * LN2_HI + (e * LN2_LO + 2 * f * s);
}

function pow(x: number, y: number): number {
    if (y === 0 || x === 1) return 1;
    if (Number.isNaN(x) || Number.isNaN(y)) return NaN;
    const integer = Number.isFinite(y) && Math.floor(y) === y;
    if (integer && Math.abs(y) <= 1024) {
        // A whole power by squaring: exact where the result fits (2 ** 3 is 8, not 7.999...).
        let n = Math.abs(y), base = x, r = 1;
        while (n > 0) {
            if (n % 2 === 1) r *= base;
            base *= base;
            n = Math.floor(n / 2);
        }
        return y < 0 ? 1 / r : r;
    }
    const odd = integer && Math.abs(y % 2) === 1;
    if (x === 0) {
        const r = y > 0 ? 0 : Infinity;
        return odd && Object.is(x, -0) ? -r : r;
    }
    if (x < 0) {
        if (!integer) return NaN;
        const r = exp(y * log(-x));
        return odd ? -r : r;
    }
    return exp(y * log(x));
}

export const repro = {
    PI,
    sin(x: number): number { return sincos(x)[0]; },
    cos(x: number): number { return sincos(x)[1]; },
    /** [sin x, cos x] at once. */
    sincos,
    tan(x: number): number { const [s, c] = sincos(x); return s / c; },
    atan(x: number): number { return Number.isNaN(x) ? NaN : withSign(atanPos(Math.abs(x)), x); },
    atan2,
    asin(x: number): number { return Math.abs(x) <= 1 ? atan2(x, Math.sqrt((1 - x) * (1 + x))) : NaN; },
    acos(x: number): number { return Math.abs(x) <= 1 ? atan2(Math.sqrt((1 - x) * (1 + x)), x) : NaN; },
    exp,
    log,
    pow,
    cbrt(x: number): number {
        const a = Math.abs(x);
        if (a === 0 || !Number.isFinite(a)) return x;
        let y = exp(log(a) / 3);
        y -= (y * y * y - a) / (3 * y * y);
        return withSign(y, x);
    },
    /** The length of a vector of any number of parts. */
    hypot(...v: number[]): number {
        let sum = 0;
        for (const x of v) {
            if (!Number.isFinite(x) && !Number.isNaN(x)) return Infinity;
            sum += x * x;
        }
        return Math.sqrt(sum);
    },
};
