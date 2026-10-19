// ⌘K global search (tpd `search` + local navigation) and ⌘P project switcher.

import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "./api";
import { navigate, useApp, VIEWS, type Route } from "./router";
import type { SearchHit } from "./types";

interface Item {
  key: string;
  title: string;
  subtitle?: string;
  kind: string;
  run: () => void;
}

const SEARCH_DEBOUNCE_MS = 150;

function hitRoute(h: SearchHit): Route {
  switch (h.kind) {
    case "task":
      return { view: "tasks", id: h.id };
    case "project":
      return { view: "projects", id: h.id };
    case "approval":
      return { view: "approvals", id: h.id };
    case "memory":
      return { view: "memory" };
    case "audit":
      return { view: "settings", id: "audit" };
    case "deployment":
      return { view: "projects" };
    default:
      return { view: "overview" };
  }
}

export default function Palette({ mode, onClose }: { mode: "search" | "project"; onClose: () => void }) {
  const { projects, setProject, link } = useApp();
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sel, setSel] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const listId = "palette-list";

  useEffect(() => {
    input.current?.focus();
  }, []);

  useEffect(() => {
    if (mode !== "search" || q.trim().length < 2 || link.state !== "online") {
      setHits([]);
      setSearching(false);
      return;
    }
    setSearching(true);
    let live = true;
    const t = window.setTimeout(() => {
      api.search(q.trim(), 20).then(
        (h) => {
          if (!live) return;
          setHits(h);
          setError(null);
          setSearching(false);
        },
        (e: unknown) => {
          if (!live) return;
          setError(e instanceof Error ? e.message : String(e));
          setSearching(false);
        },
      );
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      live = false;
      window.clearTimeout(t);
    };
  }, [q, mode, link.state]);

  const items: Item[] = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const match = (s: string) => !needle || s.toLowerCase().includes(needle);
    if (mode === "project") {
      return [
        { key: "p:", title: "All projects", kind: "scope", run: () => setProject("") },
        ...projects
          .filter((p) => match(p.slug) || match(p.name))
          .map((p) => ({
            key: `p:${p.slug}`,
            title: p.slug,
            subtitle: p.name,
            kind: "scope",
            run: () => setProject(p.slug),
          })),
      ];
    }
    const nav: Item[] = VIEWS.filter((v) => match(v.label)).map((v) => ({
      key: `v:${v.name}`,
      title: `Go to ${v.label}`,
      subtitle: `g ${v.key}`,
      kind: "view",
      run: () => navigate({ view: v.name }),
    }));
    const proj: Item[] = needle
      ? projects
          .filter((p) => match(p.slug) || match(p.name))
          .slice(0, 5)
          .map((p) => ({
            key: `pr:${p.slug}`,
            title: p.slug,
            subtitle: p.name,
            kind: "project",
            run: () => navigate({ view: "projects", id: p.slug }),
          }))
      : [];
    const remote: Item[] = hits
      .filter((h) => !(h.kind === "project" && proj.some((p) => p.title === h.id)))
      .map((h) => ({
        key: `h:${h.kind}:${h.id}`,
        title: h.title,
        subtitle: h.subtitle,
        kind: h.kind,
        run: () => navigate(hitRoute(h)),
      }));
    return [...remote, ...proj, ...nav];
  }, [mode, q, projects, hits, setProject]);

  useEffect(() => {
    setSel(0);
  }, [q, mode]);
  const clamped = Math.min(sel, Math.max(0, items.length - 1));

  const choose = (i: number) => {
    const it = items[i];
    if (!it) return;
    onClose();
    it.run();
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSel((s) => Math.min(s + 1, items.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSel((s) => Math.max(s - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(clamped);
    } else if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    }
  };

  return (
    <div className="overlay top" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="palette" role="dialog" aria-modal="true" aria-label={mode === "search" ? "Search" : "Switch project"}>
        <input
          ref={input}
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={onKey}
          placeholder={mode === "search" ? "Search tasks, projects, approvals, memory, audit…" : "Switch project scope…"}
          role="combobox"
          aria-expanded="true"
          aria-controls={listId}
          aria-activedescendant={items[clamped] ? `pal-${clamped}` : undefined}
          aria-label="Query"
        />
        <ul id={listId} role="listbox" className="palette-list">
          {items.map((it, i) => (
            <li
              key={it.key}
              id={`pal-${i}`}
              role="option"
              aria-selected={i === clamped}
              className={i === clamped ? "sel" : ""}
              onMouseEnter={() => setSel(i)}
              onMouseDown={(e) => {
                e.preventDefault();
                choose(i);
              }}
            >
              <span className="pal-kind">{it.kind}</span>
              <span className="pal-title">{it.title}</span>
              {it.subtitle && <span className="pal-sub">{it.subtitle}</span>}
            </li>
          ))}
          {items.length === 0 && !searching && <li className="muted pal-empty">No matches</li>}
        </ul>
        <div className="palette-foot">
          {searching ? "Searching…" : error ? <span className="bad-text">{error}</span> : "↑↓ to move · ↵ to open · Esc to close"}
        </div>
      </div>
    </div>
  );
}
