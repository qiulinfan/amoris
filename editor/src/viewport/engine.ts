// The viewport's renderer interface. The engine's renderer, compiled to wasm with wasm-bindgen
// (crates/pocket-web), implements `Viewport` and exports `createViewport`; the host serves the
// wasm-bindgen output at `/wasm/pocket_web.js`. The renderer draws the host's world from the render
// feed (`renderUrl`, host-protocol.md section 5) and assets (`assetsUrl`); the editor owns the camera,
// the selection and the gizmo and only tells the renderer about them. Until the wasm exists (or when
// it fails to load), the editor draws with its fallback (fallback/renderer.ts), which implements the
// same interface from the world data the editor already has.
//
// Conventions shared with the renderer: right-handed, +y up, metres, quaternions [x, y, z, w],
// colours sRGB 0..1 for overlays, and pixel coordinates in CSS pixels from the canvas's top left.

import type { Quat, Vec3 } from "../host/protocol";
import type { Camera } from "./camera";

export type { Camera };

export interface ViewportOptions {
  /** WebSocket URL of the render feed, e.g. ws://127.0.0.1:7878/render. */
  renderUrl: string;
  /** Base URL of project assets, e.g. http://127.0.0.1:7878/assets/. */
  assetsUrl: string;
}

export interface PickHit {
  entity: number;
  /** The world-space point under the pixel, when the renderer knows it (depth buffer). */
  position?: Vec3;
}

export type GizmoMode = "translate" | "rotate" | "scale";
export type GizmoHandle = "x" | "y" | "z" | "xy" | "yz" | "xz" | "xyz";
export type Rgba = [number, number, number, number];

/** A world-space shape of the gizmo. The renderer draws them on top of the scene, in order. */
export interface GizmoShape {
  kind: "line" | "polygon";
  points: Vec3[];
  color: Rgba;
  /** Line width in CSS pixels (lines and polygon outlines). */
  width_px?: number;
  /** Polygons are filled with `color`; set to also draw the outline. */
  outline?: Rgba;
  handle?: GizmoHandle;
}

export interface Gizmo {
  mode: GizmoMode;
  space: "world" | "local";
  position: Vec3;
  rotation: Quat;
  /** Handle length in CSS pixels; the shapes are already sized for the current camera. */
  size_px: number;
  hovered: GizmoHandle | null;
  active: GizmoHandle | null;
  /** Ready-to-draw geometry (gizmo.ts builds it; the editor hit-tests the same shapes). */
  shapes: GizmoShape[];
}

export interface Overlays {
  grid: boolean;
  /** World axes through the origin. */
  axes: boolean;
}

export interface Viewport {
  setCamera(camera: Camera): void;
  /** The entity under a pixel (CSS pixels from the canvas's top left), or null. */
  pick(x: number, y: number): Promise<PickHit | null>;
  /** Canvas size in CSS pixels and the device pixel ratio; the renderer sizes its backbuffer. */
  resize(width: number, height: number, devicePixelRatio: number): void;
  setGizmo(gizmo: Gizmo | null): void;
  /** Selected entities (outlined) and the hovered one (highlighted). */
  setSelection(entities: readonly number[], hovered: number | null): void;
  setOverlays(overlays: Overlays): void;
  dispose(): void;
}

/** What `/wasm/pocket_web.js` exports. `default` is wasm-bindgen's init and runs first when present. */
export interface ViewportModule {
  default?: (input?: unknown) => Promise<unknown>;
  createViewport(canvas: HTMLCanvasElement, options: ViewportOptions): Viewport | Promise<Viewport>;
}

export const WASM_MODULE_URL = "/wasm/pocket_web.js";

export type ViewportKind = "wasm" | "fallback";

export function viewportOptions(httpBase: string, wsUrl: string): ViewportOptions {
  const origin = httpBase || window.location.origin;
  return { renderUrl: wsUrl.replace(/\/ws$/, "/render"), assetsUrl: `${origin}/assets/` };
}

/**
 * Loads the wasm renderer unless `?viewport=fallback`; `?viewport=wasm` refuses the fallback (to
 * test the integration). Resolves with the renderer and which one it is.
 */
export async function loadViewport(
  canvas: HTMLCanvasElement,
  options: ViewportOptions,
  fallback: () => Viewport,
): Promise<{ viewport: Viewport; kind: ViewportKind; note?: string }> {
  const want = new URLSearchParams(window.location.search).get("viewport");
  if (want === "fallback") return { viewport: fallback(), kind: "fallback", note: "forced by ?viewport=fallback" };
  const base = options.assetsUrl.replace(/\/assets\/$/, "");
  const url = `${base}${WASM_MODULE_URL}`;
  try {
    const head = await fetch(url, { method: "HEAD" });
    if (!head.ok || !/javascript|ecmascript/.test(head.headers.get("content-type") ?? "")) throw new Error(`${WASM_MODULE_URL}: HTTP ${head.status}`);
    const mod = (await import(/* @vite-ignore */ url)) as ViewportModule;
    if (mod.default) await mod.default();
    if (typeof mod.createViewport !== "function") throw new Error(`${WASM_MODULE_URL} exports no createViewport`);
    return { viewport: await mod.createViewport(canvas, options), kind: "wasm" };
  } catch (e) {
    const note = e instanceof Error ? e.message : String(e);
    if (want === "wasm") throw e;
    return { viewport: fallback(), kind: "fallback", note };
  }
}
