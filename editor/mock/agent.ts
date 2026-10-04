// A simulated MCP agent for the mock: every call an agent makes is mirrored to editors as an `agent`
// event (host-protocol.md section 7), and its writes go through `world.edit` like the editor's, so
// they land in the same undo history. The first session inspects the scene and moves the crate that
// floats out of reach; later sessions only observe.

import type { Session } from "./session";
import { MockError } from "./util";

type Step = { delay: number; run: () => void };

export class MockAgent {
  private timers: ReturnType<typeof setTimeout>[] = [];
  private round = 0;

  constructor(
    private readonly session: Session,
    private readonly broadcast: (topic: string, data: unknown) => void,
  ) {}

  start(firstDelayMs = 6000) {
    this.schedule(firstDelayMs);
  }

  stop() {
    this.timers.forEach(clearTimeout);
    this.timers = [];
  }

  private schedule(delay: number) {
    this.timers.push(setTimeout(() => this.runRound(), delay));
  }

  private emit(session: string, kind: "call" | "result" | "message", summary: string, method?: string, ok = true) {
    this.broadcast("agent", { session, kind, method, summary, ok, ts: Date.now() });
  }

  private call(session: string, method: string, params: Record<string, unknown>, describe: (r: unknown) => string, note: string) {
    this.emit(session, "call", note, method);
    try {
      const r = this.session.call(method, params);
      this.emit(session, "result", describe(r), method);
      return r;
    } catch (e) {
      const msg = e instanceof MockError ? `${e.code}: ${e.message}` : String(e);
      this.emit(session, "result", msg, method, false);
      return undefined;
    }
  }

  private runRound() {
    const first = this.round === 0;
    const session = first ? "claude-opus · a7f3" : `claude-opus · ${(0xb000 + this.round * 977).toString(16).slice(-4)}`;
    this.round++;
    const s = this.session;
    const steps: Step[] = [];
    const add = (delay: number, run: () => void) => steps.push({ delay, run });
    add(0, () => this.emit(session, "message", first ? "Connected over MCP (developer tools). Goal: make every crate reachable on the sloop's course." : "Checking on the voyage."));
    add(700, () =>
      this.call(session, "world.tree", {}, (r) => `${countTree(r as TreeLike[])} entities; roots ${(r as TreeLike[]).slice(0, 4).map((n) => n.name).join(", ")}…`, "list the scene"),
    );
    if (first) {
      add(1500, () =>
        this.call(session, "world.query", { with: ["Cargo", "Transform"], fields: ["Transform.position", "Cargo.value"] }, (r) => {
          const rows = r as { name: string; "Transform.position": number[] }[];
          return rows.map((x) => `${x.name} (${x["Transform.position"].map((v) => v.toFixed(1)).join(", ")})`).join("; ");
        }, "where are the crates?"),
      );
      add(2400, () => this.emit(session, "message", "Crate4 floats 6 m off the course and REACH is 3 m, so the crew can never take it. Moving it onto the line."));
      add(3300, () => {
        if (s.mode !== "edit") {
          this.emit(session, "message", "Play is running; I will not edit the world now.");
          return;
        }
        const crate = [...s.world.entities.values()].find((e) => e.name === "Crate4");
        if (!crate) return;
        this.call(
          session,
          "world.edit",
          { ops: [{ set: { entity: crate.id, component: "Transform", value: { position: [26, 0, -2.5] } } }], label: "agent: move Crate4 within reach" },
          (r) => `applied ${(r as { applied: number }).applied} op at tick ${(r as { tick: number }).tick}; undoable from History`,
          "move Crate4 to (26, 0, -2.5)",
        );
      });
      add(4300, () => this.emit(session, "message", "Done: all four crates now lie within 3 m of the sloop's line. Undo it from History to restore the original layout."));
    } else {
      add(1400, () => this.call(session, "events.since", { seq: Math.max(0, s.seq - 5) }, (r) => `${(r as unknown[]).length} recent events`, "what happened lately?"));
      add(2200, () =>
        this.call(session, "world.query", { with: ["Boat"], fields: ["Boat.speed", "Boat.heading_deg"] }, (r) => {
          const row = (r as Record<string, number>[])[0];
          return row ? `Sloop ${Number(row["Boat.speed"]).toFixed(2)} m/s heading ${Number(row["Boat.heading_deg"]).toFixed(0)}°` : "no boat";
        }, "how is the sloop doing?"),
      );
      add(3000, () => this.emit(session, "message", s.mode === "play" ? `Tick ${s.tick}: the voyage is under way.` : "The world is in Edit mode; nothing to report."));
    }
    let at = 0;
    for (const step of steps) {
      at = step.delay;
      this.timers.push(setTimeout(step.run, at));
    }
    this.schedule(at + 30000);
  }
}

interface TreeLike {
  name: string;
  children: TreeLike[];
}

function countTree(nodes: TreeLike[]): number {
  return nodes.reduce((n, x) => n + 1 + countTree(x.children), 0);
}
