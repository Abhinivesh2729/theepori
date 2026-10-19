// Manager chat. The manager proposes; anything consequential still goes
// through policy and the Approval Center.

import { useEffect, useRef, useState, type FormEvent } from "react";
import { api, useAction, useQuery, useTpdEvents } from "../api";
import type { ChatMessage } from "../types";
import { Empty, ErrorNote, fmtTime, Loading, PageHeader } from "../ui";

const HISTORY = 200;

function merge(list: ChatMessage[], m: ChatMessage): ChatMessage[] {
  if (list.some((x) => x.id === m.id)) return list;
  return [...list, m].sort((a, b) => a.id - b.id).slice(-HISTORY);
}

export default function Chat() {
  const q = useQuery(() => api.chatHistory(HISTORY).then((h) => [...h].sort((a, b) => a.id - b.id)), []);
  const [text, setText] = useState("");
  const [outbox, setOutbox] = useState<string | null>(null);
  const act = useAction();
  const end = useRef<HTMLDivElement>(null);

  useTpdEvents((ev) => {
    if (ev.event === "chat.message") q.set((prev) => merge(prev ?? [], ev.data));
  }, ["chat.message"]);

  useEffect(() => {
    end.current?.scrollIntoView({ block: "end" });
  }, [q.data?.length, outbox]);

  const send = async (e?: FormEvent) => {
    e?.preventDefault();
    const t = text.trim();
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
  return (
    <div className="page fill chat">
      <PageHeader title="Manager" sub="Ask about status, priorities or a task. The manager can propose work; it cannot change policy." />
      <ErrorNote error={q.error} onRetry={q.reload} />
      <div className="chat-log" role="log" aria-live="polite" aria-label="Conversation">
        {q.loading && !q.data ? (
          <Loading />
        ) : msgs.length === 0 && !outbox ? (
          <Empty>No messages yet. Try “What needs my attention today?”</Empty>
        ) : (
          msgs.map((m) => (
            <div key={m.id} className={`msg ${m.role}`}>
              <div className="msg-meta">
                <b>{m.role === "founder" ? "You" : "Manager"}</b> <time>{fmtTime(m.ts)}</time>
              </div>
              <div className="msg-text">{m.text}</div>
            </div>
          ))
        )}
        {outbox && (
          <>
            <div className="msg founder pending">
              <div className="msg-meta">
                <b>You</b>
              </div>
              <div className="msg-text">{outbox}</div>
            </div>
            <div className="msg manager pending" aria-label="Manager is replying">
              <div className="msg-text muted">Thinking…</div>
            </div>
          </>
        )}
        <div ref={end} />
      </div>
      <ErrorNote error={act.error} />
      <form className="chat-input" onSubmit={(e) => void send(e)}>
        <textarea
          rows={3}
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="Message the manager (⌘↵ to send)"
          aria-label="Message"
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void send();
          }}
        />
        <button className="btn primary" type="submit" disabled={!text.trim() || act.busy}>
          Send
        </button>
      </form>
    </div>
  );
}
