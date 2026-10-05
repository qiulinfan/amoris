// The native shell exposes only project selection and editor menu commands. World edits keep
// using the host protocol. Report unsaved buffers and edit-world changes before closing a host.
import { runCommand } from "./commands/registry";
import { useScripts } from "./state/scripts";
import { useSession } from "./state/session";

interface DesktopBridge {
  readonly platform: string;
  openProject(): Promise<void>;
  onCommand(listener: (id: string) => void): () => void;
  setModified(modified: boolean): void;
}

declare global {
  interface Window {
    amorisDesktop?: DesktopBridge;
  }
}

let savedEditHash: string | null = null;
let editHash: string | null = null;
let reportedModified: boolean | null = null;

function reportModified() {
  const scriptsDirty = Object.values(useScripts.getState().dirty).some(Boolean);
  const modified = scriptsDirty || editHash !== savedEditHash;
  if (modified !== reportedModified) {
    window.amorisDesktop?.setModified(modified);
    reportedModified = modified;
  }
}

export function installDesktop() {
  const desktop = window.amorisDesktop;
  if (!desktop) return;
  desktop.onCommand(runCommand);
  useScripts.subscribe(reportModified);
  useSession.subscribe(({ status }) => {
    if (status?.mode === "edit") {
      editHash = status.world_hash;
      savedEditHash ??= editHash;
    }
    reportModified();
  });
}

/** Called after a successful save and a fresh host status, including saves during Play. */
export function markDesktopSceneSaved(hash: string | null) {
  savedEditHash = hash;
  reportModified();
}

export function desktopEditHash(): string | null {
  return editHash;
}
