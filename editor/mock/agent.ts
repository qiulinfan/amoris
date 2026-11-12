// A simulated MCP agent for the mock: each round is an agent session of the mock host (opened,
// listed and closed as the real host's are, server.md 3.5), every call it makes is recorded and
// pushed to editors as an `agent` event, and its writes go through `world.edit` like the editor's,
// so they land in the same undo history under the session's name. The first session inspects the
// scene and moves the crate that floats out of reach; later sessions only observe.

import type { Session } from "./session";
import { MockError } from "./util";

type Step = { delay: number; run: () => void };

export class MockAgent {
  private timers: ReturnType<typeof setTimeout>[] = [];
  private round = 0;

  constructor(private readonly session: Session) {}

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

  private emit(session: string, summary: string) {
    this.session.agents.message(session, summary);
  }

  private call(session: string, method: string, params: Record<string, unknown>, describe: (r: unknown) => string, note: string) {
    const agents = this.session.agents;
    agents.call(session, method, note);
    try {
      // Each round is one MCP session: a developer client of its own, named as its row is.
      const r = this.session.call(method, params, { source: { developer: this.round }, session });
      agents.result(session, method, true, describe(r));
      return r;
    } catch (e) {
      const msg = e instanceof MockError ? `${e.code}: ${e.message}` : String(e);
      agents.result(session, method, false, msg);
      return undefined;
    }
  }

  private runRound() {
    const first = this.round === 0;
    this.round++;
    const s = this.session;
    // An MCP client that connected over HTTP, as the host would list it.
    const session = s.agents.open({ transport: "http", label: "claude-opus (simulated)" }).row.id;
    const steps: Step[] = [];
    const add = (delay: number, run: () => void) => steps.push({ delay, run });
    add(0, () => this.emit(session, first ? "Connected over MCP (developer tools). Goal: make every crate reachable on the sloop's course." : "Checking on the voyage."));
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
      add(2400, () => this.emit(session, "Crate4 floats 6 m off the course and REACH is 3 m, so the crew can never take it. Moving it onto the line."));
      add(3300, () => {
        if (s.mode !== "edit") {
          this.emit(session, "Play is running; I will not edit the world now.");
          return;
        }
        const crate = [...s.world.entities.values()].find((e) => e.name === "Crate4");
        if (!crate) return;
        this.call(
          session,
          "world.edit",
          { ops: [{ set: { entity: crate.id, component: "Transform", value: { position: [26, 0, -2.5] } } }], label: "move Crate4 within reach" },
          (r) => `applied ${(r as { applied: number }).applied} op at tick ${(r as { tick: number }).tick}; undoable from History`,
          "move Crate4 to (26, 0, -2.5)",
        );
      });
      add(4300, () => this.emit(session, "Done: all four crates now lie within 3 m of the sloop's line. Undo it from History to restore the original layout."));
    } else {
      add(1400, () => this.call(session, "events.since", { seq: Math.max(0, s.seq - 5) }, (r) => `${(r as unknown[]).length} recent events`, "what happened lately?"));
      add(2200, () =>
        this.call(session, "world.query", { with: ["Boat"], fields: ["Boat.speed", "Boat.heading_deg"] }, (r) => {
          const row = (r as Record<string, number>[])[0];
          return row ? `Sloop ${Number(row["Boat.speed"]).toFixed(2)} m/s heading ${Number(row["Boat.heading_deg"]).toFixed(0)}°` : "no boat";
        }, "how is the sloop doing?"),
      );
      add(3000, () => this.emit(session, s.mode === "play" ? `Tick ${s.tick}: the voyage is under way.` : "The world is in Edit mode; nothing to report."));
    }
    let at = 0;
    for (const step of steps) {
      at = step.delay;
      this.timers.push(setTimeout(step.run, at));
    }
    // The client ends its session after its last step.
    this.timers.push(setTimeout(() => s.agents.close(session), at + 800));
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
