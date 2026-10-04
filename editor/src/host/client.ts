// The host client: one WebSocket to `/ws` carrying requests (with ids the client picks), their
// responses (answered in completion order) and pushed events, reconnecting with backoff. On every
// (re)connection it subscribes to the topics it was asked for and tells its listeners, which reload
// what they show: the host is the only authority, so nothing is replayed from the client.

import {
  ALL_TOPICS,
  type CatalogCommand,
  type MethodName,
  type Params,
  type RequestId,
  type Result,
  type Topic,
  type TopicData,
  type WireError,
  type WireResponse,
} from "./protocol";

export type ConnectionState = "connecting" | "open" | "closed";

export class HostError extends Error {
  readonly code: string;
  readonly detail: Record<string, unknown>;
  readonly method: string;

  constructor(error: WireError, method: string) {
    super(error.message);
    this.code = error.code;
    this.detail = error.detail ?? {};
    this.method = method;
  }

  /** "did you mean" suggestions the host attached, if any. */
  get suggestions(): string[] {
    const d = this.detail.did_you_mean;
    return Array.isArray(d) ? d.map(String) : [];
  }
}

export interface HostEndpoints {
  /** Origin for HTTP endpoints, e.g. "" (same origin) or "http://127.0.0.1:7879". */
  http: string;
  /** WebSocket URL of `/ws`. */
  ws: string;
  /** Shown in the status bar. */
  label: string;
}

/** Same origin by default (the host serves the editor; Vite proxies in development); `?host=` overrides. */
export function endpointsFromLocation(loc: Location = window.location): HostEndpoints {
  const override = new URLSearchParams(loc.search).get("host");
  if (override) {
    const base = /^https?:\/\//.test(override) ? override : `http://${override}`;
    const u = new URL(base);
    return { http: u.origin, ws: `${u.protocol === "https:" ? "wss" : "ws"}://${u.host}/ws`, label: u.host };
  }
  const wsProto = loc.protocol === "https:" ? "wss" : "ws";
  return { http: "", ws: `${wsProto}://${loc.host}/ws`, label: loc.host };
}

interface Pending {
  method: string;
  resolve: (v: unknown) => void;
  reject: (e: unknown) => void;
  timer: ReturnType<typeof setTimeout>;
}

type Listener = (data: unknown) => void;

export interface ConnectionInfo {
  state: ConnectionState;
  retryAt: number | null;
  attempts: number;
  lastError: string | null;
}

export class HostClient {
  private ws: WebSocket | null = null;
  private nextId = 1;
  private readonly pending = new Map<RequestId, Pending>();
  private readonly listeners = new Map<string, Set<Listener>>();
  private readonly stateListeners = new Set<(info: ConnectionInfo) => void>();
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private stopped = false;
  info: ConnectionInfo = { state: "closed", retryAt: null, attempts: 0, lastError: null };

  constructor(
    readonly endpoints: HostEndpoints,
    private readonly topics: Topic[] = ALL_TOPICS,
  ) {}

  connect() {
    this.stopped = false;
    if (this.ws && this.ws.readyState <= WebSocket.OPEN) return;
    this.setInfo({ state: "connecting", retryAt: null });
    let ws: WebSocket;
    try {
      ws = new WebSocket(this.endpoints.ws);
    } catch (e) {
      this.scheduleRetry(String(e));
      return;
    }
    this.ws = ws;
    ws.onopen = () => {
      this.setInfo({ state: "open", attempts: 0, lastError: null, retryAt: null });
      void this.call("subscribe", { topics: this.topics }).catch(() => undefined);
    };
    ws.onmessage = (ev) => this.receive(ev.data);
    ws.onerror = () => {
      this.info.lastError = `Cannot reach ${this.endpoints.label}`;
    };
    ws.onclose = () => {
      if (this.ws === ws) this.ws = null;
      this.failPending("client.disconnected", "The connection to the host closed.");
      if (!this.stopped) this.scheduleRetry(this.info.lastError ?? "Connection closed");
      else this.setInfo({ state: "closed", retryAt: null });
    };
  }

  close() {
    this.stopped = true;
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.ws?.close();
  }

  /** Reconnects now instead of waiting for the backoff. */
  retryNow() {
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.retryTimer = null;
    this.connect();
  }

  private scheduleRetry(reason: string) {
    const attempts = this.info.attempts + 1;
    const delay = Math.min(5000, 400 * 2 ** Math.min(attempts - 1, 4)) + Math.random() * 200;
    this.setInfo({ state: "closed", attempts, lastError: reason, retryAt: Date.now() + delay });
    this.retryTimer = setTimeout(() => this.connect(), delay);
  }

  private setInfo(patch: Partial<ConnectionInfo>) {
    this.info = { ...this.info, ...patch };
    for (const l of this.stateListeners) l(this.info);
  }

  private failPending(code: string, message: string) {
    for (const [id, p] of this.pending) {
      clearTimeout(p.timer);
      p.reject(new HostError({ code, message }, p.method));
      this.pending.delete(id);
    }
  }

  private receive(raw: unknown) {
    let msg: WireResponse & { event?: string; data?: unknown };
    try {
      msg = JSON.parse(String(raw));
    } catch {
      return;
    }
    if (typeof msg.event === "string") {
      const set = this.listeners.get(msg.event);
      if (set) for (const l of set) l(msg.data);
      const any = this.listeners.get("*");
      if (any) for (const l of any) l(msg);
      return;
    }
    if (msg.id === null || msg.id === undefined) return;
    const p = this.pending.get(msg.id);
    if (!p) return;
    this.pending.delete(msg.id);
    clearTimeout(p.timer);
    if (msg.error) p.reject(new HostError(msg.error, p.method));
    else p.resolve(msg.result);
  }

  /** A typed call of a section 4 method. */
  call<M extends MethodName>(method: M, params: Params<M>, timeoutMs = 30000): Promise<Result<M>> {
    return this.callRaw(method, params, timeoutMs) as Promise<Result<M>>;
  }

  /** Any catalog method, by name (the command palette calls methods it learns at run time). */
  callRaw(method: string, params: unknown, timeoutMs = 30000): Promise<unknown> {
    const ws = this.ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      return Promise.reject(new HostError({ code: "client.offline", message: `Not connected to the host (${this.endpoints.label}).` }, method));
    }
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new HostError({ code: "client.timeout", message: `${method} got no answer in ${timeoutMs / 1000} s.` }, method));
      }, timeoutMs);
      this.pending.set(id, { method, resolve, reject, timer });
      ws.send(JSON.stringify({ id, method, params: params ?? {} }));
    });
  }

  on<T extends Topic>(topic: T, fn: (data: TopicData[T]) => void): () => void {
    let set = this.listeners.get(topic);
    if (!set) this.listeners.set(topic, (set = new Set()));
    set.add(fn as Listener);
    return () => set!.delete(fn as Listener);
  }

  onState(fn: (info: ConnectionInfo) => void): () => void {
    this.stateListeners.add(fn);
    fn(this.info);
    return () => this.stateListeners.delete(fn);
  }

  /** `GET /api/catalog`: the authoritative list of commands. */
  async catalog(): Promise<CatalogCommand[]> {
    const r = await fetch(`${this.endpoints.http}/api/catalog`);
    if (!r.ok) throw new HostError({ code: "client.http", message: `GET /api/catalog: HTTP ${r.status}` }, "catalog");
    const body = (await r.json()) as unknown;
    const list = Array.isArray(body) ? body : ((body as { commands?: unknown[] }).commands ?? []);
    return (list as CatalogCommand[]).map((c) => ({ ...c, kind: String(c.kind ?? "").toLowerCase() }));
  }
}
