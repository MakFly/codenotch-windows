import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  emptySnapshot,
  type Snapshot,
  type Account,
  type LoginProvider,
  type ProviderStatus,
  type Settings,
} from "./types";
export const native = isTauri();
export const demo =
  !native && new URLSearchParams(location.search).get("demo") === "1";
let preview: Snapshot = structuredClone(emptySnapshot);
if (!native) {
  let stored: string | null = null;
  try { stored = localStorage.getItem("codenotch-language"); } catch { /* Storage may be disabled in a preview. */ }
  const language = new URLSearchParams(location.search).get("lang") ?? stored;
  if (language === "en" || language === "fr") preview.config.settings.language = language;
}
if (demo) {
  const now = Math.floor(Date.now() / 1000);
  preview.config.accounts = [
    {
      id: "demo-codex",
      name: "Mon compte personnel",
      provider: "codex",
      enabled: true,
      profilePath: "",
      endpoint: "",
      secretEnv: "",
      hasSecret: false,
    },
    {
      id: "demo-claude",
      name: "Mon espace de travail",
      provider: "claude",
      enabled: true,
      profilePath: "",
      endpoint: "",
      secretEnv: "",
      hasSecret: false,
    },
    {
      id: "demo-copilot",
      name: "GitHub personnel",
      provider: "copilot",
      enabled: true,
      profilePath: "",
      endpoint: "",
      secretEnv: "",
      hasSecret: false,
    },
  ];
  preview.readings = preview.config.accounts.map((a, i) => ({
    accountId: a.id,
    status: "ok",
    message: "",
    source: "Données fictives de démonstration",
    plan: ["Plus", "Pro", "Pro"][i],
    windows: [
      {
        label: i === 2 ? "Requêtes premium" : "5 h",
        usedPercent: [32, 68, 14][i],
        resetsAt: now + [8000, 4500, 432000][i],
      },
      ...(i === 2
        ? []
        : [
            {
              label: "7 jours",
              usedPercent: [47, 81][i],
              resetsAt: now + 190000,
            },
          ]),
    ],
    tokensToday: i === 0 ? 124650 : null,
    lifetimeTokens: i === 0 ? 8215000 : null,
    daily:
      i === 0
        ? [62000, 98000, 54000, 166000, 103000, 82000, 124650].map((t, j) => ({
            date: new Date(Date.now() - (6 - j) * 86400000)
              .toISOString()
              .slice(0, 10),
            tokens: t,
          }))
        : [],
    models: [],
    updatedAt: now,
    retryAt: null,
  }));
}
const listeners = new Set<(s: Snapshot) => void>();
function publish() {
  const s = structuredClone(preview);
  listeners.forEach((fn) => fn(s));
  return s;
}
export async function subscribe(
  fn: (s: Snapshot) => void,
): Promise<() => void> {
  if (native) return listen<Snapshot>("snapshot", (e) => fn(e.payload));
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}
export const getSnapshot = () =>
  native
    ? invoke<Snapshot>("get_snapshot")
    : Promise.resolve(structuredClone(preview));
export const refresh = () =>
  native ? invoke<void>("refresh") : Promise.resolve();
export const notchMode = (mode: "collapsed" | "rail" | "detail") =>
  native ? invoke<void>("set_notch_mode", { mode }) : Promise.resolve();
export const dragNotch = () => native ? invoke<void>("drag_notch") : Promise.resolve();
export const subscribeWidgetErrors = (onError: (message: string) => void) =>
  native ? listen<string>("widget-error", e => onError(e.payload)) : Promise.resolve(() => {});
export async function saveAccount(
  account: Account,
  secret: string,
  clearSecret: boolean,
) {
  if (native)
    return invoke<Snapshot>("save_account", {
      account,
      secret: secret || null,
      clearSecret,
    });
  if (secret)
    throw new Error(
      "Les secrets ne sont acceptés que dans l’application Windows.",
    );
  const next = { ...account, id: account.id || crypto.randomUUID() };
  const index = preview.config.accounts.findIndex((a) => a.id === next.id);
  if (index >= 0) preview.config.accounts[index] = next;
  else preview.config.accounts.push(next);
  return publish();
}
export async function removeAccount(id: string) {
  if (native) return invoke<Snapshot>("remove_account", { id });
  preview.config.accounts = preview.config.accounts.filter((a) => a.id !== id);
  preview.readings = preview.readings.filter((r) => r.accountId !== id);
  return publish();
}
export async function saveSettings(settings: Settings) {
  if (native) return invoke<Snapshot>("save_settings", { settings });
  preview.config.settings = settings;
  try { localStorage.setItem("codenotch-language", settings.language); } catch { /* Native settings use the local config file. */ }
  return publish();
}
export const discoverAccounts = () =>
  native
    ? invoke<Snapshot>("discover_accounts")
    : Promise.resolve(structuredClone(preview));
export async function providerStatus(provider?: LoginProvider, profilePath = "") {
  if (!native) return [] as ProviderStatus[];
  const result = await invoke<ProviderStatus[]>("provider_status", {
    provider: provider ?? null,
    profilePath: profilePath || null,
  });
  return Array.isArray(result) ? result : [];
}
export interface LoginStarted {
  /** True when CodeNotch runs the OAuth itself; false when a CLI console window opened. */
  inApp: boolean;
  url: string | null;
  userCode: string | null;
}
export interface LoginFinished {
  provider: LoginProvider;
  success: boolean;
  error: string | null;
}
export async function startLogin(provider: LoginProvider, profilePath = "") {
  if (!native)
    throw new Error("La connexion est disponible dans l’application Windows.");
  const started = await invoke<LoginStarted | null>("start_login", { provider, profilePath: profilePath || null });
  return started && typeof started === "object" ? started : { inApp: false, url: null, userCode: null };
}
export const cancelLogin = () =>
  native ? invoke<void>("cancel_login") : Promise.resolve();
export const subscribeLogin = (fn: (e: LoginFinished) => void) =>
  native ? listen<LoginFinished>("login-finished", (e) => fn(e.payload)) : Promise.resolve(() => {});
export const finishLogin = (provider: LoginProvider) =>
  native
    ? invoke<Snapshot>("finish_login", { provider })
    : Promise.resolve(structuredClone(preview));
export async function windowAction(
  action:
    "minimize" | "hide" | "toggleMaximize" | "startDragging" | "dashboard",
) {
  if (!native) return;
  if (action === "dashboard") await invoke("show_dashboard");
  else if (action === "hide") await invoke("hide_dashboard");
  else if (action === "minimize") await invoke("minimize_dashboard");
  else await getCurrentWindow()[action]();
}
