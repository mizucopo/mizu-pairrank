import { messagesEn } from "./locales/messages-en.js";
import { viewEn } from "./locales/view-en.js";

export type Locale = "ja" | "en";
export type LanguagePreference = Locale | "system";
export const languageStorageKey = "pairrank.language";
let preference: LanguagePreference = "ja";
const english: Record<string, string> = {
  ...messagesEn,
  ...viewEn,
  "デスクトップアプリで起動してください。": "Please launch the desktop app.",
  "言語設定を保存できませんでした。この起動中のみ適用されます。":
    "The language setting could not be saved. It will apply only for this session.",
};

export function isLanguagePreference(value: unknown): value is LanguagePreference {
  return value === "ja" || value === "en" || value === "system";
}

// Keep existing installations in Japanese unless the user chooses another language.
export function initializeLanguage(): void {
  try {
    const saved = globalThis.localStorage?.getItem(languageStorageKey);
    preference = isLanguagePreference(saved) ? saved : "ja";
  } catch {
    preference = "ja";
  }
}

export function getLanguagePreference(): LanguagePreference {
  return preference;
}

export function resolveLocale(languages: readonly string[]): Locale {
  for (const language of languages) {
    const base = language.toLowerCase().split(/[-_]/)[0];
    if (base === "ja" || base === "en") return base;
  }
  return "en";
}

export function getLocale(): Locale {
  return preference === "system"
    ? resolveLocale(globalThis.navigator?.languages ?? [globalThis.navigator?.language ?? "en"])
    : preference;
}

/** Applies immediately; false means the preference could not be persisted. */
export function setLanguagePreference(value: LanguagePreference): boolean {
  preference = value;
  try {
    if (!globalThis.localStorage) return false;
    globalThis.localStorage.setItem(languageStorageKey, value);
    return true;
  } catch {
    return false;
  }
}

// Values are interpolated once, so names containing braces are not interpreted as messages.
// HTML-producing callers must escape user-provided values at their rendering boundary.
export function t(message: string, values: Record<string, string | number> = {}): string {
  const template =
    getLocale() === "en" && Object.hasOwn(english, message) ? english[message]! : message;
  return template.replace(/\{(\w+)\}/g, (match: string, key: string) =>
    Object.hasOwn(values, key) ? String(values[key]) : match,
  );
}
