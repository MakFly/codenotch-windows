import { chromium, expect } from '@playwright/test';
import { emptySnapshot } from '../src/lib/types';

const browser = await chromium.launch({ headless: true });
try {
  for (const size of [{ width: 860, height: 690 }, { width: 600, height: 500 }]) {
    const page = await browser.newPage({ viewport: size });
    page.setDefaultTimeout(8000);
    const errors: string[] = [];
    page.on('pageerror', e => errors.push(e.message));
    const snapshot = structuredClone(emptySnapshot);
    snapshot.config.settings.language = 'fr';
    snapshot.config.settings.onboardingDone = false;
    await page.addInitScript(snapshot => {
      const host = window as any;
      host.isTauri = true;
      host.saved = [];
      host.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
      host.__TAURI_INTERNALS__ = {
        transformCallback: () => 1, unregisterCallback: () => {},
        invoke: async (command: string, args: any) => {
          if (command === 'get_snapshot') return snapshot;
          if (command === 'provider_status') return ['codex', 'claude', 'gemini', 'grok', 'opencode'].map(provider => ({
            provider, installed: provider !== 'gemini', cliPath: null, signedIn: false, signedInAt: null, profilePath: '',
          }));
          if (command === 'save_settings') {
            host.saved.push(args.settings);
            snapshot.config.settings = args.settings;
            return structuredClone(snapshot);
          }
          return null;
        },
      };
    }, snapshot);
    await page.goto('http://127.0.0.1:1437/');
    await expect(page.getByRole('heading', { name: 'Connecte tes abonnements IA' })).toBeVisible();
    await expect(page.getByRole('navigation', { name: 'Réglages' })).toHaveCount(0);
    await expect(page.getByRole('region', { name: 'Tes abonnements' }).getByRole('button', { name: 'Se connecter', exact: true })).toHaveCount(5);
    const finish = page.getByRole('button', { name: 'Terminer', exact: true });
    await expect(finish).toBeDisabled(); // Nothing connected yet.
    await expect(finish).toBeInViewport();
    await expect(page.getByRole('button', { name: 'Plus tard', exact: true })).toBeInViewport();
    await page.screenshot({ path: `test-results/welcome-${size.width}.png` });
    await page.getByRole('button', { name: 'Plus tard', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Comptes', exact: true })).toBeVisible();
    expect(await page.evaluate(() => (window as any).saved.at(-1).onboardingDone)).toBe(true);
    // The welcome screen can be shown again from the preferences.
    await page.getByRole('button', { name: 'Apparence', exact: true }).click();
    await page.getByRole('button', { name: 'Revoir l’écran d’accueil', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Connecte tes abonnements IA' })).toBeVisible();
    expect(errors).toEqual([]);
    console.log(`PASS: welcome ${size.width}×${size.height}: first-run screen, providers, skip, finish guard, reopen.`);
    await page.close();
  }
} finally {
  await browser.close();
}
