// Hash router: `#/tasks`, `#/tasks/<id>`, `#/projects/<slug>`. Survives reloads
// and keeps the app free of a routing dependency.

import { createContext, useContext, useEffect, useState } from "react";
import type { IconName } from "./icons";
import type { Alert, Link, Project } from "./types";

export interface AlertEntry {
  id: number;
  ts: string;
  alert: Alert;
}

export type ViewName =
  | "overview"
  | "plan"
  | "approvals"
  | "tasks"
  | "projects"
  | "chat"
  | "memory"
  | "usage"
  | "vault"
  | "settings";

export interface Route {
  view: ViewName;
  /** Task id (tasks) or project slug (projects). */
  id?: string;
}

export const VIEWS: { name: ViewName; label: string; key: string; icon: IconName; group: "main" | "factory" }[] = [
  { name: "overview", label: "Home", key: "o", icon: "home", group: "main" },
  { name: "approvals", label: "Approvals", key: "a", icon: "inbox", group: "main" },
  { name: "plan", label: "Daily plan", key: "p", icon: "calendar", group: "main" },
  { name: "tasks", label: "Tasks", key: "t", icon: "list", group: "main" },
  { name: "projects", label: "Projects", key: "j", icon: "folder", group: "main" },
  { name: "chat", label: "Manager", key: "c", icon: "chat", group: "main" },
  { name: "memory", label: "Memory", key: "m", icon: "brain", group: "factory" },
  { name: "usage", label: "Usage & cost", key: "u", icon: "chart", group: "factory" },
  { name: "vault", label: "Vault", key: "v", icon: "lock", group: "factory" },
  { name: "settings", label: "Settings & safety", key: "s", icon: "settings", group: "factory" },
];

const NAMES = new Set<string>(VIEWS.map((v) => v.name));

export function parseHash(hash: string): Route {
  const parts = hash.replace(/^#\/?/, "").split("/").filter(Boolean).map(decodeURIComponent);
  const view = parts[0] && NAMES.has(parts[0]) ? (parts[0] as ViewName) : "overview";
  return parts[1] ? { view, id: parts[1] } : { view };
}

export function href(r: Route): string {
  return `#/${r.view}${r.id ? `/${encodeURIComponent(r.id)}` : ""}`;
}

export function navigate(r: Route) {
  const h = href(r);
  if (window.location.hash !== h) window.location.hash = h;
}

export function useHashRoute(): Route {
  const [route, setRoute] = useState(() => parseHash(window.location.hash));
  useEffect(() => {
    const on = () => setRoute(parseHash(window.location.hash));
    window.addEventListener("hashchange", on);
    return () => window.removeEventListener("hashchange", on);
  }, []);
  return route;
}

// ------------------------------------------------------------ app context

export interface AppCtx {
  link: Link;
  /** Global project scope from the top-bar switcher ("" = all). */
  project: string;
  setProject: (slug: string) => void;
  projects: Project[];
  refreshStatus: () => void;
  /** Alerts received since this window opened (tpd has no alert history method). */
  alerts: AlertEntry[];
}

export const AppContext = createContext<AppCtx>({
  link: { state: "connecting" },
  project: "",
  setProject: () => undefined,
  projects: [],
  refreshStatus: () => undefined,
  alerts: [],
});

export const useApp = () => useContext(AppContext);
