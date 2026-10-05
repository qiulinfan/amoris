// Errors from the host shown the way the protocol states them: code, message and "did you mean".

import { HostError } from "../host/client";
import { logLocal } from "../state/logs";
import { useUi } from "../state/ui";

export function reportError(e: unknown, context?: string) {
  if (e instanceof HostError) {
    const did = e.suggestions;
    useUi.getState().toast({
      kind: "error",
      title: `${context ? `${context}: ` : ""}${e.code}`,
      body: e.message + (did.length && !e.message.includes("did you mean") ? ` Did you mean ${did.map((d) => `'${d}'`).join(", ")}?` : ""),
    });
    logLocal("error", `${e.method} → ${e.code}: ${e.message}`);
    return;
  }
  const message = e instanceof Error ? e.message : String(e);
  useUi.getState().toast({ kind: "error", title: context ?? "Error", body: message });
  logLocal("error", `${context ? `${context}: ` : ""}${message}`);
}

/** Runs a host call, reporting a failure; resolves undefined then. */
export async function attempt<T>(p: Promise<T>, context?: string): Promise<T | undefined> {
  try {
    return await p;
  } catch (e) {
    reportError(e, context);
    return undefined;
  }
}
