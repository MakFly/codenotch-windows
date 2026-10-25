import { chromium, expect } from '@playwright/test';
import { emptySnapshot, blankAccount } from '../src/lib/types';

const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 860, height: 900 } });
  page.setDefaultTimeout(8000);
  const errors: string[] = [];
  page.on('pageerror', e => errors.push(e.message));
  const snapshot = structuredClone(emptySnapshot);
  snapshot.config.settings.language = 'fr';
  snapshot.config.accounts = [{ ...blankAccount(), id: 'codex', name: 'Codex · personnel' }];
  await page.addInitScript(snapshot => {
    const host = window as any;
    host.isTauri = true;
    host.calls = [];
    host.callbacks = {};
    host.listeners = {};
    host.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    host.emitTauri = (event: string, payload: unknown) =>
      (host.listeners[event] ?? []).forEach((id: number) => host.callbacks[id]?.({ event, id: 0, payload }));
    let claudeSignedAt: number | null = null;
    const status = (provider: string) => ({
      codex: { provider, installed: true, cliPath: 'C:\\Users\\Kev\\AppData\\Local\\Programs\\OpenAI\\Codex\\bin\\codex.exe', signedIn: true, signedInAt: 1, profilePath: 'C:\\Users\\Kev\\.codex' },
      claude: { provider, installed: true, cliPath: 'C:\\Users\\Kev\\AppData\\Local\\Claude-Data\\claude_x\\claude-code\\2.1.308\\abc\\claude.exe', signedIn: claudeSignedAt !== null, signedInAt: claudeSignedAt, profilePath: 'C:\\Users\\Kev\\.claude' },
      gemini: { provider, installed: false, cliPath: null, signedIn: false, signedInAt: null, profilePath: 'C:\\Users\\Kev\\.gemini' },
      grok: { provider, installed: true, cliPath: 'C:\\Users\\Kev\\.grok\\bin\\grok.exe', signedIn: false, signedInAt: null, profilePath: 'C:\\Users\\Kev\\.grok' },
      opencode: { provider, installed: false, cliPath: null, signedIn: false, signedInAt: null, profilePath: 'C:\\Users\\Kev\\.local\\share\\opencode' },
    } as any)[provider];
    let nextCallback = 1;
    host.__TAURI_INTERNALS__ = {
      transformCallback: (cb: (v: unknown) => void) => { const id = nextCallback++; host.callbacks[id] = cb; return id; },
      unregisterCallback: (id: number) => { delete host.callbacks[id]; },
      invoke: async (command: string, args: any) => {
        host.calls.push([command, args]);
        if (command === 'plugin:event|listen') {
          (host.listeners[args.event] ??= []).push(args.handler);
          return args.handler;
        }
        if (command === 'get_snapshot') return snapshot;
        if (command === 'provider_status')
          return args?.provider ? [status(args.provider)] : ['codex', 'claude', 'gemini', 'grok', 'opencode'].map(status);
        if (command === 'start_login') {
          if (args.provider === 'codex')
            return { inApp: true, url: 'https://auth.openai.com/oauth/authorize?client_id=test', userCode: null };
          // The official CLI writes its credential file a moment after the browser step.
          setTimeout(() => { claudeSignedAt = Date.now(); }, 300);
          return { inApp: false, url: null, userCode: null };
        }
        if (command === 'finish_login') {
          if (!snapshot.config.accounts.some((a: any) => a.provider === args.provider))
            snapshot.config.accounts.push({ ...snapshot.config.accounts[0], id: args.provider, provider: args.provider, name: 'Claude · personnel' });
          return structuredClone(snapshot);
        }
        return null;
      },
    };
  }, snapshot);
  await page.goto('http://127.0.0.1:1437/');
  const section = page.getByRole('region', { name: 'Connexion rapide' });
  await expect(section).toBeVisible();
  await expect(section).toContainText('Connecté · suivi dans l’encoche');
  await expect(section).toContainText('Installé · pas encore connecté');
  await expect(section).toContainText('Détecté dans l’application de bureau');
  await expect(section).toContainText('npm i -g @google/gemini-cli');
  const buttons = section.getByRole('button', { name: 'Se connecter', exact: true });
  await expect(buttons).toHaveCount(4); // Claude and Grok enabled; Gemini and OpenCode wait for an install.
  await expect(section).toContainText('npm i -g opencode-ai');
  await expect(buttons.nth(1)).toBeDisabled();
  await page.screenshot({ path: 'test-results/quick-connect-before.png' });

  // Claude: official CLI window, detected when its credential file changes.
  await buttons.first().click();
  await expect(section).toContainText('Termine la connexion Claude dans le navigateur');
  await expect(section).toContainText('Connexion Claude réussie', { timeout: 10000 });
  await expect(page.getByRole('button', { name: 'Claude · personnel Claude', exact: true })).toBeVisible();

  // Codex: in-app OAuth through App Server, completed by its login-finished event.
  await section.getByRole('button', { name: 'Reconnecter', exact: true }).first().click();
  await expect(section).toContainText('navigateur qui vient de s’ouvrir');
  await expect(section.locator('.login-url')).toHaveText(/^https:\/\/auth\.openai\.com\//);
  await page.screenshot({ path: 'test-results/quick-connect-codex-pending.png' });
  await page.evaluate(() => (window as any).emitTauri('login-finished', { provider: 'codex', success: true, error: null }));
  await expect(section).toContainText('Connexion Codex / ChatGPT réussie');

  // A refused sign-in is reported, not mistaken for success.
  await section.getByRole('button', { name: 'Reconnecter', exact: true }).first().click();
  await page.evaluate(() => (window as any).emitTauri('login-finished', { provider: 'codex', success: false, error: 'Connexion ChatGPT annulée ou refusée.' }));
  await expect(section.getByRole('alert')).toContainText('Connexion ChatGPT annulée ou refusée.');

  // A signed release found by the updater is announced and installed on request.
  await page.evaluate(() => (window as any).emitTauri('update-available', { version: '0.1.2', current: '0.1.1', notes: null }));
  await expect(page.getByText('CodeNotch 0.1.2 est disponible')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Mise à jour 0.1.2' })).toBeVisible();
  await page.screenshot({ path: 'test-results/update-available.png' });
  await page.getByRole('button', { name: 'Mettre à jour', exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).calls.some(([c]: [string]) => c === 'install_update'))).toBe(true);

  const calls: [string, any][] = await page.evaluate(() => (window as any).calls);
  expect(calls.find(([c]) => c === 'start_login')?.[1]).toEqual({ provider: 'claude', profilePath: null });
  expect(calls.filter(([c, a]) => c === 'finish_login').map(([, a]) => a.provider)).toEqual(['claude', 'codex']);
  await page.screenshot({ path: 'test-results/quick-connect-after.png' });
  expect(errors).toEqual([]);
  console.log('PASS: quick connect shows detected CLIs, signs in Claude through its CLI and Codex in-app, and reports refusals.');
} finally {
  await browser.close();
}
