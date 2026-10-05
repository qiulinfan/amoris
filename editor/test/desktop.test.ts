import { expect, test } from "bun:test";
import { registerCommands } from "../src/commands/registry";
import { installDesktop, markDesktopSceneSaved } from "../src/desktop";
import { useScripts } from "../src/state/scripts";
import { useSession } from "../src/state/session";
import type { Status } from "../src/host/protocol";

test("native close guard follows unsaved buffers and the edit world across Play, save, and undo", () => {
  const modified: boolean[] = [];
  let command: ((id: string) => void) | null = null;
  Object.defineProperty(globalThis, "window", { configurable: true, value: {
    amorisDesktop: {
      platform: "darwin",
      openProject: async () => {},
      setModified: (value: boolean) => modified.push(value),
      onCommand: (listener: (id: string) => void) => { command = listener; return () => {}; },
    },
  } });
  const status = (hash: string, mode: "edit" | "play" = "edit"): Status => ({
    world_hash: hash, mode, tick: 0, t_s: 0, paused: true, pacing: "paused", entities: 1,
  });
  installDesktop();
  useSession.getState().setStatus(status("opened"));
  expect(modified).toEqual([false]);
  useScripts.getState().setDirty("scripts/main.ts", true);
  // Repeated status pushes do not flood native IPC or clear an unsaved script.
  useSession.getState().setStatus(status("opened"));
  markDesktopSceneSaved("opened");
  expect(modified).toEqual([false, true]);
  useScripts.getState().setDirty("scripts/main.ts", false);
  expect(modified.at(-1)).toBe(false);
  useSession.getState().setStatus(status("edited"));
  useSession.getState().setStatus(status("running-fork", "play"));
  expect(modified.at(-1)).toBe(true);
  markDesktopSceneSaved("edited");
  expect(modified.at(-1)).toBe(false);
  useSession.getState().setStatus(status("concurrent-edit"));
  // A save started before another client edited the world cannot mark that newer edit saved.
  markDesktopSceneSaved("edited");
  expect(modified.at(-1)).toBe(true);
  useSession.getState().setStatus(status("edited"));
  expect(modified.at(-1)).toBe(false);
  let ran = 0;
  registerCommands([
    { id: "test.native", title: "Native", category: "File", run: () => { ran++; } },
    { id: "test.disabled", title: "Disabled", category: "File", enabled: () => false, run: () => { ran++; } },
  ]);
  const send = command as unknown as (id: string) => void;
  send("test.native");
  send("test.disabled");
  send("missing.command");
  expect(ran).toBe(1);
});
