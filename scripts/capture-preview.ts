// Documentation screenshots use browser-only sample data, never local accounts.
import { chromium, expect } from '@playwright/test';
import { mkdir } from 'node:fs/promises';

await mkdir('docs/images', { recursive: true });
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1100, height: 850 }, reducedMotion: 'reduce' });
  await page.goto('http://127.0.0.1:1437/?demo=1&lang=en');
  await page.getByRole('button', { name: 'Expand notch', exact: true }).hover();
  await page.getByRole('button', { name: /Claude, / }).hover();
  const detail = page.getByRole('region', { name: 'Claude usage' });
  await expect(detail).toHaveCSS('opacity', '1');
  const boxes = await Promise.all([detail.boundingBox(), page.locator('.notch-rail').boundingBox()]);
  if (boxes.some(box => !box)) throw new Error('Preview geometry is unavailable.');
  const left = Math.max(0, Math.floor(Math.min(...boxes.map(box => box!.x)) - 18));
  const top = Math.max(0, Math.floor(Math.min(...boxes.map(box => box!.y)) - 18));
  const right = Math.min(1100, Math.ceil(Math.max(...boxes.map(box => box!.x + box!.width)) + 18));
  const bottom = Math.min(850, Math.ceil(Math.max(...boxes.map(box => box!.y + box!.height)) + 18));
  await page.screenshot({ path: 'docs/images/notch.png', clip: { x: left, y: top, width: right-left, height: bottom-top } });

  // Settings screens use a mocked native bridge with fictional accounts and paths.
  const native = await browser.newPage({ viewport: { width: 860, height: 690 }, reducedMotion: 'reduce' });
  await native.addInitScript(() => {
    const host = window as any;
    const settings = {
      language: 'en', pollSeconds: 60, notifications: true, alertPercent: 80, widget: true, autoHide: true,
      hideInFullscreen: true, gameMode: false, widgetPlacement: null, widgetPosition: 'right', onboardingDone: false,
      criticalPercent: 95, forecastAlerts: true, forecastMinutes: 30, resetAlerts: true, criticalInFullscreen: true,
      telegramEnabled: false, telegramChatId: '', discordEnabled: true,
    };
    const account = (id: string, provider: string, name: string) => ({
      id, provider, name, enabled: true, profilePath: '', endpoint: '', secretEnv: '', hasSecret: false,
    });
    const snapshot = {
      config: { settings, accounts: [account('codex', 'codex', 'Codex · personal'), account('claude', 'claude', 'Claude · personal')] },
      readings: [], refreshing: false, storageError: null, telegramTokenSaved: false, discordWebhookSaved: true,
    };
    const statuses = [
      { provider: 'codex', installed: true, cliPath: 'C:\\Users\\you\\AppData\\Local\\Programs\\OpenAI\\Codex\\bin\\codex.exe', signedIn: true, signedInAt: 1, profilePath: 'C:\\Users\\you\\.codex' },
      { provider: 'claude', installed: true, cliPath: 'C:\\Users\\you\\.local\\bin\\claude.exe', signedIn: true, signedInAt: 1, profilePath: 'C:\\Users\\you\\.claude' },
      { provider: 'gemini', installed: false, cliPath: null, signedIn: false, signedInAt: null, profilePath: 'C:\\Users\\you\\.gemini' },
      { provider: 'grok', installed: true, cliPath: 'C:\\Users\\you\\.grok\\bin\\grok.exe', signedIn: false, signedInAt: null, profilePath: 'C:\\Users\\you\\.grok' },
      { provider: 'opencode', installed: false, cliPath: null, signedIn: false, signedInAt: null, profilePath: 'C:\\Users\\you\\.local\\share\\opencode' },
    ];
    host.isTauri = true;
    host.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    host.__TAURI_INTERNALS__ = {
      transformCallback: () => 1, unregisterCallback: () => {},
      invoke: async (command: string, args: any) => {
        if (command === 'get_snapshot') return snapshot;
        if (command === 'provider_status') return args?.provider ? statuses.filter(s => s.provider === args.provider) : statuses;
        if (command === 'plugin:app|version') return '0.1.3';
        if (command === 'save_settings') { snapshot.config.settings = args.settings; return structuredClone(snapshot); }
        return null;
      },
    };
  });
  await native.goto('http://127.0.0.1:1437/');
  await expect(native.getByRole('heading', { name: 'Connect your AI subscriptions' })).toBeVisible();
  await expect(native.getByText('Signed in · tracked in the notch').first()).toBeVisible();
  await native.locator('.settings-window').screenshot({ path: 'docs/images/welcome.png' });
  await native.getByRole('button', { name: 'Later', exact: true }).click();
  await expect(native.getByRole('heading', { name: 'Accounts', exact: true })).toBeVisible();
  await native.locator('.settings-window').screenshot({ path: 'docs/images/accounts.png' });
  await native.getByRole('button', { name: 'Alerts', exact: true }).click();
  await expect(native.locator('.accordion-trigger', { hasText: 'Discord' })).toContainText('Active');
  await native.locator('.settings-window').screenshot({ path: 'docs/images/alerts.png' });
  await native.getByRole('button', { name: 'Appearance', exact: true }).click();
  await expect(native.getByText('Language and welcome')).toBeVisible();
  await native.locator('.settings-window').screenshot({ path: 'docs/images/settings.png' });
  console.log('Saved demo-only README screenshots.');
} finally { await browser.close(); }
