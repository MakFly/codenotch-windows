import { useRef, useState, type FormEvent } from "react";
import {
  Plus,
  X,
  Search,
  UsersRound,
  ChevronRight,
  ShieldCheck,
  LoaderCircle,
  Check,
  Trash2,
  Monitor,
  RefreshCw,
  Bell,
  ExternalLink,
} from "lucide-react";
import {
  blankAccount,
  providers,
  type Account,
  type Settings,
  type Snapshot,
} from "./lib/types";
import { Mark, Toggle, Choice, SelectItem } from "./components/Controls";
import { Dialog, DialogContent, DialogTitle, DialogDescription } from "./components/ui/dialog";
import * as api from "./lib/bridge";
import { useLocale } from "./lib/Locale";
const provider = (id: string) => providers.find((p) => p.id === id)!;
type Run = <T>(fn: () => Promise<T>, message?: string) => Promise<void>;
export function Accounts({
  snapshot,
  busy,
  run,
  onSaved,
  error,
  onEdit,
}: {
  snapshot: Snapshot;
  busy: boolean;
  run: Run;
  onSaved: (s: Snapshot) => void;
  error: string;
  onEdit: () => void;
}) {
  const { t } = useLocale();
  const [editing, setEditing] = useState<Account | null>(null);
  const [secret, setSecret] = useState("");
  const [clear, setClear] = useState(false);
  const [remove, setRemove] = useState("");
  const opener = useRef<HTMLElement | null>(null);
  const addButton = useRef<HTMLButtonElement>(null);
  const dialog = useRef<HTMLDivElement>(null);
  function edit(a: Account) {
    opener.current = document.activeElement as HTMLElement;
    onEdit();
    setEditing({ ...a });
    setSecret("");
    setClear(false);
    setRemove("");
  }
  const change = (key: keyof Account, value: string | boolean) =>
    setEditing((a) => (a ? { ...a, [key]: value } : a));
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!editing) return;
    await run(async () => {
      onSaved(await api.saveAccount(editing, secret, clear));
      setEditing(null);
      setSecret("");
    }, t("Compte enregistré. Le suivi démarrera automatiquement."));
  }
  return (
    <>
      <div className="page-heading">
        <div>
          <h1>{t("Comptes")}</h1>
          <p>{t("Choisissez les comptes affichés dans l’encoche.")}</p>
        </div>
        <button ref={addButton} className="button primary" disabled={busy} onClick={() => edit(blankAccount())}>
          <Plus size={16} />{t("Ajouter")}</button>
      </div>
      <div className="account-config-layout">
        <section className="configuration-list">
          <div className="list-header">
            <h2>{t("Comptes configurés")}</h2>
            <button
              className="text-button"
              disabled={busy || !api.native}
              onClick={() =>
                run(
                  async () => onSaved(await api.discoverAccounts()),
                  t("Recherche terminée. Les profils locaux disponibles ont été ajoutés."),
                )
              }
            >
              <Search size={15} />{t("Détecter")}</button>
          </div>
          {!snapshot.config.accounts.length && (
            <div className="empty-state">
              <UsersRound size={28} />
              <h3>{t("Aucun compte configuré")}</h3>
              <p>{t("Ajoute un fournisseur ou détecte les profils Codex et Claude déjà connectés sur ce PC.")}</p>
            </div>
          )}
          {snapshot.config.accounts.map((a) => (
            <div
              className={`config-account ${editing?.id === a.id ? "editing" : ""}`}
              key={a.id}
            >
              <button className="config-account-main" disabled={busy} onClick={() => edit(a)}>
                <Mark type={a.provider} />
                <div>
                  <strong>{a.name}</strong>
                  <span>{t(provider(a.provider).name)}</span>
                </div>
                <ChevronRight size={16} />
              </button>
              <Toggle
                checked={a.enabled}
                label={t("Activer {name}", { name: a.name })}
                disabled={busy}
                onChange={(enabled) =>
                  run(async () =>
                    onSaved(
                      await api.saveAccount({ ...a, enabled }, "", false),
                    ),
                  )
                }
              />
            </div>
          ))}
          <div className="connector-note">
            <ShieldCheck size={18} />
            <p>{t("Les sessions existantes restent dans leur outil d’origine. Les secrets ajoutés ici sont conservés dans le Gestionnaire d’identification Windows.")}</p>
          </div>
        </section>
        <Dialog open={!!editing} onOpenChange={(open) => { if (!open && !busy) { setEditing(null); setSecret(""); } }}>
        {editing && (
          <DialogContent ref={dialog} tabIndex={-1}
            onOpenAutoFocus={(e) => { e.preventDefault(); dialog.current?.focus(); }}
            onCloseAutoFocus={(e) => { e.preventDefault(); (opener.current?.isConnected ? opener.current : addButton.current)?.focus(); }}
            onPointerDownOutside={(e) => e.preventDefault()}
            onEscapeKeyDown={(e) => { if (busy) e.preventDefault(); e.stopPropagation(); }}
            onKeyDown={(e) => e.stopPropagation()}
          >
          <form className="account-form" onSubmit={submit} aria-busy={busy}>
            <div className="dialog-heading">
              <Mark type={editing.provider} />
              <div>
                <DialogTitle>{editing.id ? t("Configurer le compte") : t("Nouveau compte")}</DialogTitle>
                <DialogDescription>{editing.id ? editing.name : t("Ajoutez un compte à votre encoche.")}</DialogDescription>
              </div>
              <button
                type="button"
                className="icon-button"
                aria-label={t("Fermer le formulaire")}
                disabled={busy}
                onClick={() => { setEditing(null); setSecret(""); }}
              >
                <X size={18} />
              </button>
            </div>
            <div className="account-form-scroll">
            <fieldset disabled={busy} className="account-fields">
            <label htmlFor="account-provider">{t("Fournisseur")}</label>
            <Choice
              id="account-provider"
              value={editing.provider}
              onChange={(value) => {
                change("provider", value);
                change(
                  "endpoint",
                  value === "ollama" ? "http://127.0.0.1:11434/api/ps" : "",
                );
              }}
            >
              {providers.map((p) => (
                <SelectItem key={p.id} value={p.id}>
                  {t(p.name)}
                </SelectItem>
              ))}
            </Choice>
            <p className="field-help">
              {t(provider(editing.provider).description)}
            </p>
            <label htmlFor="account-name">{t("Nom du compte")}</label>
            <input
              id="account-name"
              required
              maxLength={80}
              placeholder={t("Ex. Personnel, travail, projet…")}
              value={editing.name}
              onChange={(e) => change("name", e.target.value)}
            />
            {["codex", "claude"].includes(editing.provider) && (
              <>
                <label htmlFor="profile-path">{t("Dossier du profil")}{" "}<span>{t("facultatif")}</span>
                </label>
                <input
                  id="profile-path"
                  placeholder={
                    editing.provider === "codex"
                      ? t("Par défaut : %USERPROFILE%\\.codex")
                      : t("Par défaut : %USERPROFILE%\\.claude")
                  }
                  value={editing.profilePath}
                  onChange={(e) => change("profilePath", e.target.value)}
                />
                <p className="field-help">{t("Chemin absolu du profil déjà connecté, par exemple C:\\Users\\Kev\\.")}{editing.provider}{t("-travail.")}</p>
              </>
            )}
            {["custom", "ollama"].includes(editing.provider) && (
              <>
                <label htmlFor="account-endpoint">{t("URL du relevé JSON")}</label>
                <input
                  id="account-endpoint"
                  type="url"
                  required
                  placeholder="https://mon-service.fr/usage"
                  value={editing.endpoint}
                  onChange={(e) => change("endpoint", e.target.value)}
                />
                <p className="field-help">{t("HTTPS, ou HTTP sur localhost. Le format attendu est détaillé dans l’aide.")}</p>
              </>
            )}
            {editing.provider !== "codex" && (
              <details className="advanced-connection" key={editing.provider}
                open={editing.provider === "copilot" || editing.provider === "custom" || editing.hasSecret || !!editing.secretEnv}>
                <summary>{t("Jeton et options avancées")}</summary>
                <div className="account-fields">
                <label htmlFor="account-secret">{t("Jeton d’accès")}{" "}
                  <span>
                    {editing.hasSecret ? t("déjà enregistré") : t("facultatif")}
                  </span>
                </label>
                <input
                  id="account-secret"
                  type="password"
                  autoComplete="new-password"
                  disabled={!api.native}
                  placeholder={
                    api.native
                      ? editing.hasSecret
                        ? t("Laisser vide pour conserver le secret")
                        : t("Bearer token / jeton OAuth")
                      : t("Disponible dans l’application Windows")
                  }
                  value={secret}
                  onChange={(e) => setSecret(e.target.value)}
                />
                {editing.hasSecret && (
                  <label className="checkbox-label">
                    <input
                      type="checkbox"
                      checked={clear}
                      onChange={(e) => setClear(e.target.checked)}
                    />{t("Effacer le secret enregistré")}</label>
                )}
                <label htmlFor="secret-env">{t("Ou une variable d’environnement")}{" "}<span>{t("facultatif")}</span>
                </label>
                <input
                  id="secret-env"
                  placeholder={
                    editing.provider === "copilot" ? "GH_TOKEN" : t("MON_TOKEN_IA")
                  }
                  value={editing.secretEnv}
                  onChange={(e) => change("secretEnv", e.target.value)}
                  pattern="[A-Za-z0-9_]*"
                />
                <p className="field-help">{t("Un secret enregistré est prioritaire. Relance l’application après avoir modifié l’environnement Windows.")}</p>
                </div>
              </details>
            )}
            </fieldset>
            {editing.id && (
              <div className="delete-section">
                {remove === editing.id ? (
                  <>
                    <p>{t("Retirer ce compte de CodeNotch ? Sa connexion dans le fournisseur sera conservée.")}</p>
                    <button
                      type="button"
                      className="button danger"
                      disabled={busy}
                      onClick={() =>
                        run(async () => {
                          onSaved(await api.removeAccount(editing.id));
                          setEditing(null);
                        }, t("Compte retiré de CodeNotch."))
                      }
                    >{t("Confirmer le retrait")}</button>
                    <button
                      type="button"
                      className="text-button"
                      onClick={() => setRemove("")}
                    >{t("Garder le compte")}</button>
                  </>
                ) : (
                  <button
                    type="button"
                    className="text-button danger-text"
                    disabled={busy}
                    onClick={() => setRemove(editing.id)}
                  >
                    <Trash2 size={15} />{t("Retirer ce compte")}</button>
                )}
              </div>
            )}
            </div>
            <div className="dialog-footer">
              {error && <p className="message error" role="alert">{t(error)}</p>}
              <div className="form-actions">
                <button className="button secondary" type="button" disabled={busy} onClick={() => { setEditing(null); setSecret(""); }}>{t("Annuler")}</button>
                <button className="button primary" type="submit" disabled={busy}>
                  {busy ? <LoaderCircle className="spin" size={16} /> : <Check size={16} />}
                  {busy ? t("Enregistrement…") : t("Enregistrer")}
                </button>
              </div>
            </div>
          </form>
          </DialogContent>
        )}
        </Dialog>
          <section className="connectors-catalog">
            <h2>{t("Ajouter un fournisseur")}</h2>
            <p>{t("Plusieurs comptes du même fournisseur ? Ajoute simplement un profil pour chacun.")}</p>
            {providers.map((p) => (
              <button
                className="catalog-row"
                key={p.id}
                disabled={busy}
                onClick={() =>
                  edit({
                    ...blankAccount(),
                    provider: p.id,
                    endpoint:
                      p.id === "ollama" ? "http://127.0.0.1:11434/api/ps" : "",
                  })
                }
              >
                <Mark type={p.id} />
                <div>
                  <strong>{t(p.name)}</strong>
                  <span>{t(p.description)}</span>
                </div>
                <Plus size={17} />
              </button>
            ))}
          </section>
      </div>
    </>
  );
}

export function Preferences({
  settings,
  busy,
  run,
  onSaved,
}: {
  settings: Settings;
  busy: boolean;
  run: Run;
  onSaved: (s: Snapshot) => void;
}) {
  const { t } = useLocale();
  const update = (patch: Partial<Settings>) =>
    run(async () => onSaved(await api.saveSettings({ ...settings, ...patch })));
  return (
    <>
      <div className="page-heading">
        <div>
          <h1>{t("Apparence et alertes")}</h1>
          <p>{t("Position, fréquence des relevés et notifications.")}</p>
        </div>
      </div>
      <div className="settings-layout">
        <section className="settings-panel">
          <h2>{t("Langue")}</h2>
          <div className="setting-row">
            <div>
              <label htmlFor="interface-language">{t("Langue de l’interface")}</label>
              <p>{t("S’applique immédiatement et reste enregistrée sur ce PC.")}</p>
            </div>
            <Choice id="interface-language" value={settings.language ?? "en"} disabled={busy} onChange={(language) => update({ language: language as Settings["language"] })}>
              <SelectItem value="en">English</SelectItem>
              <SelectItem value="fr">Français</SelectItem>
            </Choice>
          </div>
          <h2>
            <Monitor size={20} />{t("Encoche")}</h2>
          <div className="setting-row">
            <div>
              <strong>{t("Afficher l’encoche")}</strong>
              <p>{t("Les quotas restent visibles au bord de l’écran. Survolez un anneau pour afficher les détails.")}</p>
            </div>
            <Toggle
              checked={settings.widget}
              onChange={(widget) => update({ widget })}
              label={t("Afficher l’encoche")}
              disabled={busy}
            />
          </div>
          <div className="setting-row">
            <div>
              <strong>{t("Replier automatiquement")}</strong>
              <p>{t("Une languette discrète au repos. Survolez-la pour déplier les comptes ; éloignez la souris pour les masquer.")}</p>
            </div>
            <Toggle checked={settings.autoHide} onChange={(autoHide) => update({ autoHide })} label={t("Replier automatiquement")} disabled={busy} />
          </div>
          <div className="setting-row">
            <div>
              <label htmlFor="widget-position">{t("Position de départ")}</label>
              <p>{t("La poignée mémorise ta position et ton écran. Choisir un autre bord réinitialise ce placement.")}</p>
            </div>
            <Choice
              disabled={busy}
              id="widget-position"
              value={settings.widgetPosition}
              onChange={(v) =>
                update({ widgetPosition: v as Settings["widgetPosition"] })
              }
            >
              <SelectItem value="right">{t("À droite")}</SelectItem>
              <SelectItem value="left">{t("À gauche")}</SelectItem>
              <SelectItem value="top">{t("En haut")}</SelectItem>
              <SelectItem value="bottom">{t("En bas")}</SelectItem>
            </Choice>
          </div>
          <div className="setting-row">
            <div>
              <strong>{t("Masquer en plein écran")}</strong>
              <p>{t("Masque l’encoche et ses alertes pendant un jeu ou une vidéo en plein écran, même sans bordure. Retour automatique après Alt+Tab.")}</p>
            </div>
            <Toggle checked={settings.hideInFullscreen} onChange={(hideInFullscreen) => update({ hideInFullscreen })} label={t("Masquer en plein écran")} disabled={busy} />
          </div>
          <div className="setting-row">
            <div>
              <strong>{t("Mode jeu")}</strong>
              <p>{t("Masque l’encoche et ses alertes jusqu’à désactivation. Pour les jeux en fenêtre, active-le depuis l’icône près de l’horloge.")}</p>
            </div>
            <Toggle checked={settings.gameMode} onChange={(gameMode) => update({ gameMode })} label={t("Mode jeu")} disabled={busy} />
          </div>
          <h2>
            <RefreshCw size={20} />{t("Actualisation")}</h2>
          <div className="setting-row">
            <div>
              <label htmlFor="poll-interval">{t("Fréquence des relevés")}</label>
              <p>{t("Claude respecte un minimum de 2 minutes. Les délais imposés par les fournisseurs sont prioritaires.")}</p>
            </div>
            <Choice
              disabled={busy}
              id="poll-interval"
              value={String(settings.pollSeconds)}
              onChange={(v) => update({ pollSeconds: Number(v) })}
            >
              {[30, 60, 120, 300].map((n) => (
                <SelectItem key={n} value={String(n)}>
                  {n < 60
                    ? t("{count} secondes", { count: n })
                    : t(n === 60 ? "{count} minute" : "{count} minutes", { count: n / 60 })}
                </SelectItem>
              ))}
            </Choice>
          </div>
          <h2>
            <Bell size={20} />{t("Alertes")}</h2>
          <div className="setting-row">
            <div>
              <strong>{t("Notifications de quota")}</strong>
              <p>{t("Une notification au franchissement du seuil, sans répétition tant qu’il reste dépassé.")}</p>
            </div>
            <Toggle
              checked={settings.notifications}
              onChange={(notifications) => update({ notifications })}
              label={t("Notifications de quota")}
              disabled={busy}
            />
          </div>
          <div className="setting-row">
            <div>
              <label htmlFor="alert-percent">{t("Me prévenir à")}</label>
              <p>{t("Pourcentage consommé d’une fenêtre de quota.")}</p>
            </div>
            <Choice
              disabled={busy}
              id="alert-percent"
              value={String(settings.alertPercent)}
              onChange={(v) => update({ alertPercent: Number(v) })}
            >
              {[50, 70, 80, 90, 95, 100].map((n) => (
                <SelectItem key={n} value={String(n)}>
                  {n} %
                </SelectItem>
              ))}
            </Choice>
          </div>
        </section>
        <aside className="settings-aside">
          <h3>{t("En arrière-plan")}</h3>
          <p>{t("Fermer la fenêtre laisse le suivi actif. Pour arrêter complètement CodeNotch, choisis « Quitter » depuis l’icône près de l’horloge.")}</p>
          <span>
            <ShieldCheck size={16} />{t("Préférences enregistrées localement")}</span>
        </aside>
      </div>
    </>
  );
}

export function Help() {
  const { t } = useLocale();
  return (
    <>
      <div className="page-heading">
        <div>
          <h1>{t("À propos des relevés")}</h1>
          <p>{t("Ce que CodeNotch lit, et ce que chaque mesure raconte.")}</p>
        </div>
      </div>
      <div className="help-content">
        <section>
          <h2>{t("Quota et tokens : deux repères différents.")}</h2>
          <p>{t("Un quota à 40 % signifie que le fournisseur indique 40 % de consommation sur une période donnée. Il ne permet pas de déduire un nombre de tokens restants. Les tokens s’affichent uniquement quand la source communique ce nombre.")}</p>
        </section>
        <section>
          <h2>{t("Connecter tes outils")}</h2>
          <div className="help-provider">
            <strong>Codex / ChatGPT</strong>
            <p>{t("Connecte-toi avec Codex CLI ou l’application Codex. CodeNotch lit les quotas via")}{" "}<code>codex app-server</code>{t(", ainsi que l’activité en tokens si ta version la prend en charge. Une clé API seule ne donne pas accès aux quotas de l’abonnement ChatGPT. Les jours d’activité sont affichés en UTC.")}</p>
          </div>
          <div className="help-provider">
            <strong>Claude</strong>
            <p>{t("Connecte Claude Code sur Windows. Le connecteur lit le fichier")}{" "}
              <code>.claude/.credentials.json</code>{" "}{t("ou le jeton OAuth configuré. Les sessions expirées sont renouvelées par le CLI Claude Code installé, sans envoyer de prompt. Si le renouvellement échoue, reconnectez Claude Code avec « claude auth login ». L’endpoint de quota est interne et peut changer.")}</p>
          </div>
          <div className="help-provider">
            <strong>GitHub Copilot</strong>
            <p>{t("Configure un jeton GitHub disposant de l’accès Copilot, ou la variable")}{" "}<code>GH_TOKEN</code>{t(". Le connecteur utilise l’endpoint interne de quota des éditeurs. Tous les jetons GitHub ne sont pas autorisés à le lire.")}</p>
          </div>
          <div className="help-provider">
            <strong>Ollama</strong>
            <p>{t("Le serveur doit être lancé. L’URL par défaut est")}{" "}
              <code>http://127.0.0.1:11434/api/ps</code>{t(". Le relevé indique les modèles chargés, pas une consommation de tokens.")}</p>
          </div>
        </section>
        <section>
          <h2>{t("Tout autre fournisseur : un endpoint JSON")}</h2>
          <p>{t("Ajoute « Endpoint personnalisé » pour Gemini, OpenRouter, un proxy interne ou un autre outil. L’URL doit retourner le format ci-dessous. Ce n’est pas un connecteur natif universel : une API de chat standard ne fournit pas ces données automatiquement.")}</p>
          <pre>{`{
  "plan": "${t("Mon abonnement")}",
  "windows": [
    {
      "label": "${t("Quota mensuel")}",
      "usedPercent": 42,
      "resetsAt": "2026-11-01T00:00:00Z"
    }
  ],
  "tokensToday": 12345,
  "lifetimeTokens": 250000,
  "daily": [{ "date": "2026-10-24", "tokens": 12345 }]
}`}</pre>
          <p>{t("Exemple illustratif. Les mesures inconnues doivent être omises ou nulles.")}{" "}<code>resetsAt</code>{" "}{t("accepte une date ISO ou un timestamp Unix en secondes. Au moins une mesure est nécessaire. Le jeton optionnel est transmis en Bearer uniquement à l’URL configurée ; les redirections sont refusées.")}</p>
        </section>
        <section>
          <h2>{t("Local, lisible, sans surprises.")}</h2>
          <p>{t("Pas de serveur CodeNotch, pas de télémétrie, pas d’inférence générée pour mesurer l’usage. La configuration est enregistrée dans le dossier de configuration de l’application. Les secrets ajoutés sont stockés dans le Gestionnaire d’identification Windows. Les relevés sont conservés en mémoire pendant la session ; les dates permettent de repérer une valeur ancienne.")}</p>
          <p>{t("Les quotas sont interrogés périodiquement : il peut donc y avoir un décalage avec le fournisseur. L’activité n’est jamais présentée comme une donnée en direct quand elle ne l’est pas.")}</p>
        </section>
        <section className="credits">
          <h2>{t("Référence")}</h2>
          <p>
            <a
              href="https://github.com/vinzdg/codenotch"
              target="_blank"
              rel="noreferrer"
            >
              CodeNotch <ExternalLink size={13} />
            </a>{" "}{t("est la référence visuelle et fonctionnelle. Cette adaptation Windows est un projet indépendant, développé avec Tauri.")}</p>
        </section>
      </div>
    </>
  );
}
