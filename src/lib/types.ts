export type Provider = "codex" | "claude" | "gemini" | "grok" | "opencode" | "copilot" | "ollama" | "custom";
/** Providers whose official CLI signs in with OAuth and can be launched from the settings. */
export type LoginProvider = "codex" | "claude" | "gemini" | "grok" | "opencode";
export const loginProviders: LoginProvider[] = ["codex", "claude", "gemini", "grok", "opencode"];
export const isLoginProvider = (p: Provider): p is LoginProvider =>
  (loginProviders as Provider[]).includes(p);
export interface ProviderStatus {
  provider: LoginProvider;
  installed: boolean;
  cliPath: string | null;
  signedIn: boolean;
  signedInAt: number | null;
  profilePath: string;
}
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
  /** False until the first-run welcome screen is finished or skipped. */
  onboardingDone?: boolean | null;
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
    id: "gemini",
    name: "Gemini",
    short: "Gemini",
    color: "#3d68c9",
    description: "Quotas Gemini CLI via sa session Google.",
  },
  {
    id: "grok",
    name: "Grok",
    short: "Grok",
    color: "#5d5d66",
    description: "Crédits d’abonnement Grok via la session Grok CLI.",
  },
  {
    id: "opencode",
    name: "OpenCode",
    short: "OpenCode",
    color: "#3f6b5c",
    description: "Quotas de l’abonnement OpenCode Go via sa clé.",
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
      "OpenRouter ou un autre service, via un endpoint JSON au format SmashToken.",
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
  // The native app decides first-run state; previews open on the settings.
  onboardingDone: true,
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
