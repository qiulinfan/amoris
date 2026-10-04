// The viewport panel: a canvas the renderer draws into (the engine's wasm renderer, or the fallback
// until it is served), the controller's input handling, and overlays: tool bar, axis gizmo, stats,
// the wind, the selection rectangle and asset drops.

import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { host } from "../../host/api";
import { spawnAsset } from "../../actions/world";
import { ASSET_MIME } from "../../state/assets";
import { useConnection } from "../../state/connection";
import { useSelection } from "../../state/selection";
import { useSession } from "../../state/session";
import { useViewport } from "../../state/viewport";
import { useWorld } from "../../state/world";
import { logLocal } from "../../state/logs";
import { loadViewport, viewportOptions } from "../../viewport/engine";
import { createFallbackViewport, type FallbackEntity } from "../../viewport/fallback/renderer";
import { useUi } from "../../state/ui";
import { entityMenu } from "../hierarchy/entityMenu";
import { ViewportController } from "./controller";
import { AxisGizmo } from "./AxisGizmo";
import { ViewportToolbar } from "./ViewportToolbar";
import { ViewportStats } from "./ViewportStats";
import "./viewport.css";

const source = {
  *entities(): Iterable<FallbackEntity> {
    const s = useWorld.getState();
    for (const id of s.order) {
      const e = s.entities[id];
      if (!e) continue;
      const p = s.preview[id];
      yield p ? { ...e, components: { ...e.components, ...p } } : e;
    }
  },
  time: () => ViewportController.clock(),
  smooth: () => useSession.getState().status?.mode === "play",
};

export function ViewportPanel() {
  const wrap = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [ctl] = useState(() => new ViewportController());
  const [dropping, setDropping] = useState(false);
  const [cursor, setCursor] = useState("default");
  const version = useSyncExternalStore(
    (fn) => ctl.subscribe(fn),
    () => `${ctl.orbit.yaw}|${ctl.orbit.pitch}|${ctl.orbit.ortho}|${ctl.rect ? `${ctl.rect.x},${ctl.rect.y},${ctl.rect.w},${ctl.rect.h}` : ""}`,
  );

  // Renderer lifecycle.
  useEffect(() => {
    const el = canvas.current!;
    let disposed = false;
    ctl.start();
    if (import.meta.env.DEV) (window as unknown as { pocketEditorViewport: ViewportController }).pocketEditorViewport = ctl;
    void loadViewport(el, viewportOptions(host.endpoints.http, host.endpoints.ws), () => createFallbackViewport(el, source)).then(
      ({ viewport, kind, note }) => {
        if (disposed) {
          viewport.dispose();
          return;
        }
        ctl.attach(viewport);
        useConnection.getState().set({ viewport: kind });
        if (kind === "fallback") logLocal("debug", `Viewport: fallback renderer (${note ?? "no wasm"})`, { source: "viewport" });
      },
      (e: unknown) => logLocal("error", `Viewport: ${e instanceof Error ? e.message : String(e)}`, { source: "viewport" }),
    );
    return () => {
      disposed = true;
      ctl.stop();
      ctl.engine?.dispose();
      ctl.engine = null;
    };
  }, [ctl]);

  // Size.
  useEffect(() => {
    const el = wrap.current!;
    const ro = new ResizeObserver(() => ctl.resize(el.clientWidth, el.clientHeight));
    ro.observe(el);
    ctl.resize(el.clientWidth, el.clientHeight);
    return () => ro.disconnect();
  }, [ctl]);

  // Selection, overlays and camera requests from the rest of the editor.
  useEffect(() => {
    const a = useSelection.subscribe((s) => ctl.engine?.setSelection(s.ids, s.hovered));
    const b = useViewport.subscribe((s, prev) => {
      if (s.grid !== prev.grid) ctl.engine?.setOverlays({ grid: s.grid, axes: true });
      if (s.frameNonce !== prev.frameNonce) ctl.frameSelection();
      if (s.view && s.view.nonce !== prev.view?.nonce) ctl.look(s.view.dir);
    });
    return () => {
      a();
      b();
    };
  }, [ctl]);

  const local = (e: { clientX: number; clientY: number }) => {
    const r = wrap.current!.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top] as const;
  };

  useEffect(() => {
    const el = wrap.current!;
    const wheel = (e: WheelEvent) => {
      e.preventDefault();
      ctl.wheel(e);
    };
    el.addEventListener("wheel", wheel, { passive: false });
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape" && ctl.cancel()) {
        e.preventDefault();
        e.stopPropagation();
      }
    };
    window.addEventListener("keydown", key, true);
    return () => {
      el.removeEventListener("wheel", wheel);
      window.removeEventListener("keydown", key, true);
    };
  }, [ctl]);

  const rect = ctl.rect;
  void version;

  return (
    <div className="viewport">
      <div
        ref={wrap}
        className={`viewport-surface ${dropping ? "is-dropping" : ""}`}
        style={{ cursor }}
        tabIndex={0}
        onPointerDown={(e) => {
          wrap.current!.focus();
          (e.target as HTMLElement).setPointerCapture(e.pointerId);
          const [x, y] = local(e);
          ctl.pointerDown(e.nativeEvent, x, y);
          setCursor(ctl.cursor);
        }}
        onPointerMove={(e) => {
          const [x, y] = local(e);
          ctl.pointerMove(e.nativeEvent, x, y);
          const c = ctl.cursor;
          if (c !== cursor) setCursor(c);
        }}
        onPointerUp={(e) => {
          const [x, y] = local(e);
          const ev = e.nativeEvent;
          void ctl.pointerUp(ev, x, y).then(async ({ contextMenu: open }) => {
            setCursor(ctl.cursor);
            if (!open) return;
            const hit = await ctl.engine?.pick(x, y);
            const sel = useSelection.getState();
            if (hit && !sel.ids.includes(hit.entity)) sel.set([hit.entity]);
            const ids = hit ? useSelection.getState().ids : [];
            useUi.getState().openContextMenu(ev.clientX, ev.clientY, entityMenu(ids, { at: ctl.groundAt(x, y) }));
          });
        }}
        onPointerLeave={() => ctl.leave()}
        onContextMenu={(e) => e.preventDefault()}
        onDoubleClick={(e) => {
          const [x, y] = local(e);
          void ctl.engine?.pick(x, y).then((hit) => {
            if (hit) {
              useSelection.getState().set([hit.entity]);
              ctl.frameSelection();
            }
          });
        }}
        onDragOver={(e) => {
          if (e.dataTransfer.types.includes(ASSET_MIME)) {
            e.preventDefault();
            e.dataTransfer.dropEffect = "copy";
            setDropping(true);
          }
        }}
        onDragLeave={() => setDropping(false)}
        onDrop={(e) => {
          setDropping(false);
          const path = e.dataTransfer.getData(ASSET_MIME);
          if (!path) return;
          e.preventDefault();
          const [x, y] = local(e);
          void spawnAsset(path, ctl.groundAt(x, y));
        }}
      >
        <canvas ref={canvas} className="viewport-canvas" />
        {rect && <div className="select-rect" style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }} />}
      </div>
      <ViewportToolbar ctl={ctl} />
      <AxisGizmo ctl={ctl} />
      <ViewportStats />
    </div>
  );
}
