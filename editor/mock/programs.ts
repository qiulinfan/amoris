// The agent programs the mock host may start (docs/spec/server.md 3.7; amoris-server's
// launch.rs), in the host's shapes: `agent.session.agents` lists them by name and label,
// `agent.session.start {agent, goal, seat?}` opens a session of transport `launch` and runs the
// program, whose lines are the session's `message` events and whose calls are the session's, and
// the session closes when it exits or `agent.session.stop` ends it. The mock's one program is
// scripted: it says its goal, lists the scene and exits.

import type { Session } from "./session";
import type { AgentRow } from "./sessions";
import { MockError } from "./util";

/** Programs the host runs at once. */
export const RUNNING = 4;
const GOAL_MAX = 4000;

export const PROGRAMS = [{ name: "scripted", label: "Scripted agent (mock)" }];

export class MockPrograms {
  /** Timers of each running program, by session id. */
  private running = new Map<string, ReturnType<typeof setTimeout>[]>();
  private pid = 4000;

  constructor(
    private readonly session: Session,
    /** Milliseconds between the program's steps. */
    private readonly pace = 700,
  ) {}

  agents() {
    return { agents: PROGRAMS, running: this.running.size, limit: RUNNING };
  }

  start(p: { agent?: unknown; goal?: unknown; seat?: unknown }): AgentRow & { agent: string; pid: number } {
    const agent = String(p.agent ?? "");
    const program = PROGRAMS.find((x) => x.name === agent);
    if (!program) {
      throw new MockError("request.invalid_value", `'agent' must be one of ${PROGRAMS.map((x) => x.name).join(", ")}; got '${agent}'.`, {
        path: "/agent",
        got: agent,
        allowed: PROGRAMS.map((x) => x.name),
      });
    }
    const goal = String(p.goal ?? "").trim();
    if (goal.length === 0 || goal.length > GOAL_MAX) {
      throw new MockError("request.out_of_range", `A goal is 1 to ${GOAL_MAX} characters.`, { path: "/goal", got: goal.length, max: GOAL_MAX });
    }
    if (this.running.size >= RUNNING) {
      throw new MockError("session.limit_reached", `This host already runs ${this.running.size} agent programs; the limit is ${RUNNING}.`, {
        limit: RUNNING,
        value: this.running.size,
        what: "agent programs",
      });
    }
    const seat = typeof p.seat === "string" && p.seat !== "" ? p.seat : undefined;
    const agents = this.session.agents;
    const { row } = agents.open({ transport: "launch", label: program.label, seat });
    const id = row.id;
    const pid = ++this.pid;
    agents.message(id, `started ${agent} (pid ${pid}): ${goal}`, "host");
    const timers: ReturnType<typeof setTimeout>[] = [];
    this.running.set(id, timers);
    const at = (n: number, run: () => void) => timers.push(setTimeout(run, n * this.pace));
    at(1, () => agents.message(id, `goal: ${goal}`, "stdout"));
    at(2, () => {
      agents.call(id, "world.tree", "{}");
      try {
        const r = this.session.call("world.tree", {}, { source: { developer: 100 + pid }, session: id }) as unknown[];
        agents.result(id, "world.tree", true, `${r.length} roots`);
      } catch (e) {
        agents.result(id, "world.tree", false, e instanceof MockError ? `${e.code}: ${e.message}` : String(e));
      }
    });
    at(3, () => agents.message(id, "Listed the scene; nothing else to do.", "stdout"));
    at(4, () => this.end(id, "exit status: 0"));
    return { ...row, agent, pid };
  }

  stop(p: { session?: unknown }): AgentRow {
    const id = String(p.session ?? "");
    const timers = this.running.get(id);
    if (!timers) {
      const row = this.session.agents.row(id);
      if (row && row.transport === "launch" && !row.live) return row;
      throw new MockError("host.session_unknown", `This host runs no agent program as the session '${id}'; agent.session.list lists the sessions (transport launch).`, {
        session: id,
      });
    }
    this.session.agents.message(id, "stopping", "host");
    return this.end(id, "signal: 9 (SIGKILL)");
  }

  private end(id: string, status: string): AgentRow {
    this.running.get(id)?.forEach(clearTimeout);
    this.running.delete(id);
    this.session.agents.message(id, status, "host");
    return this.session.agents.close(id)!;
  }

  /** Ends every program (the mock host stops). */
  stopAll() {
    for (const id of [...this.running.keys()]) this.end(id, "signal: 15 (SIGTERM)");
  }
}
