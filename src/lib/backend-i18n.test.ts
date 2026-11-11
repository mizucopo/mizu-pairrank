import { afterEach, describe, expect, it } from "vitest";

import { localizeBackendError } from "./backend-i18n.js";
import { setLanguagePreference } from "./i18n.js";

afterEach(() => setLanguagePreference("ja"));

describe("native error presentation", () => {
  it("preserves Japanese errors in Japanese mode", () => {
    setLanguagePreference("ja");
    expect(localizeBackendError("リストが更新されています。最新の比較を読み直してください。")).toBe(
      "リストが更新されています。最新の比較を読み直してください。",
    );
  });

  it("translates known validation and recovery messages only for display", () => {
    setLanguagePreference("en");
    expect(localizeBackendError("項目名を入力してください。")).toBe("Enter an item name.");
    expect(localizeBackendError("リストが更新されています。最新の比較を読み直してください。")).toBe(
      "The list has changed. Reload the latest comparison.",
    );
    expect(localizeBackendError("タグ名は30文字以内にしてください。")).toBe(
      "Tag names must be no more than 30 characters.",
    );
  });

  it("distinguishes image retrieval failures from empty search results in English", () => {
    setLanguagePreference("en");
    expect(
      localizeBackendError(
        "検索結果は取得できましたが、表示できる画像を読み込めませんでした。通信状態を確認するか、検索語を変えて再試行してください。",
      ),
    ).toBe(
      "Search results were returned, but no image previews could be loaded. Check your connection or change the query and try again.",
    );
    expect(
      localizeBackendError(
        "OpenAIの画像検索結果に利用できる画像URL・出典URLがありませんでした。検索語を変えて再試行してください。",
      ),
    ).toBe(
      "OpenAI image search returned no usable image and source URLs. Change the query and try again.",
    );
    expect(
      localizeBackendError(
        "OpenAIのモデル「chosen-model」は検索結果を返しましたが、画像が含まれていませんでした。検索語やモデルの画像検索への対応状況を確認してください。",
      ),
    ).toBe(
      "OpenAI model ‘chosen-model’ returned search results without images. Check the query and the model's image-search support.",
    );
  });

  it("preserves arbitrary diagnostic details and Japanese file paths verbatim", () => {
    setLanguagePreference("en");
    const details = "権限エラー: /Users/名前/項目が見つかりません。\n$& {name}";
    expect(localizeBackendError(`データベース処理に失敗しました: ${details}`)).toBe(
      `Database operation failed: ${details}`,
    );
    expect(
      localizeBackendError(
        `データベースの更新（バージョン 12）に失敗しました。変更を取り消しました: ${details}`,
      ),
    ).toBe(`Database update to version 12 failed. Changes were rolled back: ${details}`);
  });

  it("preserves unknown messages and does not translate embedded user text", () => {
    setLanguagePreference("en");
    for (const value of [
      "未知のエラー",
      "My list: 項目が見つかりません。",
      "constructor",
      "__proto__",
    ]) {
      expect(localizeBackendError(value)).toBe(value);
    }
    expect(
      localizeBackendError(
        "OpenAIのモデル「日本語$&」から画像検索の結果を取得できませんでした。画像検索に対応する別のモデルを指定してください。",
      ),
    ).toBe(
      "Could not retrieve image search results from OpenAI model ‘日本語$&’. Choose another model that supports image search.",
    );
  });
});
