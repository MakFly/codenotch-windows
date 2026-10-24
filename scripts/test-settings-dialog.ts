import { chromium, expect } from '@playwright/test';
import { emptySnapshot, blankAccount } from '../src/lib/types';

const browser = await chromium.launch({ headless: true });
try {
  for (const size of [{ width: 860, height: 690 }, { width: 600, height: 500 }]) {
    const page = await browser.newPage({ viewport: size });
    page.setDefaultTimeout(5000);
    const errors: string[] = [];
    page.on('pageerror', e => errors.push(e.message));
    const snapshot = structuredClone(emptySnapshot);
    snapshot.config.settings.language = 'fr';
    snapshot.config.accounts = [
      { ...blankAccount(), id: 'claude', provider: 'claude', name: 'Claude · personnel' },
      { ...blankAccount(), id: 'codex', name: 'Codex · personnel' },
    ];
    await page.addInitScript(snapshot => {
      const host = window as any;
      host.isTauri = true;
      host.dashboardCommands = [];
      host.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
      host.__TAURI_INTERNALS__ = {
        transformCallback: () => 1, unregisterCallback: () => {},
        invoke: async (command: string) => {
          host.dashboardCommands.push(command);
          if (command === 'get_snapshot') return snapshot;
          if (command === 'save_account') {
            await new Promise(r => setTimeout(r, 200));
            throw new Error('Échec de test : compte non enregistré.');
          }
          return 1;
        },
      };
    }, snapshot);
    await page.goto('http://127.0.0.1:1437/');
    await expect(page.getByRole('heading', { name: 'Comptes', exact: true })).toBeVisible();
    const opener = page.getByRole('button', { name: 'Claude · personnel Claude', exact: true });
    await opener.click();
    const modal = page.getByRole('dialog', { name: 'Configurer le compte' });
    await expect(modal).toBeVisible();
    const backgroundScroll = await page.locator('.settings-body').evaluate(el => el.scrollTop);
    await page.getByLabel('Nom du compte').fill('Mon espace Claude');
    await page.getByLabel('Fournisseur', { exact: true }).click();
    await expect(page.getByRole('listbox')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('listbox')).toHaveCount(0);
    await expect(modal).toBeVisible();
    await page.screenshot({ path: `test-results/settings-dialog-${size.width}.png` });
    await page.getByText('Jeton et options avancées', { exact: true }).click();
    await expect(page.getByLabel('Ou une variable d’environnement')).toBeVisible();
    for (let i = 0; i < 16; i++) {
      await page.keyboard.press('Tab');
      expect(await modal.evaluate(el => el.contains(document.activeElement))).toBe(true);
    }
    await page.locator('.account-form-scroll').evaluate(el => el.scrollTop = el.scrollHeight);
    await page.screenshot({ path: `test-results/settings-dialog-advanced-${size.width}.png` });
    for (const locator of [modal.getByRole('button', { name: 'Enregistrer', exact: true }), modal.getByRole('button', { name: 'Fermer le formulaire' })]) {
      await expect(locator).toBeInViewport();
    }
    expect(await page.locator('.settings-body').evaluate(el => el.scrollTop)).toBe(backgroundScroll);
    await page.mouse.click(5, 5);
    await expect(modal).toBeVisible();
    await modal.getByRole('button', { name: 'Enregistrer', exact: true }).click();
    await expect(modal.getByRole('button', { name: 'Enregistrement…' })).toBeDisabled();
    await expect(modal.getByRole('alert')).toContainText('compte non enregistré');
    await expect(page.getByLabel('Nom du compte')).toHaveValue('Mon espace Claude');
    await page.keyboard.press('Escape');
    await expect(modal).toHaveCount(0);
    await expect(page.getByRole('dialog', { name: 'Réglages CodeNotch' })).toBeVisible();
    await expect(opener).toBeFocused();
    // Closing a child modal must not hide its native parent window.
    expect(await page.evaluate(() => (window as any).dashboardCommands.filter((c: string) => c === 'hide_dashboard'))).toHaveLength(0);
    await page.locator('.settings-body').evaluate(el => el.scrollTop = el.scrollHeight);
    await expect(page.getByRole('heading', { name: 'Comptes', exact: true })).toBeInViewport();
    await expect(page.getByRole('button', { name: 'Ajouter', exact: true })).toBeInViewport();
    await page.getByRole('button', { name: 'Fermer les réglages', exact: true }).click();
    await page.getByRole('button', { name: 'Réduire', exact: true }).click();
    await page.keyboard.press('Escape');
    expect(await page.evaluate(() => (window as any).dashboardCommands.filter((c: string) => ['hide_dashboard', 'minimize_dashboard'].includes(c)))).toEqual(['hide_dashboard', 'minimize_dashboard', 'hide_dashboard']);
    expect(errors).toEqual([]);
    console.log(`PASS: settings ${size.width}×${size.height}: modal, Select/Escape, focus trap/return, fixed actions, scroll isolation, failed save preserves draft, sticky page heading`);
    await page.close();
  }
} finally { await browser.close(); }
