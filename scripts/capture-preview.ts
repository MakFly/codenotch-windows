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
  await page.locator('.preview-caption button').click();
  await page.getByRole('button', { name: 'Appearance', exact: true }).click();
  await page.locator('.settings-window').screenshot({ path: 'docs/images/settings.png' });
  console.log('Saved demo-only README screenshots.');
} finally { await browser.close(); }
