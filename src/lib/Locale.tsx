import { createContext, useContext } from 'react';
import { translator, type Language } from './i18n';

export const Locale = createContext<Language>('en');
export function useLocale() {
  const language = useContext(Locale);
  return { language, t: translator(language) };
}
