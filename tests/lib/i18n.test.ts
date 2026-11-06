// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  getLanguagePreference,
  getLocale,
  initializeLanguage,
  isLanguagePreference,
  languageStorageKey,
  resolveLocale,
  setLanguagePreference,
  t,
} from "../../src/lib/i18n.js";

afterEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
  initializeLanguage();
});

describe("language preferences", () => {
  it("keeps existing installations in Japanese and rejects invalid persisted settings", () => {
    initializeLanguage();
    expect(getLocale()).toBe("ja");
    localStorage.setItem(languageStorageKey, "unsupported");
    initializeLanguage();
    expect(getLanguagePreference()).toBe("ja");
    expect(isLanguagePreference("EN")).toBe(false);
  });
  it("persists explicit choices across initialization", () => {
    expect(setLanguagePreference("en")).toBe(true);
    initializeLanguage();
    expect(getLocale()).toBe("en");
    expect(t("デスクトップアプリで起動してください。")).toBe("Please launch the desktop app.");
    setLanguagePreference("ja");
    expect(t("デスクトップアプリで起動してください。")).toBe(
      "デスクトップアプリで起動してください。",
    );
  });
  it("uses the first supported system language and falls back to English", () => {
    expect(resolveLocale(["fr-FR", "ja-JP", "en-US"])).toBe("ja");
    expect(resolveLocale(["en-GB", "ja"])).toBe("en");
    expect(resolveLocale(["JA_jp"])).toBe("ja");
    expect(resolveLocale(["de-DE"])).toBe("en");
    expect(resolveLocale([])).toBe("en");
    vi.spyOn(navigator, "languages", "get").mockReturnValue(["ja-JP"]);
    setLanguagePreference("system");
    expect(getLocale()).toBe("ja");
  });
  it("falls back safely when storage is unavailable, retaining a session choice", () => {
    vi.spyOn(localStorage, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    initializeLanguage();
    expect(getLocale()).toBe("ja");
    vi.spyOn(localStorage, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(setLanguagePreference("en")).toBe(false);
    expect(getLocale()).toBe("en");
  });
  it("interpolates without reinterpreting values and preserves unknown messages", () => {
    setLanguagePreference("en");
    expect(t("{name}: {count}", { name: "<tag>{count}", count: 2 })).toBe("<tag>{count}: 2");
    expect(t("Unknown {missing}")).toBe("Unknown {missing}");
    expect(t("constructor")).toBe("constructor");
    expect(t("__proto__")).toBe("__proto__");
  });
});

describe("English translation catalog", () => {
  it("uses consistent shared messages and preserves interpolation placeholders", async () => {
    const { messagesEn } = await import("../../src/lib/locales/messages-en.js");
    const { viewEn } = await import("../../src/lib/locales/view-en.js");
    for (const [key, value] of Object.entries(messagesEn)) {
      if (key in viewEn) expect(value, key).toBe(viewEn[key]);
    }
    const placeholders = (value: string) =>
      [...value.matchAll(/\{(\w+)\}/g)].map((match) => match[1]).sort();
    for (const [key, value] of Object.entries({ ...messagesEn, ...viewEn })) {
      expect(placeholders(value), key).toEqual(placeholders(key));
    }
  });
});
