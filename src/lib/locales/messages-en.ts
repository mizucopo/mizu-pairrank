export const messagesEn: Record<string, string> = {
  "リストが変更されました。リストを選択するか、新しく作成してください。":
    "The lists have changed. Select a list or create a new one.",
  "画像登録の結果を確認できませんでした。": "Could not confirm the image registration result.",
  "タグ名を入力してください。": "Enter a tag name.",
  "出力する表は 50 MiB 以内にしてください。画像を含めずに再試行できます。":
    "Reports must be 50 MiB or smaller. Try again without images.",
  "リストをエクスポートしました。": "List exported.",
  "新しいリストとしてインポートしました。": "Imported as a new list.",
  "名前を入力してください。": "Enter a name.",
  "項目名を入力してください。": "Enter an item name.",
  "比較をリセットしました。「比較する」からやり直せます。":
    "Comparisons reset. Select Compare to start again.",
  "順位ほぼ確定。比較を続けることもできます。":
    "The ranking is nearly settled. You can continue comparing.",
  "直前の回答を取り消しました。同じペアにもう一度回答できます。":
    "Last answer undone. You can answer the same pair again.",
  "検索語は1〜400文字、50語以内で入力してください。":
    "Enter a search query of 1–400 characters and no more than 50 words.",
  "保存済みの接続状態を確認するか、検索設定で取得方法と認証を設定してください。":
    "Check the saved connection or configure the method and authentication in search settings.",
  "保存済みキーを確認するか、検索設定でAPIキーを登録してください。":
    "Check the saved key or add an API key in search settings.",
  "APIキーの形式が正しくありません。": "The API key format is invalid.",
  "APIキーを削除しました。": "API key removed.",
  "APIキーを保存しました。": "API key saved.",
  "OpenAIのモデルIDを入力してください。": "Enter an OpenAI model ID.",
  "OpenAIの取得方法とモデルIDを保存しました。次の検索に適用されます。":
    "OpenAI method and model ID saved. They will apply to the next search.",
  "ChatGPTの接続状態を更新しました。": "ChatGPT connection status updated.",
  "保存済みのChatGPTアカウントを選択しました。": "Saved ChatGPT account selected.",
  "資格情報を読み取れませんでした。": "Could not read credentials.",
  "ChatGPTとの接続を解除しました。": "Disconnected from ChatGPT.",
  "リストが更新されています。もう一度比較を開始してください。":
    "The list has changed. Start comparing again.",
  "リストが更新されています。最新の比較を読み直してください。":
    "The list has changed. Reload the latest comparison.",
  "リストが見つかりません。": "List not found.",
  "タグが見つかりません。": "Tag not found.",
  "同じ名前のタグが既にあります。": "A tag with that name already exists.",
  "項目が見つかりません。": "Item not found.",
  "取り消せる回答がありません。": "There is no answer to undo.",
  ランキング: "Ranking",
  "Tier 表": "Tier list",
  "リストを再取得できませんでした: {error}": "Could not reload the list: {error}",
  "タグ名は{max}文字以内にしてください。": "Tag names must be {max} characters or fewer.",
  "{view}を {format} で出力しました。": "Exported {view} as {format}.",
  "インポート後のリスト一覧を更新できませんでした: {error}":
    "Could not refresh the lists after import: {error}",
  "{count}件の項目を追加しました。": "Added {count} items.",
  "モデル一覧を取得できませんでした: {error} モデルIDは直接入力して保存できます。":
    "Could not load models: {error} You can enter and save a model ID directly.",
  "接続状態を再確認できませんでした: {error}": "Could not recheck the connection: {error}",
  "画像の一括登録を停止しました。{done} / {total} 件を処理し、{registered} 件を登録、{skipped} 件を見送りました。":
    "Bulk image registration stopped. Processed {done} / {total} items; registered {registered}, skipped {skipped}.",
  "画像の一括登録が中断しました。{done} / {total} 件を処理し、{registered} 件を登録、{skipped} 件を見送りました。":
    "Bulk image registration interrupted. Processed {done} / {total} items; registered {registered}, skipped {skipped}.",
  "画像の一括登録が完了しました。{done} / {total} 件を処理し、{registered} 件を登録、{skipped} 件を見送りました。":
    "Bulk image registration completed. Processed {done} / {total} items; registered {registered}, skipped {skipped}.",
  あなたのランキング: "Your ranking",
  "あなたの Tier 表": "Your tier list",
  順位: "Rank",
  画像: "Image",
  項目: "Items",
  評価: "Rating",
  比較: "Comparisons",
  項目なし: "No items",
  "推定評価の高い順。評価が同じ場合は登録順です。タグ別の順位は表示項目で振り直しています。":
    "Sorted by estimated rating, with ties in registration order. Filtered ranks are recalculated for the displayed items.",
  "リスト全体の評価の最高値から最低値までを5等分。各 Tier は左ほど上位です。全項目が同じ評価のときは C に表示します。":
    "The full rating range is divided into five equal tiers. Items rank higher toward the left. All items appear in C when ratings are equal.",
  "リスト全体の評価の最高値から最低値までを5等分。全項目が同じ評価のときは C に表示します。":
    "The full rating range is divided into five equal tiers. All items appear in C when ratings are equal.",
  "選択したタグをすべて持つ項目はありません。": "No items have all the selected tags.",
  "項目はありません。": "No items.",
  "評価と「順位ほぼ確定」の判定は、タグの表示に関係なくリスト全体で行います。":
    "Ratings and ranking stability are calculated for the entire list, regardless of the tag filter.",
  "{count} 回": "{count} times",
  "{count} 項目 · {comparisons} 回の比較": "{count} items · {comparisons} comparisons",
  "表示: {count} 項目": "Showing: {count} items",
  "選択したタグ: {tags}（すべて一致）": "Selected tags: {tags} (match all)",
  "PDFで表示できない文字があります: {character}。HTML形式でエクスポートしてください。":
    "This character cannot be displayed in PDF: {character}. Export as HTML instead.",
  Aが大好き: "Strongly prefer A",
  Aが好き: "Prefer A",
  同じ: "Equal",
  Bが好き: "Prefer B",
  Bが大好き: "Strongly prefer B",
};
