import { Box, Camera, ChevronDown, Focus, Globe, Grid3x3, Magnet, MousePointer2, Move3d, Rotate3d, Scale3d } from "lucide-react";
import { useState } from "react";
import { useViewport, type Tool } from "../../state/viewport";
import { IconButton } from "../../ui/Button";
import { PopupMenu } from "../../ui/Menu";
import type { MenuEntry } from "../../state/ui";
import type { ViewportController } from "./controller";

const TOOLS: { tool: Tool; icon: typeof Move3d; label: string; keys: string }[] = [
  { tool: "select", icon: MousePointer2, label: "Select", keys: "Q" },
  { tool: "translate", icon: Move3d, label: "Move", keys: "W" },
  { tool: "rotate", icon: Rotate3d, label: "Rotate", keys: "E" },
  { tool: "scale", icon: Scale3d, label: "Scale", keys: "R" },
];

const STEPS = {
  translate: [0.1, 0.25, 0.5, 1, 2, 5],
  rotate: [5, 10, 15, 30, 45, 90],
  scale: [0.05, 0.1, 0.25, 0.5],
};

export function ViewportToolbar({ ctl }: { ctl: ViewportController }) {
  const { tool, space, snap, grid, set, setSnap } = useViewport();
  const [menu, setMenu] = useState<{ x: number; y: number; items: MenuEntry[] } | null>(null);
  const open = (e: React.MouseEvent, items: MenuEntry[]) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    setMenu({ x: r.left, y: r.bottom + 4, items });
  };
  const snapMenu: MenuEntry[] = [
    { label: "Snap while dragging", checked: snap.enabled, run: () => setSnap({ enabled: !snap.enabled }) },
    "separator",
    { header: "Move step (m)" },
    ...STEPS.translate.map((v) => ({ label: String(v), checked: snap.translate === v, run: () => setSnap({ translate: v, enabled: true }) })),
    { header: "Rotate step (°)" },
    ...STEPS.rotate.map((v) => ({ label: `${v}°`, checked: snap.rotate === v, run: () => setSnap({ rotate: v, enabled: true }) })),
    { header: "Scale step" },
    ...STEPS.scale.map((v) => ({ label: String(v), checked: snap.scale === v, run: () => setSnap({ scale: v, enabled: true }) })),
  ];
  const cameraMenu: MenuEntry[] = [
    { label: "Perspective", checked: !ctl.orbit.ortho, run: () => ctl.setOrbit({ ortho: false }) },
    { label: "Orthographic", checked: ctl.orbit.ortho, run: () => ctl.setOrbit({ ortho: true }) },
    "separator",
    { label: "Top", run: () => ctl.look("+y") },
    { label: "Bottom", run: () => ctl.look("-y") },
    { label: "Front", run: () => ctl.look("+z") },
    { label: "Back", run: () => ctl.look("-z") },
    { label: "Right", run: () => ctl.look("+x") },
    { label: "Left", run: () => ctl.look("-x") },
    "separator",
    { label: "Frame Selection", keys: "F", run: () => ctl.frameSelection() },
    { label: "Reset Camera", run: () => ctl.setOrbit({ target: [8, 0, 0], yaw: -38 * (Math.PI / 180), pitch: 24 * (Math.PI / 180), distance: 30, ortho: false }, 300) },
  ];
  return (
    <>
      <div className="vp-bar vp-bar-left" onPointerDown={(e) => e.stopPropagation()}>
        <div className="seg">
          {TOOLS.map((t) => (
            <IconButton key={t.tool} icon={t.icon} label={t.label} keys={t.keys} active={tool === t.tool} onClick={() => set({ tool: t.tool })} />
          ))}
        </div>
        <div className="seg">
          <button
            type="button"
            className="vp-text-btn"
            data-tip={`Gizmo space  X`}
            onClick={() => set({ space: space === "world" ? "local" : "world" })}
          >
            {space === "world" ? <Globe size={14} /> : <Box size={14} />}
            <span>{space === "world" ? "World" : "Local"}</span>
          </button>
        </div>
        <div className="seg">
          <IconButton icon={Magnet} label={snap.enabled ? `Snap on (${snap.translate} m, ${snap.rotate}°)` : "Snap off"} active={snap.enabled} onClick={() => setSnap({ enabled: !snap.enabled })} />
          <button type="button" className="vp-chev" aria-label="Snap settings" onClick={(e) => open(e, snapMenu)}>
            <ChevronDown size={12} />
          </button>
          <IconButton icon={Grid3x3} label="Grid" keys="G" active={grid} onClick={() => set({ grid: !grid })} />
        </div>
      </div>
      <div className="vp-bar vp-bar-right" onPointerDown={(e) => e.stopPropagation()}>
        <div className="seg">
          <IconButton icon={Focus} label="Frame selection" keys="F" onClick={() => ctl.frameSelection()} />
          <button type="button" className="vp-text-btn" onClick={(e) => open(e, cameraMenu)}>
            <Camera size={14} />
            <span>{ctl.orbit.ortho ? "Ortho" : "Persp"}</span>
            <ChevronDown size={12} />
          </button>
        </div>
      </div>
      {menu && <PopupMenu items={menu.items} x={menu.x} y={menu.y} onClose={() => setMenu(null)} />}
    </>
  );
}
