import { describe, expect, test } from 'bun:test';
import { translator } from '../src/lib/i18n';
import { defaultSettings } from '../src/lib/types';
import { freshness, percentage, tokens } from '../src/lib/format';

describe('English and French', () => {
  test('defaults to English and translates known UI and service errors', () => {
    expect(defaultSettings.language).toBe('en');
    const en = translator('en');
    const fr = translator('fr');
    expect(en('Comptes')).toBe('Accounts');
    expect(fr('Comptes')).toBe('Comptes');
    expect(en('Activer {name}', { name: 'Mon compte' })).toBe('Enable Mon compte');
    expect(en('Sonnet · 7 jours')).toBe('Sonnet · 7 days');
    expect(en('Le service a répondu HTTP 503. Vérifie le connecteur.')).toBe('The service returned HTTP 503. Check the connector.');
    expect(en('Trop de requêtes. Les relevés reprendront après le délai du fournisseur.')).toStartWith('Too many requests.');
    expect(fr(en('Compte retiré de CodeNotch.'))).toBe('Compte retiré de CodeNotch.');
    expect(en('my-custom-model')).toBe('my-custom-model');
  });
  test('formats numbers and freshness according to the chosen language', () => {
    expect(percentage(12.5, 'en')).toBe('12.5 %');
    expect(percentage(12.5, 'fr')).toBe('12,5 %');
    expect(tokens(1234, false, 'en')).toBe('1,234');
    expect(tokens(1234, false, 'fr')).toMatch(/1\s234/);
    expect(freshness(null, 1000000, 'en')).toBe('No reading');
    expect(freshness(990, 1000000, 'en')).toBe('10 s ago');
    expect(freshness(990, 1000000, 'fr')).toBe('Il y a 10 s');
  });
});
