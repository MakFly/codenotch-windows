import { chromium, expect } from '@playwright/test';
import { emptySnapshot } from '../src/lib/types';

const browser = await chromium.launch({ headless: true });
try {
  for (const edge of ['right', 'left', 'top', 'bottom'] as const) {
    const page = await browser.newPage({ viewport: { width: 900, height: 800 } });
    page.setDefaultTimeout(5000);
    const snapshot = structuredClone(emptySnapshot);
    snapshot.config.settings.language = 'fr';
    snapshot.config.settings.widgetPosition = edge;
    snapshot.config.accounts = [{ id: 'test', name: 'Test', provider: 'claude', enabled: true, profilePath: '', endpoint: '', secretEnv: '', hasSecret: false }];
    snapshot.config.accounts.push({ ...snapshot.config.accounts[0], id: 'second', name: 'Second', provider: 'codex' });
    await page.addInitScript(({ snapshot }) => {
      const host = window as any;
      host.isTauri = true;
      host.modeCalls = [];
      host.__TAURI_INTERNALS__ = {
        transformCallback: () => 1,
        unregisterCallback: () => {},
        invoke: async (command: string, args: any) => {
          if (command === 'get_snapshot') return snapshot;
          if (command === 'set_notch_mode') {
            host.modeCalls.push({ mode: args.mode, time: performance.now() });
            // A slow native response must finish before the entry animation.
            await new Promise(resolve => setTimeout(resolve, 80));
          }
          return 1;
        },
      };
    }, { snapshot });
    await page.goto('http://127.0.0.1:1437/?widget=1');
    const notch = page.locator('.notch-position');
    const peek = page.getByRole('button', { name: 'Déplier l’encoche' });
    await expect(peek).toBeVisible();
    await peek.hover();
    // Observe actual frames, not just the presence of a transition declaration.
    const samples = await page.evaluate(async () => {
      const values: number[] = [];
      const start = performance.now();
      while (performance.now() - start < 500) {
        values.push(Number(getComputedStyle(document.querySelector('.notch-rail')!).opacity));
        await new Promise(requestAnimationFrame);
      }
      return values;
    });
    expect(samples.some(v => v > 0 && v < 1)).toBe(true);
    await expect(page.locator('.notch-rail')).toHaveCSS('opacity', '1');
    await page.getByRole('button', { name: /Claude, Test,/ }).hover();
    await expect.poll(() => page.evaluate(() => (window as any).modeCalls.at(-1).mode)).toBe('detail');
    const detailCalls = await page.evaluate(() => (window as any).modeCalls.length);
    await page.getByRole('button', { name: /Codex, Second,/ }).hover();
    await page.waitForTimeout(200);
    expect(await page.evaluate(() => (window as any).modeCalls.length)).toBe(detailCalls);
    const rail = await page.locator('.notch-rail').boundingBox();
    if (!rail) throw new Error('Missing rail');
    const center = { x: rail.x + rail.width / 2, y: rail.y + rail.height / 2 };
    await page.mouse.move(center.x, center.y);
    await page.mouse.move(100, 100);
    await expect(notch).toHaveClass(/is-retracted/);
    // Losing keyboard focus while exiting must not reverse the exit animation.
    await page.locator('.notch-position').dispatchEvent('focusout', { relatedTarget: null });
    await page.waitForTimeout(20);
    await expect(notch).toHaveClass(/is-retracted/);
    // Re-enter halfway through the exit; the native window must not shrink.
    await page.waitForTimeout(70);
    const before = await page.evaluate(() => (window as any).modeCalls.filter((c: any) => c.mode === 'collapsed').length);
    await page.mouse.move(center.x, center.y);
    await expect(notch).toHaveClass(/is-revealed/);
    await page.waitForTimeout(350);
    await expect(notch).toHaveClass(/is-open/);
    expect(await page.evaluate(() => (window as any).modeCalls.filter((c: any) => c.mode === 'collapsed').length)).toBe(before);
    // The rail/card crossing has a grace period and cannot instantly fold.
    await page.mouse.move(100, 100);
    await page.waitForTimeout(100);
    await expect(notch).toHaveClass(/is-revealed/);
    await expect(notch).toHaveClass(/is-retracted/);
    expect(await page.evaluate(() => (window as any).modeCalls.filter((c: any) => c.mode === 'collapsed').length)).toBe(before);
    await expect(peek).toBeVisible();
    await expect.poll(() => page.evaluate(() => (window as any).modeCalls.at(-1).mode)).toBe('collapsed');
    // Rapid crossings must not enqueue a collapse while the pointer keeps returning.
    const hotspot = await peek.boundingBox();
    if (!hotspot) throw new Error('Missing collapsed hotspot');
    const collapsedBefore = await page.evaluate(() => (window as any).modeCalls.filter((c: any) => c.mode === 'collapsed').length);
    for (let i = 0; i < 12; i++) {
      await page.mouse.move(hotspot.x + hotspot.width / 2, hotspot.y + hotspot.height / 2);
      await page.waitForTimeout(20);
      await page.mouse.move(100, 100);
      await page.waitForTimeout(20);
    }
    expect(await page.evaluate(() => (window as any).modeCalls.filter((c: any) => c.mode === 'collapsed').length)).toBe(collapsedBefore);
    await expect(peek).toBeVisible();
    await expect.poll(() => page.evaluate(() => (window as any).modeCalls.at(-1).mode)).toBe('collapsed');
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await peek.hover();
    await expect(notch).toHaveClass(/is-revealed/);
    expect(await page.locator('.notch-rail').evaluate(el => getComputedStyle(el).transitionDuration)).toBe('0s');
    await page.close();
    console.log(`PASS: ${edge}: provider switch without extra geometry, blur during exit, animated frames, reversed exit, 12 rapid crossings, deferred region clipping, reduced motion`);
  }
} finally { await browser.close(); }
