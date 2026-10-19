// Small shared UI primitives. No UI kit: plain elements + styles.css tokens.

import {
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import type { Risk, RunState, TaskState } from "./types";
import { href, type Route } from "./router";

// ------------------------------------------------------------ formatting

export function fmtTime(ts?: string | null): string {
  if (!ts) return "–";
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  return sameDay
    ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })
    : d.toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

export function relTime(ts?: string | null): string {
  if (!ts) return "–";
  const t = new Date(ts).getTime();
  if (Number.isNaN(t)) return ts;
  const s = Math.round((Date.now() - t) / 1000);
  if (s < 0) return "just now";
  if (s < 60) return `${s}s ago`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.round(m / 60);
  if (h < 48) return `${h}h ago`;
  return `${Math.round(h / 24)}d ago`;
}

export function fmtUsd(n?: number | null): string {
  if (n == null) return "–";
  return n >= 100 ? `$${n.toFixed(0)}` : `$${n.toFixed(2)}`;
}

export function fmtNum(n?: number | null): string {
  if (n == null) return "–";
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e4) return `${(n / 1e3).toFixed(0)}k`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}k`;
  return String(n);
}

export function fmtMs(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)} s`;
  return `${Math.floor(s / 60)}m ${Math.round(s % 60)}s`;
}

export function fmtUptime(ms: number): string {
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

export const label = (s: string) => s.replace(/_/g, " ");

// ------------------------------------------------------------ chips

export type Tone = "ok" | "bad" | "warn" | "info" | "muted" | "accent";

export function Chip({ tone = "muted", children, title }: { tone?: Tone; children: ReactNode; title?: string }) {
  return (
    <span className={`chip ${tone}`} title={title}>
      {children}
    </span>
  );
}

const ACTIVE_STATES = new Set<TaskState>(["implementing", "verifying", "repairing", "reviewing", "deploying", "post_verify"]);
export const isActiveState = (s: TaskState) => ACTIVE_STATES.has(s);

export function taskTone(s: TaskState): Tone {
  if (s === "done" || s === "merged") return "ok";
  if (s === "blocked" || s === "escalated" || s === "rolled_back") return "bad";
  if (s === "awaiting_approval" || s === "pr_open") return "warn";
  if (s === "cancelled") return "muted";
  if (ACTIVE_STATES.has(s)) return "info";
  return "muted";
}

export function StateChip({ state }: { state: TaskState }) {
  return <Chip tone={taskTone(state)}>{label(state)}</Chip>;
}

export function riskTone(r: Risk): Tone {
  return r === "critical" || r === "high" ? "bad" : r === "medium" ? "warn" : "ok";
}

export function RiskChip({ risk }: { risk: Risk }) {
  return (
    <Chip tone={riskTone(risk)} title="Risk level">
      {risk} risk
    </Chip>
  );
}

export function runTone(s: RunState): Tone {
  if (s === "running" || s === "starting") return "info";
  if (s === "succeeded") return "ok";
  if (s === "failed") return "bad";
  return "warn";
}

export function statusTone(s: string): Tone {
  switch (s) {
    case "ok":
    case "passed":
    case "healthy":
    case "validated":
    case "approved":
    case "applied":
    case "resolved":
      return "ok";
    case "fail":
    case "failed":
    case "blocking":
    case "critical":
    case "rejected":
    case "rolled_back":
      return "bad";
    case "warn":
    case "warning":
    case "major":
    case "blocked":
    case "inconclusive":
    case "deferred":
    case "proposed":
    case "pending":
    case "draft":
      return "warn";
    case "running":
    case "info":
      return "info";
    default:
      return "muted";
  }
}

// ------------------------------------------------------------ layout bits

export function Card({
  title,
  count,
  actions,
  children,
  className,
}: {
  title: ReactNode;
  count?: number;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={`card ${className ?? ""}`}>
      <div className="card-head">
        <h2>
          {title}
          {count != null && <span className="count">{count}</span>}
        </h2>
        {actions && <div className="card-actions">{actions}</div>}
      </div>
      {children}
    </section>
  );
}

export function PageHeader({ title, sub, actions }: { title: string; sub?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="page-head">
      <div>
        <h1>{title}</h1>
        {sub && <p className="muted">{sub}</p>}
      </div>
      {actions && <div className="page-actions">{actions}</div>}
    </div>
  );
}

export function ErrorNote({ error, onRetry }: { error: string | null | undefined; onRetry?: () => void }) {
  if (!error) return null;
  return (
    <div className="note bad" role="alert">
      <span>{error}</span>
      {onRetry && (
        <button className="btn small" onClick={onRetry}>
          Retry
        </button>
      )}
    </div>
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return <p className="empty">{children}</p>;
}

export function Loading({ what = "Loading" }: { what?: string }) {
  return (
    <p className="muted loading" aria-live="polite">
      {what}…
    </p>
  );
}

export function A({ to, children, className }: { to: Route; children: ReactNode; className?: string }) {
  return (
    <a href={href(to)} className={className}>
      {children}
    </a>
  );
}

/** External link (PRs, issues). */
export function Ext({ url, children }: { url: string; children?: ReactNode }) {
  return (
    <a href={url} target="_blank" rel="noreferrer noopener" className="ext">
      {children ?? url}
    </a>
  );
}

// ------------------------------------------------------------ tabs

export function Tabs<K extends string>({
  tabs,
  value,
  onChange,
  ariaLabel,
}: {
  tabs: { key: K; label: ReactNode }[];
  value: K;
  onChange: (k: K) => void;
  ariaLabel: string;
}) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const onKey = (e: KeyboardEvent, i: number) => {
    let j = -1;
    if (e.key === "ArrowRight") j = (i + 1) % tabs.length;
    if (e.key === "ArrowLeft") j = (i - 1 + tabs.length) % tabs.length;
    if (j >= 0) {
      e.preventDefault();
      onChange(tabs[j].key);
      refs.current[j]?.focus();
    }
  };
  return (
    <div className="tabs" role="tablist" aria-label={ariaLabel}>
      {tabs.map((t, i) => (
        <button
          key={t.key}
          ref={(n) => {
            refs.current[i] = n;
          }}
          role="tab"
          aria-selected={t.key === value}
          tabIndex={t.key === value ? 0 : -1}
          className={t.key === value ? "tab active" : "tab"}
          onClick={() => onChange(t.key)}
          onKeyDown={(e) => onKey(e, i)}
        >
          {t.label}
        </button>
      ))}
    </div>
  );
}

// ------------------------------------------------------------ forms

export function Field({ label: l, children, hint }: { label: string; children: ReactNode; hint?: ReactNode }) {
  return (
    <label className="field">
      <span className="field-label">{l}</span>
      {children}
      {hint && <span className="field-hint">{hint}</span>}
    </label>
  );
}

export function Toggle({
  checked,
  onChange,
  label: l,
  disabled,
  hint,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  disabled?: boolean;
  hint?: string;
}) {
  const id = useId();
  return (
    <div className="toggle-row">
      <button
        id={id}
        role="switch"
        aria-checked={checked}
        className={`switch ${checked ? "on" : ""}`}
        onClick={() => onChange(!checked)}
        disabled={disabled}
      >
        <span className="knob" />
      </button>
      <label htmlFor={id}>
        {l}
        {hint && <span className="field-hint">{hint}</span>}
      </label>
    </div>
  );
}

// ------------------------------------------------------------ dialogs

export function Modal({
  title,
  onClose,
  children,
  wide,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  wide?: boolean;
}) {
  const box = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    const first = box.current?.querySelector<HTMLElement>(
      "input:not([disabled]), textarea, select, button:not(.modal-x)",
    );
    (first ?? box.current)?.focus();
    return () => prev?.focus?.();
  }, []);
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose();
    }
    if (e.key === "Tab" && box.current) {
      const f = box.current.querySelectorAll<HTMLElement>(
        "button:not([disabled]), input:not([disabled]), textarea, select, a[href]",
      );
      if (f.length === 0) return;
      const a = f[0];
      const z = f[f.length - 1];
      if (e.shiftKey && document.activeElement === a) {
        e.preventDefault();
        z.focus();
      } else if (!e.shiftKey && document.activeElement === z) {
        e.preventDefault();
        a.focus();
      }
    }
  };
  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div
        ref={box}
        className={`modal ${wide ? "wide" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={onKey}
      >
        <div className="modal-head">
          <h2 id={titleId}>{title}</h2>
          <button className="modal-x" aria-label="Close" onClick={onClose}>
            ×
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}

/** Confirmation dialog; `typed` requires the user to type a word to confirm. */
export function Confirm({
  title,
  body,
  confirmLabel = "Confirm",
  danger,
  typed,
  busy,
  error,
  onConfirm,
  onClose,
}: {
  title: string;
  body: ReactNode;
  confirmLabel?: string;
  danger?: boolean;
  typed?: string;
  busy?: boolean;
  error?: string | null;
  onConfirm: () => void;
  onClose: () => void;
}) {
  const [text, setText] = useState("");
  const ok = !typed || text.trim() === typed;
  return (
    <Modal title={title} onClose={onClose}>
      <div className="modal-body">{body}</div>
      {typed && (
        <Field label={`Type "${typed}" to confirm`}>
          <input value={text} onChange={(e) => setText(e.target.value)} autoComplete="off" />
        </Field>
      )}
      <ErrorNote error={error} />
      <div className="modal-foot">
        <button className="btn" onClick={onClose}>
          Cancel
        </button>
        <button
          className={`btn ${danger ? "danger" : "primary"}`}
          disabled={!ok || busy}
          onClick={onConfirm}
        >
          {busy ? "Working…" : confirmLabel}
        </button>
      </div>
    </Modal>
  );
}

export function JsonBlock({ value }: { value: unknown }) {
  if (value == null) return <span className="muted">–</span>;
  let s: string;
  try {
    s = JSON.stringify(value, null, 2);
  } catch {
    s = String(value);
  }
  return <pre className="json">{s.length > 20000 ? `${s.slice(0, 20000)}\n… (truncated)` : s}</pre>;
}

// ------------------------------------------------------------ toasts

type ToastMsg = { id: number; text: string; tone?: "bad" };
const toastSubs = new Set<(t: ToastMsg | null) => void>();
let toastSeq = 0;

/** Brief confirmation at the bottom of the window. */
export function toast(text: string, tone?: "bad") {
  const t = { id: ++toastSeq, text, tone };
  toastSubs.forEach((h) => h(t));
}

export function Toaster() {
  const [t, setT] = useState<ToastMsg | null>(null);
  useEffect(() => {
    let timer = 0;
    const h = (m: ToastMsg | null) => {
      setT(m);
      window.clearTimeout(timer);
      if (m) timer = window.setTimeout(() => setT(null), m.tone === "bad" ? 7000 : 3000);
    };
    toastSubs.add(h);
    return () => {
      toastSubs.delete(h);
      window.clearTimeout(timer);
    };
  }, []);
  if (!t) return null;
  return (
    <div className={`toast ${t.tone ?? ""}`} role="status" key={t.id}>
      <span className="pre">{t.text}</span>
      <button className="modal-x" style={{ color: "inherit" }} aria-label="Dismiss" onClick={() => setT(null)}>
        ×
      </button>
    </div>
  );
}
