// Manager chat. The manager proposes; anything consequential still goes
// through policy and the Approval Center.

import { useEffect, useLayoutEffect, useRef, useState, type FormEvent } from "react";
import { api, useAction, useQuery, useTpdEvents } from "../api";
import { Icon, Logo } from "../icons";
import type { ChatMessage } from "../types";
import { ErrorNote, fmtTime, Loading } from "../ui";

const HISTORY = 200;
const STARTERS = ["What needs my attention today?", "What is blocked and why?", "Summarise yesterday's work", "Draft today's plan"];

function merge(list: ChatMessage[], m: ChatMessage): ChatMessage[] {
  if (list.some((x) => x.id === m.id)) return list;
  return [...list, m].sort((a, b) => a.id - b.id).slice(-HISTORY);
}

export default function Chat() {
  const q = useQuery(() => api.chatHistory(HISTORY).then((h) => [...h].sort((a, b) => a.id - b.id)), []);
  const [text, setText] = useState("");
  const [outbox, setOutbox] = useState<string | null>(null);
  const act = useAction();
  const scroller = useRef<HTMLDivElement>(null);
  const box = useRef<HTMLTextAreaElement>(null);

  useTpdEvents((ev) => {
    if (ev.event === "chat.message") q.set((prev) => merge(prev ?? [], ev.data));
  }, ["chat.message"]);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [q.data?.length, outbox]);

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [text]);

  const send = async (e?: FormEvent, preset?: string) => {
    e?.preventDefault();
    const t = (preset ?? text).trim();
    if (!t || act.busy) return;
    setOutbox(t);
    setText("");
    const reply = await act.run(() => api.chatSend(t));
    setOutbox(null);
    if (reply) {
      q.set((prev) => merge(prev ?? [], reply));
      // The founder's message gets its id server-side; fetch it.
      q.reload();
    } else {
      setText(t);
    }
  };

  const msgs = q.data ?? [];
  const empty = msgs.length === 0 && !outbox;
  return (
    <>
      <div className="transcript" ref={scroller} role="log" aria-live="polite" aria-label="Conversation with the manager">
        <div className="transcript-inner">
          <ErrorNote error={q.error} onRetry={q.reload} />
          {q.loading && !q.data ? (
            <Loading />
          ) : empty ? (
            <div className="home" style={{ paddingTop: "12vh", gap: 18 }}>
              <h1 className="greeting">
                <span className="logo">
                  <Logo size={28} />
                </span>
                Ask the manager
              </h1>
              <p className="muted" style={{ textAlign: "center", margin: 0 }}>
                It knows the factory's state and can propose tasks or a plan. It cannot change policy; consequential actions still need your approval.
              </p>
              <div className="suggestions">
                {STARTERS.map((s) => (
                  <button key={s} className="suggestion" onClick={() => void send(undefined, s)} disabled={act.busy}>
                    {s}
                  </button>
                ))}
              </div>
            </div>
          ) : (
            msgs.map((m) => (
              <div key={m.id} className={`msg ${m.role}`}>
                {m.role !== "founder" && (
                  <div className="msg-meta">
                    <span className="logo" style={{ color: "var(--accent)" }}>
                      <Logo size={13} />
                    </span>
                    Manager · <time>{fmtTime(m.ts)}</time>
                  </div>
                )}
                <div className="msg-text">{m.text}</div>
              </div>
            ))
          )}
          {outbox && (
            <>
              <div className="msg founder pending">
                <div className="msg-text">{outbox}</div>
              </div>
              <div className="t-thinking" aria-label="Manager is replying">
                <span className="spinner" /> Thinking…
              </div>
            </>
          )}
        </div>
      </div>
      <div className="dock">
        <ErrorNote error={act.error} />
        <form className="composer" onSubmit={(e) => void send(e)}>
          <textarea
            ref={box}
            rows={1}
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="Message the manager…"
            aria-label="Message"
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                void send();
              }
            }}
          />
          <div className="composer-bar">
            <span className="muted small">The manager proposes; policy decides.</span>
            <button className="send" type="submit" disabled={!text.trim() || act.busy} aria-label="Send">
              {act.busy ? <span className="spinner" style={{ borderTopColor: "#fff" }} /> : <Icon name="arrowUp" />}
            </button>
          </div>
        </form>
      </div>
    </>
  );
}
