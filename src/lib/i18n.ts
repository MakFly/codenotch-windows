import english from './messages.en.json';

export type Language = 'en' | 'fr';
const messages: Record<string, string> = english;
const french = Object.fromEntries(Object.entries(messages).map(([fr, en]) => [en, fr]));

export function translator(language: Language) {
  return (message: string, values: Record<string, string | number> = {}) => {
    const key = message.replace(/\s+/g, ' ').trim();
    let translated = language === 'fr' ? french[key] ?? message : messages[key] ?? message;
    if (language === 'en' && !messages[key]) {
      translated = translated
        .replace(/^(Sonnet · |Opus · )?7 jours$/, (_, prefix: string | undefined) => `${prefix ?? ''}7 days`)
        .replace(/^Le service a répondu HTTP (\d+)\. Vérifie le connecteur\.$/, 'The service returned HTTP $1. Check the connector.')
        .replace(/^Configuration illisible : (.+)\. Le fichier a été conservé\.$/, 'Unreadable configuration: $1. The file has been preserved.');
    }
    return translated.replace(/\{(\w+)\}/g, (match, name: string) => String(values[name] ?? match));
  };
}
