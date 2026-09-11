import { i18n } from "@lingui/core";
import { messages as enMessages } from "./locales/en/messages.po";
import { messages as zhMessages } from "./locales/zh/messages.po";

export const localeStorageKey = "token-toxication-locale";

export const locales = [
  { value: "en", label: "English" },
  { value: "zh", label: "中文" },
] as const;

export type AppLocale = (typeof locales)[number]["value"];

i18n.load({
  en: enMessages,
  zh: zhMessages,
});

function isAppLocale(value: string | null): value is AppLocale {
  return locales.some((locale) => locale.value === value);
}

export function detectLocale(): AppLocale {
  if (typeof window === "undefined") {
    return "en";
  }

  const stored = window.localStorage.getItem(localeStorageKey);
  if (isAppLocale(stored)) {
    return stored;
  }

  return window.navigator.language.toLowerCase().startsWith("zh") ? "zh" : "en";
}

export function activateLocale(locale: AppLocale) {
  i18n.activate(locale);
  if (typeof document !== "undefined") {
    document.documentElement.lang = locale;
  }
  if (typeof window !== "undefined") {
    window.localStorage.setItem(localeStorageKey, locale);
  }
}

activateLocale(detectLocale());

export { i18n };
