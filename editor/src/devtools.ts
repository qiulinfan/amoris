// Development aid: the stores, the host client and the dock on `window.amoris`, for inspection
// from DevTools and for the evidence capture (tools/capture.ts). Only in `vite` dev builds, and in
// a production build made with VITE_AMORIS_EXPOSE=1 (tools/gizmo-drag.ts times one).

import { api, host } from "./host/api";
import { openPanel, resetLayout, toggleMaximize } from "./layout/dock";
import { runCommand } from "./commands/registry";
import { useConnection } from "./state/connection";
import { useDebug } from "./state/debug";
import { useScripts } from "./state/scripts";
import { useSelection } from "./state/selection";
import { useSession } from "./state/session";
import { useUi } from "./state/ui";
import { useViewport } from "./state/viewport";
import { useWorld } from "./state/world";
import { useEvents } from "./state/events";
import * as debugActions from "./actions/debug";
import * as worldActions from "./actions/world";

export function expose() {
  (window as unknown as { amoris: unknown }).amoris = {
    api,
    host,
    openPanel,
    resetLayout,
    toggleMaximize,
    runCommand,
    world: useWorld,
    selection: useSelection,
    connection: useConnection,
    viewport: useViewport,
    session: useSession,
    debug: useDebug,
    scripts: useScripts,
    events: useEvents,
    ui: useUi,
    actions: { debug: debugActions, world: worldActions },
  };
}
