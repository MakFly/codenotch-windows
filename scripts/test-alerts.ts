import { chromium, expect } from '@playwright/test';
import { emptySnapshot } from '../src/lib/types';

const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 860, height: 900 } });
  page.setDefaultTimeout(8000);
  const errors: string[] = [];
  page.on('pageerror', e => errors.push(e.message));
  const snapshot = structuredClone(emptySnapshot);
  snapshot.config.settings.language = 'fr';
  await page.addInitScript(snapshot => {
    const host = window as any;
    host.isTauri = true;
    host.calls = [];
    host.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    host.__TAURI_INTERNALS__ = {
      transformCallback: () => 1, unregisterCallback: () => {},
      invoke: async (command: string, args: any) => {
        host.calls.push([command, args]);
        if (command === 'get_snapshot') return snapshot;
        if (command === 'save_settings') {
          snapshot.config.settings = args.settings;
          return structuredClone(snapshot);
        }
        if (command === 'save_telegram_token') {
          snapshot.telegramTokenSaved = !!args.token;
          return structuredClone(snapshot);
        }
        if (command === 'save_discord_webhook') {
          snapshot.discordWebhookSaved = !!args.url;
          return structuredClone(snapshot);
        }
        if (command === 'detect_telegram_chats')
          return [{ id: '111', name: 'Kev' }, { id: '-1002', name: 'Agents' }];
        if (command === 'provider_status') return [];
        return null;
      },
    };
  }, snapshot);
  await page.goto('http://127.0.0.1:1437/');
  await page.getByRole('button', { name: 'Alertes', exact: true }).click();

  // Discord comes first and open; Telegram is shown but switched off for now.
  const sections = page.locator('.accordion-trigger');
  const telegramTrigger = page.locator('.accordion-trigger', { hasText: 'Telegram' });
  await expect(sections.first()).toContainText('Discord');
  await expect(sections.first()).toHaveAttribute('aria-expanded', 'true');
  await expect(telegramTrigger).toContainText('Bientôt disponible');
  await expect(telegramTrigger).toBeDisabled();
  await expect(page.getByLabel('Jeton du bot')).toHaveCount(0);
  await expect(page.getByLabel('Seuil critique', { exact: true })).toHaveCount(0);
  await page.screenshot({ path: 'test-results/alerts-collapsed.png' });

  // Discord: webhook stored write-only, then a test card.
  const discord = page.getByRole('region', { name: /Discord/ });
  await discord.getByLabel('URL du webhook').fill('https://discord.com/api/webhooks/123/abcdefghijklmnopqrstuvwxyz0123');
  await discord.getByRole('button', { name: 'Enregistrer', exact: true }).click();
  await expect(sections.first()).toContainText('Configuré');
  await page.getByRole('switch', { name: 'Alertes Discord' }).click();
  await expect(sections.first()).toContainText('Actif');
  await discord.getByRole('button', { name: 'Envoyer un test' }).click();
  await expect(page.getByText('Message de test envoyé. Regarde Discord.')).toBeVisible();

  // Thresholds and forecast live in their own sections.
  await page.getByRole('button', { name: /Seuils et notifications Windows/ }).click();
  // Only one section stays open: opening the thresholds folds Discord.
  await expect(sections.first()).toHaveAttribute('aria-expanded', 'false');
  await expect(page.getByLabel('Seuil critique', { exact: true })).toHaveText('95 %');
  await expect(page.getByRole('switch', { name: 'Alertes urgentes en plein écran' })).toHaveAttribute('aria-checked', 'true');
  await page.getByRole('button', { name: /Prévision du plafond/ }).click();
  await expect(page.getByRole('switch', { name: 'Prévenir avant le plafond' })).toHaveAttribute('aria-checked', 'true');
  await expect(page.getByLabel('Anticipation', { exact: true })).toHaveText('30 minutes');
  await page.screenshot({ path: 'test-results/alerts-open.png', fullPage: true });

  const calls: [string, any][] = await page.evaluate(() => (window as any).calls);
  const saved = calls.filter(([c]) => c === 'save_settings').map(([, a]) => a.settings);
  expect(saved.at(-1).discordEnabled).toBe(true);
  expect(calls.some(([c]) => c === 'test_discord')).toBe(true);
  expect(calls.some(([c]) => c.includes('telegram'))).toBe(false);
  // Secrets never come back to the page.
  const html = await page.evaluate(() => document.body.innerHTML);
  expect(html).not.toContain('abcdefghijklmnopqrstuvwxyz0123');
  expect(errors).toEqual([]);
  console.log('PASS: collapsible alerts (Discord first, Telegram coming soon), Discord webhook write-only and test, single open section, thresholds and forecast.');
} finally {
  await browser.close();
}
