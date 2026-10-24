import type { Reading } from "./types";
import type { Language } from "./i18n";
export function tokens(n: number | null | undefined, compact = false, language: Language = "en") {
  return n == null
    ? "—"
    : new Intl.NumberFormat(language === "fr" ? "fr-FR" : "en-US", {
        notation: compact ? "compact" : "standard",
        maximumFractionDigits: compact ? 1 : 0,
      }).format(n);
}
export function percentage(n: number | undefined, language: Language = "en") {
  return n == null || !Number.isFinite(n)
    ? "—"
    : `${new Intl.NumberFormat(language === "fr" ? "fr-FR" : "en-US", { maximumFractionDigits: 1 }).format(n)} %`;
}
export function resetTime(timestamp: number | null, now = Date.now(), language: Language = "en") {
  if (timestamp === null) return language === "fr" ? "Date non communiquée" : "Reset time unavailable";
  const minutes = Math.ceil((timestamp * 1000 - now) / 60000);
  if (minutes <= 0) return language === "fr" ? "Renouvellement à vérifier" : "Reset needs checking";
  if (minutes < 60) return language === "fr" ? `dans ${minutes} min` : `in ${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours >= 24) return language === "fr" ? `dans ${Math.floor(hours / 24)} j ${hours % 24} h` : `in ${Math.floor(hours / 24)} d ${hours % 24} h`;
  return language === "fr" ? `dans ${hours} h ${minutes % 60} min` : `in ${hours} h ${minutes % 60} min`;
}
export function freshness(
  timestamp: number | null | undefined,
  now = Date.now(),
  language: Language = "en",
) {
  if (!timestamp) return language === "fr" ? "Aucun relevé" : "No reading";
  const seconds = Math.max(0, Math.floor((now - timestamp * 1000) / 1000));
  if (seconds < 10) return language === "fr" ? "À l’instant" : "Just now";
  if (seconds < 60) return language === "fr" ? `Il y a ${seconds} s` : `${seconds} s ago`;
  if (seconds < 3600) return language === "fr" ? `Il y a ${Math.floor(seconds / 60)} min` : `${Math.floor(seconds / 60)} min ago`;
  return language === "fr" ? `Il y a ${Math.floor(seconds / 3600)} h` : `${Math.floor(seconds / 3600)} h ago`;
}
export function isFresh(
  reading: Reading | undefined,
  interval: number,
  now = Date.now(),
) {
  return (
    reading?.status === "ok" &&
    reading.updatedAt != null &&
    now / 1000 - reading.updatedAt <= Math.max(interval * 2 + 30, 270)
  );
}
export function headline(reading: Reading | undefined) {
  return reading?.windows.reduce<Reading["windows"][number] | undefined>(
    (highest, window) =>
      !highest || window.usedPercent > highest.usedPercent ? window : highest,
    undefined,
  );
}
export const statusLabels: Record<Reading["status"], string> = {
  ok: "À jour",
  needsAuth: "À connecter",
  error: "Indisponible",
  stale: "Ancien relevé",
  rateLimited: "En attente",
};
