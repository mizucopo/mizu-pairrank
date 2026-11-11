import { getLocale } from "./i18n.js";
import { backendEn } from "./locales/backend-en.js";

const diagnosticPrefixes: Record<string, string> = {
  "ドキュメントの場所を取得できません: ": "Could not locate the Documents folder: ",
  "保存先を作成できません: ": "Could not create the storage location: ",
  "データ処理に失敗しました: ": "Data processing failed: ",
  "データベース処理に失敗しました: ": "Database operation failed: ",
  "保存データを開けません: ": "Could not open saved data: ",
  "保存先を安全に開けません: ": "Could not safely open the storage location: ",
  "画像の保存フォルダーを開けませんでした: ": "Could not open the image folder: ",
  "画像の保存フォルダーを作成できませんでした: ": "Could not create the image folder: ",
  "保存済み画像の権限を設定できませんでした: ": "Could not set saved image permissions: ",
};

const templates: readonly [RegExp, string][] = [
  [
    /^Windows SQLite VFS に必須 syscall ([^\r\n]+) がありません。$/,
    "The Windows SQLite VFS is missing required syscall $1.",
  ],
  [
    /^Windows SQLite syscall ([^\r\n]+) を登録できませんでした。$/,
    "Could not register Windows SQLite syscall $1.",
  ],
  [
    /^("[^\r\n]*") の権限を安全に復元できません。所有者の読み取り権限（推奨 ([0-7]+)）を手動で復元して再試行してください。$/,
    "Could not safely restore permissions for $1. Manually restore owner read permission (recommended: $2) and try again.",
  ],
  [/^タグ名は(\d+)文字以内にしてください。$/, "Tag names must be no more than $1 characters."],
  [
    /^名前は空白のみ・制御文字を避け、(\d+)文字以内にしてください。$/,
    "Names must be no more than $1 characters, not just whitespace, and contain no control characters.",
  ],
  [
    /^保存先の拡張子は \.(zip|html|pdf) にしてください。$/,
    "Use the .$1 extension for the destination file.",
  ],
  [
    /^データベースのバージョン (\d+) はこのアプリの対応版 (\d+) より新しいため開けません。アプリを更新してください。$/,
    "Database version $1 is newer than this app's supported version $2. Update the app.",
  ],
  [
    /^データベースの更新（バージョン (\d+)）に失敗しました。変更を取り消しました: ([\s\S]*)$/,
    "Database update to version $1 failed. Changes were rolled back: $2",
  ],
  [
    /^OpenAIの応答がサイズ上限を超えました \(HTTP (\d{3})\)。$/,
    "The OpenAI response exceeded the size limit (HTTP $1).",
  ],
  [
    /^ChatGPT認証サービスの応答を確認できませんでした \(HTTP (\d{3})\)。$/,
    "Could not verify the ChatGPT authentication service response (HTTP $1).",
  ],
  [
    /^ChatGPT認証に失敗しました \(HTTP ([^\r\n]+)\)。再試行してください。$/,
    "ChatGPT authentication failed (HTTP $1). Try again.",
  ],
  [
    /^OpenAIのモデル「([^\r\n]*)」で画像検索を利用できません。モデルへのアクセス権を確認するか、別のモデルを指定してください。$/,
    "Image search is unavailable for OpenAI model ‘$1’. Check model access or choose another model.",
  ],
  [
    /^OpenAIのモデル「([^\r\n]*)」から画像検索の結果を取得できませんでした。画像検索に対応する別のモデルを指定してください。$/,
    "Could not retrieve image search results from OpenAI model ‘$1’. Choose another model that supports image search.",
  ],
  [
    /^OpenAIのモデル「([^\r\n]*)」は検索結果を返しましたが、画像が含まれていませんでした。検索語やモデルの画像検索への対応状況を確認してください。$/,
    "OpenAI model ‘$1’ returned search results without images. Check the query and the model's image-search support.",
  ],
  [
    /^OpenAIのモデル「([^\r\n]*)」が画像検索の結果を返しませんでした。モデルへのアクセス権を確認するか、別のモデルを指定してください。$/,
    "OpenAI model ‘$1’ returned no image search results. Check model access or choose another model.",
  ],
];

/** Translate only at the display boundary; recovery logic must retain the native error. */
export function localizeBackendError(message: string): string {
  if (getLocale() !== "en") return message;
  if (Object.hasOwn(backendEn, message)) return backendEn[message]!;
  for (const [prefix, translation] of Object.entries(diagnosticPrefixes)) {
    // Diagnostics can contain user paths, names, or external messages. Keep them verbatim.
    if (message.startsWith(prefix)) return translation + message.slice(prefix.length);
  }
  for (const [pattern, translation] of templates) {
    if (pattern.test(message)) return message.replace(pattern, translation);
  }
  return message;
}
