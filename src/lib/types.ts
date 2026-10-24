export type Provider = "codex" | "claude" | "copilot" | "ollama" | "custom";
export interface Account {
  id: string;
  name: string;
  provider: Provider;
  enabled: boolean;
  profilePath: string;
  endpoint: string;
  secretEnv: string;
  hasSecret: boolean;
}
export interface Settings {
  language: "en" | "fr";
  pollSeconds: number;
  notifications: boolean;
  alertPercent: number;
  widget: boolean;
  autoHide: boolean;
  hideInFullscreen: boolean;
  gameMode: boolean;
  widgetPlacement: { monitor: string | null; x: number; y: number } | null;
  widgetPosition: "top" | "bottom" | "left" | "right";
}
export interface LimitWindow {
  label: string;
  usedPercent: number;
  resetsAt: number | null;
}
export interface Reading {
  accountId: string;
  status: "ok" | "needsAuth" | "error" | "stale" | "rateLimited";
  message: string;
  source: string;
  plan: string | null;
  windows: LimitWindow[];
  tokensToday: number | null;
  lifetimeTokens: number | null;
  daily: { date: string; tokens: number }[];
  models: string[];
  updatedAt: number | null;
  retryAt: number | null;
}
export interface Snapshot {
  config: { accounts: Account[]; settings: Settings };
  readings: Reading[];
  refreshing: boolean;
  storageError: string | null;
}
export const providers: {
  id: Provider;
  name: string;
  short: string;
  color: string;
  description: string;
}[] = [
  {
    id: "codex",
    name: "Codex / ChatGPT",
    short: "Codex",
    color: "#286245",
    description: "Quotas d’abonnement et activité en tokens via Codex CLI.",
  },
  {
    id: "claude",
    name: "Claude",
    short: "Claude",
    color: "#ae542f",
    description: "Quotas Claude Code via sa session OAuth Windows.",
  },
  {
    id: "copilot",
    name: "GitHub Copilot",
    short: "Copilot",
    color: "#6c56a4",
    description: "Requêtes premium et quotas publiés par GitHub.",
  },
  {
    id: "ollama",
    name: "Ollama",
    short: "Ollama",
    color: "#486c85",
    description:
      "Modèles chargés sur ton serveur local. Pas de quota de tokens.",
  },
  {
    id: "custom",
    name: "Endpoint personnalisé",
    short: "Personnalisé",
    color: "#7e6540",
    description:
      "Gemini, OpenRouter ou un autre service, via un endpoint JSON au format SmashToken.",
  },
];
export const defaultSettings: Settings = {
  language: "en",
  pollSeconds: 60,
  notifications: true,
  alertPercent: 80,
  widget: true,
  autoHide: true,
  hideInFullscreen: true,
  gameMode: false,
  widgetPlacement: null,
  widgetPosition: "right",
};
export const emptySnapshot: Snapshot = {
  config: { accounts: [], settings: defaultSettings },
  readings: [],
  refreshing: false,
  storageError: null,
};
export const blankAccount = (): Account => ({
  id: "",
  name: "",
  provider: "codex",
  enabled: true,
  profilePath: "",
  endpoint: "",
  secretEnv: "",
  hasSecret: false,
});
