// Tauri IPC in the app; in a plain browser (dev only) the same calls go to
// scripts/dev-bridge.py through Vite's /__tp proxy, so the UI can be tested
// and screenshotted without the native shell.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export async function invoke<R>(cmd: string, args?: Record<string, unknown>): Promise<R> {
  if (inTauri) return tauriInvoke<R>(cmd, args);
  const res = await fetch(`/__tp/${cmd}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(args ?? {}),
  });
  const body = (await res.json()) as { ok?: R; err?: string };
  if (body.err !== undefined) throw body.err;
  return body.ok as R;
}

type Payload<P> = { payload: P };
type Channel = "tpd-event" | "tpd-connected" | "tpd-disconnected";
const subs = new Map<Channel, Set<(m: Payload<unknown>) => void>>();
let source: EventSource | null = null;

function emit(ch: Channel, payload: unknown) {
  subs.get(ch)?.forEach((h) => h({ payload }));
}

function openSource() {
  if (source) return;
  let up = false;
  const connect = () => {
    source = new EventSource("/__tp/events");
    source.addEventListener("up", () => {
      up = true;
      emit("tpd-connected", null);
    });
    source.addEventListener("down", (e) => {
      if (up) emit("tpd-disconnected", JSON.parse((e as MessageEvent).data));
      up = false;
    });
    source.onmessage = (e) => emit("tpd-event", JSON.parse(e.data));
    source.onerror = () => {
      if (up) emit("tpd-disconnected", "dev bridge lost the connection");
      up = false;
    };
  };
  connect();
}

export async function listen<P>(ch: Channel, h: (m: Payload<P>) => void): Promise<() => void> {
  if (inTauri) return tauriListen<P>(ch, h);
  const set = subs.get(ch) ?? new Set();
  subs.set(ch, set);
  const f = h as (m: Payload<unknown>) => void;
  set.add(f);
  openSource();
  return () => void set.delete(f);
}
