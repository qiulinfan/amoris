// Low-poly stand-ins for the fallback view: primitives, a sloop and a crate, in local space
// (metres, -z forward, +y up). Faces are convex polygons; each may carry its own colour.

import type { Vec3 } from "../math";

export interface Face {
  idx: number[];
  color?: Vec3;
  doubleSided?: boolean;
}

export interface Mesh {
  verts: Vec3[];
  faces: Face[];
}

function centroid(verts: Vec3[], idx: number[]): Vec3 {
  const c: Vec3 = [0, 0, 0];
  for (const i of idx) for (let k = 0; k < 3; k++) c[k] += verts[i]![k]! / idx.length;
  return c;
}

/** Winds every face counter-clockwise seen from outside: its normal points away from `interior`. */
function orient(m: Mesh, interior: (face: Face) => Vec3 = () => [0, 0, 0], from = 0): Mesh {
  for (const f of m.faces.slice(from)) {
    if (f.doubleSided) continue;
    const [a, b, c] = f.idx.slice(0, 3).map((i) => m.verts[i]!) as [Vec3, Vec3, Vec3];
    const e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    const e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    const n = [e1[1]! * e2[2]! - e1[2]! * e2[1]!, e1[2]! * e2[0]! - e1[0]! * e2[2]!, e1[0]! * e2[1]! - e1[1]! * e2[0]!];
    const ce = centroid(m.verts, f.idx);
    const inside = interior(f);
    const out = [ce[0] - inside[0], ce[1] - inside[1], ce[2] - inside[2]];
    if (n[0]! * out[0]! + n[1]! * out[1]! + n[2]! * out[2]! < 0) f.idx.reverse();
  }
  return m;
}

function box(): Mesh {
  const verts: Vec3[] = [];
  for (const x of [-0.5, 0.5]) for (const y of [-0.5, 0.5]) for (const z of [-0.5, 0.5]) verts.push([x, y, z]);
  // index = xi*4 + yi*2 + zi
  return orient({
    verts,
    faces: [
      { idx: [4, 5, 7, 6] }, // +x
      { idx: [0, 2, 3, 1] }, // -x
      { idx: [2, 6, 7, 3] }, // +y
      { idx: [0, 1, 5, 4] }, // -y
      { idx: [1, 3, 7, 5] }, // +z
      { idx: [0, 4, 6, 2] }, // -z
    ],
  });
}

function crate(): Mesh {
  const m = box();
  // A darker band of planks around the middle reads as a crate at a glance.
  const base = m.verts.length;
  const band = 0.505;
  const bandVerts: Vec3[] = [];
  for (const y of [-0.12, 0.12]) {
    bandVerts.push([band, y, band], [band, y, -band], [-band, y, -band], [-band, y, band]);
  }
  m.verts.push(...bandVerts);
  const dark: Vec3 = [0.55, 0.36, 0.2];
  m.faces.push(
    { idx: [base + 0, base + 1, base + 5, base + 4], color: dark },
    { idx: [base + 1, base + 2, base + 6, base + 5], color: dark },
    { idx: [base + 2, base + 3, base + 7, base + 6], color: dark },
    { idx: [base + 3, base + 0, base + 4, base + 7], color: dark },
  );
  return orient(m);
}

function sphere(rings = 7, segments = 12): Mesh {
  const verts: Vec3[] = [[0, 0.5, 0]];
  for (let r = 1; r < rings; r++) {
    const phi = (r / rings) * Math.PI;
    for (let s = 0; s < segments; s++) {
      const th = (s / segments) * Math.PI * 2;
      verts.push([0.5 * Math.sin(phi) * Math.cos(th), 0.5 * Math.cos(phi), 0.5 * Math.sin(phi) * Math.sin(th)]);
    }
  }
  verts.push([0, -0.5, 0]);
  const faces: Face[] = [];
  const ring = (r: number, s: number) => 1 + (r - 1) * segments + (s % segments);
  for (let s = 0; s < segments; s++) faces.push({ idx: [0, ring(1, s + 1), ring(1, s)] });
  for (let r = 1; r < rings - 1; r++) {
    for (let s = 0; s < segments; s++) faces.push({ idx: [ring(r, s), ring(r, s + 1), ring(r + 1, s + 1), ring(r + 1, s)] });
  }
  const last = verts.length - 1;
  for (let s = 0; s < segments; s++) faces.push({ idx: [last, ring(rings - 1, s), ring(rings - 1, s + 1)] });
  return orient({ verts, faces });
}

function cylinder(segments = 14): Mesh {
  const verts: Vec3[] = [];
  for (const y of [0.5, -0.5]) {
    for (let s = 0; s < segments; s++) {
      const th = (s / segments) * Math.PI * 2;
      verts.push([0.5 * Math.cos(th), y, 0.5 * Math.sin(th)]);
    }
  }
  const faces: Face[] = [];
  for (let s = 0; s < segments; s++) {
    const n = (s + 1) % segments;
    faces.push({ idx: [s, n, segments + n, segments + s] });
  }
  faces.push({ idx: Array.from({ length: segments }, (_, s) => segments - 1 - s) });
  faces.push({ idx: Array.from({ length: segments }, (_, s) => segments + s) });
  return orient({ verts, faces });
}

function plane(): Mesh {
  return {
    verts: [
      [-0.5, 0, -0.5],
      [0.5, 0, -0.5],
      [0.5, 0, 0.5],
      [-0.5, 0, 0.5],
    ],
    faces: [{ idx: [3, 2, 1, 0], doubleSided: true }],
  };
}

/** A sloop about 6.4 m long, bow toward -z: hull, deck, mast and a mainsail on its boom. */
export function sloop(boomDeg = 0, hoist = 1): Mesh {
  const deck: [number, number][] = [
    [0, -3.3],
    [0.75, -2.0],
    [1.0, -0.4],
    [1.0, 1.4],
    [0.82, 3.0],
    [-0.82, 3.0],
    [-1.0, 1.4],
    [-1.0, -0.4],
    [-0.75, -2.0],
  ];
  const verts: Vec3[] = [];
  for (const [x, z] of deck) verts.push([x, 0.55, z]);
  for (const [x, z] of deck) verts.push([x * 0.32, -0.45, z * 0.9]);
  const n = deck.length;
  const hull: Vec3 = [0.12, 0.2, 0.36];
  const deckColor: Vec3 = [0.72, 0.6, 0.45];
  const faces: Face[] = [{ idx: Array.from({ length: n }, (_, i) => i), color: deckColor }];
  faces.push({ idx: Array.from({ length: n }, (_, i) => n + i), color: hull });
  for (let i = 0; i < n; i++) {
    const j = (i + 1) % n;
    faces.push({ idx: [i, j, n + j, n + i], color: hull });
  }
  orient({ verts, faces }, () => [0, 0.05, 0]);
  // Mast: a thin box.
  const mastBase = verts.length;
  const mx = 0;
  const mz = -0.6;
  const w = 0.06;
  for (const x of [-w, w]) for (const y of [0.55, 7.2]) for (const z of [-w, w]) verts.push([mx + x, y, mz + z]);
  const mast: Vec3 = [0.78, 0.68, 0.52];
  const b = mastBase;
  faces.push(
    { idx: [b + 4, b + 5, b + 7, b + 6], color: mast },
    { idx: [b + 0, b + 2, b + 3, b + 1], color: mast },
    { idx: [b + 1, b + 3, b + 7, b + 5], color: mast },
    { idx: [b + 0, b + 4, b + 6, b + 2], color: mast },
  );
  const mastFrom = faces.length - 4;
  orient({ verts, faces }, (f) => [mx, centroid(verts, f.idx)[1], mz], mastFrom);
  // Mainsail: a triangle from the mast up to the head and back along the boom, swung by the boom.
  if (hoist > 0.05) {
    const a = (boomDeg * Math.PI) / 180;
    const boom = 3.3;
    const head = 0.9 + 6.1 * hoist;
    const sb = verts.length;
    verts.push([mx, 0.95, mz], [mx, head, mz], [mx + Math.sin(a) * boom, 0.95, mz + Math.cos(a) * boom]);
    faces.push({ idx: [sb, sb + 1, sb + 2], color: [0.95, 0.94, 0.9], doubleSided: true });
    // Jib.
    const jb = verts.length;
    verts.push([0, 0.75, -3.2], [mx, head * 0.86, mz], [Math.sin(a) * 1.1, 0.8, mz - 0.4]);
    faces.push({ idx: [jb, jb + 1, jb + 2], color: [0.9, 0.9, 0.86], doubleSided: true });
  }
  return { verts, faces };
}

const LIBRARY: Record<string, Mesh> = {
  box: box(),
  crate: crate(),
  sphere: sphere(),
  cylinder: cylinder(),
  plane: plane(),
};

/** The stand-in for a Model's mesh path or primitive name. */
export function meshFor(mesh: string): { name: string; mesh: Mesh } {
  const m = mesh.toLowerCase();
  if (/sloop|boat|ship/.test(m)) return { name: "sloop", mesh: sloop() };
  if (/crate/.test(m)) return { name: "crate", mesh: LIBRARY.crate! };
  if (/sphere|buoy|ball/.test(m)) return { name: "sphere", mesh: LIBRARY.sphere! };
  if (/cylinder|pier|island/.test(m)) return { name: "cylinder", mesh: LIBRARY.cylinder! };
  if (/plane|quad/.test(m)) return { name: "plane", mesh: LIBRARY.plane! };
  return { name: "box", mesh: LIBRARY.box! };
}

export function primitive(name: "box" | "sphere" | "cylinder" | "plane" | "crate"): Mesh {
  return LIBRARY[name]!;
}
