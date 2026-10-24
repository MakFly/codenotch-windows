import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import {
  Settings2,
  RefreshCw,
  X,
  Minus,
  UsersRound,
  Info,
  Plus,
  GripVertical,
  AlertCircle,
  Check,
  ChevronsRight,
} from "lucide-react";
import { Accounts, Preferences, Help } from "./Settings";
import { Mark } from "./components/Controls";
import {
  emptySnapshot,
  providers,
  type Account,
  type Reading,
  type Snapshot,
} from "./lib/types";
import {
  headline,
  isFresh,
  percentage,
  resetTime,
  freshness,
  tokens,
  statusLabels,
} from "./lib/format";
import * as api from "./lib/bridge";
import { Locale, useLocale } from "./lib/Locale";
import { translator } from "./lib/i18n";

const nameOf = (a: Account) =>
  providers.find((p) => p.id === a.provider)!.short;
export function usageColor(value: number | undefined) {
  return value == null
    ? "#858585"
    : value < 50
      ? "#28e07b"
      : value < 80
        ? "#f5e400"
        : "#ff4500";
}
function Ring({
  account,
  reading,
  interval,
}: {
  account: Account;
  reading?: Reading;
  interval: number;
}) {
  const value = headline(reading)?.usedPercent;
  const stale = !!reading && !isFresh(reading, interval);
  const circumference = 2 * Math.PI * 20;
  return (
    <span className={`quota-ring ${stale ? "is-stale" : ""}`}>
      <svg className="ring-arcs" viewBox="0 0 48 48" aria-hidden="true">
        <circle cx="24" cy="24" r="20" className="ring-track" />
        {value != null && (
          <circle
            cx="24"
            cy="24"
            r="20"
            className="ring-value"
            stroke={usageColor(value)}
            strokeDasharray={`${(Math.min(100, Math.max(0, value)) / 100) * circumference} ${circumference}`}
            transform="rotate(-90 24 24)"
          />
        )}
      </svg>
      <Mark type={account.provider} />
      {stale && <span className="stale-dot" />}
    </span>
  );
}

function UsagePopover({
  account,
  reading,
  interval,
  refresh,
  busy,
  onClose,
}: {
  account: Account;
  reading?: Reading;
  interval: number;
  refresh: () => void;
  busy: boolean;
  onClose: () => void;
}) {
  const fresh = isFresh(reading, interval);
  const { language, t } = useLocale();
  return (
    <section
      className="usage-popover"
      aria-label={t("Consommation {name}", { name: t(nameOf(account)) })}
    >
      <header className="popover-header">
        <Mark type={account.provider} />
        <h2>
          {t(nameOf(account))} <span>{t("Usage")}</span>
        </h2>
        <div className="popover-actions">
          <button aria-label={t("Fermer le détail")} onClick={onClose}>
            <X size={15} />
          </button>
        </div>
      </header>
      <div className="account-identity">
        <span>{account.name}</span>
        {reading?.plan && <span>{reading.plan}</span>}
      </div>
      {reading && !fresh && (
        <div className="reading-warning">
          <AlertCircle size={14} />
          <span>
            {reading.status === "ok"
              ? t("Dernier relevé ancien")
              : t(statusLabels[reading.status])}
            . {t(reading.message)}
          </span>
        </div>
      )}
      <div className="quota-windows">
        {reading?.windows.length ? (
          reading.windows.map((w, i) => (
            <div className="quota-window" key={`${w.label}-${i}`}>
              <div className="window-label">
                <span>{t(w.label)}</span>
                <span>{resetTime(w.resetsAt, Date.now(), language)}</span>
              </div>
              <div
                className="quota-track"
                role="progressbar"
                aria-label={t("Quota consommé {label}", { label: t(w.label) })}
                aria-valuenow={Math.min(100, w.usedPercent)}
                aria-valuemin={0}
                aria-valuemax={100}
              >
                <span
                  style={{
                    width: `${Math.min(100, w.usedPercent)}%`,
                    background: usageColor(w.usedPercent),
                  }}
                />
              </div>
              <p>
                {percentage(w.usedPercent, language)} <span>{t("utilisés")}</span>
              </p>
            </div>
          ))
        ) : (
          <p className="missing-reading">
            {account.provider === "ollama"
              ? t("Pas de quota pour ce serveur local.")
              : (reading && !fresh ? t("Les quotas seront affichés après la prochaine connexion réussie.") : reading?.message ? t(reading.message) : undefined) ||
                t("En attente du premier relevé. Configurez la connexion dans les réglages.")}
          </p>
        )}
      </div>
      {(reading?.tokensToday != null || reading?.lifetimeTokens != null) && (
        <div className="token-readings">
          {reading.tokensToday != null && (
            <div>
              <span>{t("Tokens aujourd’hui")}{" "}<small>UTC</small>
              </span>
              <strong>{tokens(reading.tokensToday, false, language)}</strong>
            </div>
          )}
          {reading.lifetimeTokens != null && (
            <div>
              <span>{t("Total communiqué")}</span>
              <strong>{tokens(reading.lifetimeTokens, true, language)}</strong>
            </div>
          )}
        </div>
      )}
      {!!reading?.models.length && (
        <div className="local-models">
          {reading.models.map((m) => (
            <span key={m}>{m}</span>
          ))}
        </div>
      )}
      <footer className="popover-footer">
        <span title={reading?.source ? t(reading.source) : undefined}>{freshness(reading?.updatedAt, Date.now(), language)}</span>
        <button
          onClick={refresh}
          disabled={busy || !api.native}
          aria-label={t("Actualiser {name}", { name: t(nameOf(account)) })}
        >
          <RefreshCw size={12} className={busy ? "spin" : ""} />{t("Actualiser")}</button>
      </footer>
    </section>
  );
}

export default function NotchApp() {
  const [snapshot, setSnapshot] = useState<Snapshot>(emptySnapshot);
  const language = snapshot.config.settings.language ?? "en";
  const t = translator(language);
  useLayoutEffect(() => {
    document.documentElement.lang = language;
    document.title = language === "fr" ? "CodeNotch pour Windows" : "CodeNotch for Windows";
  }, [language]);
  const [selected, setSelected] = useState<string | null>(null);
  const [railOpen, setRailOpen] = useState(false);
  const [closing, setClosing] = useState(false);
  const [motionReady, setMotionReady] = useState(false);
  const [surfaceMode, setSurfaceMode] = useState<"collapsed" | "rail" | "detail">("collapsed");
  const [menu, setMenu] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(
    api.native && new URLSearchParams(location.search).get("widget") !== "1",
  );
  const [page, setPage] = useState<"accounts" | "appearance" | "help">(
    "accounts",
  );
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [, setTime] = useState(0);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const foldTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const geometryQueue = useRef(Promise.resolve());
  const dragging = useRef(false);
  const overlayRef = useRef<HTMLDivElement>(null);
  const settingsButton = useRef<HTMLButtonElement>(null);
  const focusAfterOpen = useRef(false);
  const isWidget =
    api.native && new URLSearchParams(location.search).get("widget") === "1";
  const nativeSettings = api.native && !isWidget;
  const active = snapshot.config.accounts.filter((a) => a.enabled);
  const account = active.find((a) => a.id === selected);
  const reading = snapshot.readings.find((r) => r.accountId === selected);
  const edge = snapshot.config.settings.widgetPosition;
  const vertical = edge === "right" || edge === "left";
  const collapsed = snapshot.config.settings.autoHide && !railOpen && !selected && !menu;
  const nativeMode = collapsed ? "collapsed" : selected || menu ? "detail" : "rail";
  useLayoutEffect(() => {
    if (collapsed) setMotionReady(false);
    if (nativeMode !== "detail") setSurfaceMode(nativeMode);
  }, [collapsed, nativeMode]);
  useEffect(() => {
    if (!collapsed && motionReady && focusAfterOpen.current) {
      focusAfterOpen.current = false;
      document.querySelector<HTMLButtonElement>(".provider-cell")?.focus();
    }
  }, [collapsed, motionReady]);
  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    api
      .subscribe((s) => {
        if (!disposed) setSnapshot(s);
      })
      .then((fn) => {
        if (disposed) fn();
        else stop = fn;
      });
    api
      .getSnapshot()
      .then((s) => {
        if (!disposed) setSnapshot(s);
      })
      .catch((e) => setError(String(e)));
    const t = setInterval(() => setTime(Date.now()), 15000);
    return () => {
      disposed = true;
      stop?.();
      clearInterval(t);
      if (closeTimer.current) clearTimeout(closeTimer.current);
      if (foldTimer.current) clearTimeout(foldTimer.current);
    };
  }, []);
  useEffect(() => {
    document.body.classList.toggle("native-widget", isWidget);
    document.body.classList.toggle("native-settings", nativeSettings);
  }, [isWidget, nativeSettings]);
  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    void api.subscribeWidgetErrors(setError).then(fn => { if (disposed) fn(); else stop = fn; }).catch(e => setError(String(e)));
    return () => { disposed = true; stop?.(); };
  }, []);
  useEffect(() => {
    let disposed = false;
    let frame = 0;
    // Extend the native input/visible region before revealing its contents.
    // WebView2 keeps the same size; serialize updates so older modes cannot win.
    geometryQueue.current = geometryQueue.current
      .then(() => !disposed && isWidget ? api.notchMode(nativeMode) : undefined)
      .then(() => {
        if (!disposed && !collapsed) {
          frame = requestAnimationFrame(() => {
            frame = requestAnimationFrame(() => {
              if (!disposed) { setSurfaceMode(nativeMode); setMotionReady(true); }
            });
          });
        }
      })
      .catch((e) => { if (!disposed) setError(String(e)); });
    return () => { disposed = true; cancelAnimationFrame(frame); };
  }, [nativeMode, collapsed, isWidget, edge, active.length, snapshot.config.settings.autoHide]);
  useEffect(() => {
    if (selected && !active.some((a) => a.id === selected)) {
      setSelected(null);
    }
  }, [active, selected]);
  useEffect(() => {
    // The native settings webview is mounted while hidden. Focusing a control
    // here can activate its window; only the browser modal needs focus transfer.
    if (!settingsOpen || nativeSettings) return;
    const dialog = overlayRef.current;
    const prev = document.activeElement as HTMLElement | null;
    const timer = setTimeout(
      () => dialog?.querySelector<HTMLButtonElement>("button")?.focus(),
      0,
    );
    return () => {
      clearTimeout(timer);
      prev?.focus();
    };
  }, [settingsOpen, nativeSettings]);
  async function run<T>(fn: () => Promise<T>, message?: string) {
    setError("");
    setNotice("");
    setBusy(true);
    try {
      await fn();
      if (message) setNotice(message);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  function cancelClose() {
    if (closeTimer.current) clearTimeout(closeTimer.current);
    if (foldTimer.current) clearTimeout(foldTimer.current);
    closeTimer.current = null;
    foldTimer.current = null;
    setClosing(false);
  }
  function leave() {
    // Blur and mouseleave can both arrive during one exit. Only a genuine
    // re-entry should reverse the animation, not a second departure event.
    if (!dragging.current && !closeTimer.current && !foldTimer.current)
      closeTimer.current = setTimeout(() => { closeTimer.current = null; fold(); }, 240);
  }
  function reveal() {
    cancelClose();
    setRailOpen(true);
  }
  function fold() {
    cancelClose();
    const finish = () => {
      foldTimer.current = null;
      setSelected(null);
      setMenu(false);
      setRailOpen(false);
      setClosing(false);
    };
    if (collapsed || !snapshot.config.settings.autoHide || matchMedia("(prefers-reduced-motion: reduce)").matches) finish();
    else {
      setClosing(true);
      // Keep both the rail and the native window alive throughout the exit.
      foldTimer.current = setTimeout(finish, 220);
    }
  }
  function openAccount(id: string) {
    cancelClose();
    setRailOpen(true);
    setMenu(false);
    setSelected(id);
  }
  function openSettings() {
    cancelClose();
    setMenu(false);
    setSelected(null);
    setRailOpen(false);
    if (isWidget) void run(() => api.windowAction("dashboard"));
    else setSettingsOpen(true);
  }
  function closeSettings() {
    if (nativeSettings) void run(() => api.windowAction("hide"));
    else {
      setSettingsOpen(false);
      settingsButton.current?.focus();
    }
  }
  function keyHandler(e: React.KeyboardEvent) {
    if (e.defaultPrevented) return;
    if (e.key === "Escape") {
      fold();
      if (settingsOpen) closeSettings();
    }
    if (e.key === "Tab" && settingsOpen) {
      const nodes = overlayRef.current?.querySelectorAll<HTMLElement>(
        'button:not(:disabled),input:not(:disabled),a[href],[tabindex="0"]',
      );
      if (nodes?.length) {
        const first = nodes[0],
          last = nodes[nodes.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    }
  }
  return (
    <Locale.Provider value={language}>
    <div className="notch-app" onKeyDown={keyHandler}>
      {!api.native && (
        <>
          <div className="preview-label">
            <span className="mini-notch-icon" />
            <span>
              CodeNotch <span className="preview-platform">{t("pour Windows")}</span>
            </span>
          </div>
          <div className="preview-caption">
            <span>
              {api.demo
                ? t("Aperçu interactif · données fictives")
                : t("Aperçu navigateur · aucune connexion aux comptes")}
            </span>
            <p>{t("Survolez la languette pour afficher vos comptes.")}</p>
            <button onClick={openSettings}>
              <Settings2 size={14} />{t("Réglages")}</button>
          </div>
        </>
      )}
      {!nativeSettings && snapshot.config.settings.widget && (
        <div
          className={`notch-position edge-${edge} ${vertical ? "vertical" : "horizontal"} ${selected ? "has-detail" : ""} ${collapsed ? "is-collapsed" : "is-open"} ${closing || collapsed || !motionReady ? "is-retracted" : "is-revealed"} ${surfaceMode === "detail" ? "is-detail-ready" : ""}`}
          style={{ "--cells": Math.max(1, active.length) } as CSSProperties}
          onMouseEnter={reveal}
          onMouseLeave={leave}
          onBlur={(e) => {
            if (!e.currentTarget.contains(e.relatedTarget) && !e.currentTarget.matches(":hover")) leave();
          }}
        >
            <button className="notch-peek" aria-label={t("Déplier l’encoche")} tabIndex={collapsed ? 0 : -1} aria-hidden={!collapsed} onFocus={() => { focusAfterOpen.current = true; reveal(); }} onClick={reveal}>
              <span className="peek-indicator" aria-hidden="true" />
            </button>
          {!collapsed && (
            <>
          <div
            className="notch-rail"
            aria-label={t("Comptes suivis")}
            onContextMenu={(e) => {
              e.preventDefault();
              setMenu((v) => !v);
              setSelected(null);
            }}
          >
            <div className="notch-cells">
              {active.map((a) => {
                const r = snapshot.readings.find((r) => r.accountId === a.id);
                const w = headline(r);
                return (
                  <button
                    key={a.id}
                    className={`provider-cell ${selected === a.id ? "selected" : ""}`}
                    aria-label={`${t(nameOf(a))}, ${a.name}, ${w ? percentage(w.usedPercent, language) + " " + t("utilisés") : t("aucun relevé")}`}
                    aria-expanded={selected === a.id}
                    onMouseEnter={() => openAccount(a.id)}
                    onFocus={() => openAccount(a.id)}
                    onClick={() => {
                      openAccount(a.id);
                    }}
                  >
                    <Ring
                      account={a}
                      reading={r}
                      interval={snapshot.config.settings.pollSeconds}
                    />
                    <span className="ring-percent">
                      {percentage(w?.usedPercent, language).replace(" %", "%")}
                    </span>
                  </button>
                );
              })}
              {!active.length && (
                <button
                  className="provider-cell empty-cell"
                  onClick={openSettings}
                  aria-label={t("Ajouter un compte")}
                >
                  <span className="empty-ring">
                    <Plus size={22} />
                  </span>
                  <span>{t("Ajouter")}</span>
                </button>
              )}
            </div>
          </div>
          <div className="notch-tools">
            {snapshot.config.settings.autoHide && (
              <button className="fold-button" aria-label={t("Replier l’encoche")} title={t("Replier")} onClick={fold}>
                <ChevronsRight size={16} />
              </button>
            )}
            <button
              ref={settingsButton}
              className="settings-orb"
              title={t("Réglages")}
              aria-label={t("Ouvrir les réglages")}
              onClick={openSettings}
            >
              <Settings2 size={17} />
            </button>
            {api.native && (
              <button
                className="grip"
                aria-label={t("Déplacer l’encoche")}
                onPointerDown={(e) => {
                  if (e.button !== 0) return;
                  cancelClose();
                  dragging.current = true;
                  void run(api.dragNotch).finally(() => {
                    dragging.current = false;
                    if (!document.querySelector('.notch-position')?.matches(':hover')) leave();
                  });
                }}
              >
                <GripVertical size={14} />
              </button>
            )}
          </div>
          {account && (
            <UsagePopover
              account={account}
              reading={reading}
              interval={snapshot.config.settings.pollSeconds}
              refresh={() => run(api.refresh)}
              busy={busy || snapshot.refreshing}
              onClose={() => {
                setSelected(null);
              }}
            />
          )}
          {menu && (
            <div className="notch-menu" role="menu">
              <button role="menuitem" onClick={openSettings}>
                <Settings2 size={15} />{t("Réglages")}</button>
              <button
                role="menuitem"
                disabled={!api.native || busy}
                onClick={() => {
                  setMenu(false);
                  void run(api.refresh);
                }}
              >
                <RefreshCw size={15} />{t("Actualiser")}</button>
            </div>
          )}
            </>
          )}
        </div>
      )}
      {!settingsOpen && (error || snapshot.storageError) && (
        <div className="floating-error" role="alert">
          <AlertCircle size={15} />
          {t(error || snapshot.storageError || "")}
          <button onClick={() => setError("")} aria-label={t("Fermer l’erreur")}>
            <X size={14} />
          </button>
        </div>
      )}
      {settingsOpen && (
        <div
          className="settings-overlay"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget && !nativeSettings)
              closeSettings();
          }}
        >
          <div
            className="settings-window"
            role="dialog"
            aria-modal="true"
            aria-label={t("Réglages CodeNotch")}
            ref={overlayRef}
          >
            <header className="settings-titlebar" data-tauri-drag-region>
              <span data-tauri-drag-region>
                <span className="mini-notch-icon" />
                CodeNotch <span className="windows-label">Windows</span>
              </span>
              <div>
                {api.native && (
                  <button
                    aria-label={t("Réduire")}
                    onClick={() => run(() => api.windowAction("minimize"))}
                  >
                    <Minus size={15} />
                  </button>
                )}
                <button
                  aria-label={t("Fermer les réglages")}
                  onClick={closeSettings}
                >
                  <X size={17} />
                </button>
              </div>
            </header>
            <div className="settings-layout">
              <nav className="settings-nav" aria-label={t("Réglages")}>
                <button
                  className={page === "accounts" ? "active" : ""}
                  onClick={() => {
                    setPage("accounts");
                    setNotice("");
                  }}
                >
                  <UsersRound size={17} />{t("Comptes")}<span>{active.length}</span>
                </button>
                <button
                  className={page === "appearance" ? "active" : ""}
                  onClick={() => {
                    setPage("appearance");
                    setNotice("");
                  }}
                >
                  <Settings2 size={17} />{t("Apparence et alertes")}</button>
                <button
                  className={page === "help" ? "active" : ""}
                  onClick={() => {
                    setPage("help");
                    setNotice("");
                  }}
                >
                  <Info size={17} />{t("À propos")}</button>
                <div className="settings-nav-footer">
                  <span>{t("Adaptation Windows")}</span>
                  <span>{t("v0.1.0 · locale")}</span>
                </div>
              </nav>
              <div className="settings-body">
                {(error || snapshot.storageError) && (
                  <div className="message error" role="alert">
                    <AlertCircle size={16} />
                    <span>{t(error || snapshot.storageError || "")}</span>
                  </div>
                )}
                {notice && (
                  <div className="message success" role="status">
                    <Check size={16} />
                    {t(notice)}
                  </div>
                )}
                {page === "accounts" ? (
                  <Accounts
                    error={error}
                    onEdit={() => { setError(""); setNotice(""); }}
                    snapshot={snapshot}
                    busy={busy}
                    run={run}
                    onSaved={setSnapshot}
                  />
                ) : page === "appearance" ? (
                  <Preferences
                    settings={snapshot.config.settings}
                    busy={busy}
                    run={run}
                    onSaved={setSnapshot}
                  />
                ) : (
                  <Help />
                )}
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
    </Locale.Provider>
  );
}
