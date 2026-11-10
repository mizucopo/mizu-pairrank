import { localizeBackendError } from "./backend-i18n.js";
import { answers, configuredSearchProviders, tagNameMaxLength } from "./controller.js";
import type { AppState, View } from "./controller.js";
import { t, getLanguagePreference } from "./i18n.js";
import { escapeHtml } from "./html.js";
import { matchesSelectedTags } from "./tag-filter.js";
import { tierRows } from "./tier.js";
import type { Item, SearchProvider } from "./types.js";

const e = escapeHtml;
const disabled = (state: AppState): string => (state.busy ? " disabled" : "");
const number = (value: number): string => value.toFixed(2);
const providerNames: Record<SearchProvider, string> = {
  brave: "Brave",
  ollama: "Ollama",
  openai: "OpenAI",
};

function keyStatus(s: AppState, provider: SearchProvider): string {
  if (s.settings?.errors?.[provider] !== undefined) return t("確認できません");
  if (!s.checkedProviders.includes(provider)) return t("未確認");
  return (
    provider === "brave"
      ? s.settings?.braveConfigured
      : provider === "ollama"
        ? s.settings?.ollamaConfigured
        : s.settings?.openaiConfigured
  )
    ? t("設定済み")
    : t("未設定");
}
function credentialAccess(s: AppState): string {
  return `<div class="settings-note"><p>${t("画像検索のAPIキーを安全に保存・読み出すため、OSの資格情報ストアを使います。macOSではキーチェーンの確認が出ることがあります。")}</p><p class="muted small">${t("画像検索を使わなくても、比較やファイルからの画像登録を続けられます。")}</p><p class="small">Brave: ${keyStatus(s, "brave")} · Ollama: ${keyStatus(s, "ollama")} · OpenAI: ${keyStatus(s, "openai")}</p><button type="button" class="secondary" data-action="check-search-settings"${disabled(s)}>${t("保存済みキー・接続を確認")}</button></div>`;
}
function picture(item: Item, assetUrl: (path: string) => string, large = false): string {
  return item.image
    ? `<img class="item-image${large ? " large" : ""}" src="${e(assetUrl(item.image.path))}" alt="${e(item.name)}" ${large ? "" : 'loading="lazy"'} /><span class="image-fallback" hidden>${t("画像を表示できません")}</span>`
    : `<span class="item-image placeholder${large ? " large" : ""}" aria-hidden="true">${e(Array.from(item.name)[0] ?? "・")}</span>`;
}
function sidebar(s: AppState): string {
  const navigationDisabled = s.readPending === "settings" ? "" : disabled(s);
  return `<aside class="sidebar"><div class="brand"><span class="brand-mark" aria-hidden="true">p<span>r</span></span><div>pairrank<small>${t("あなたの好みを、順番に。")}</small></div></div><div class="sidebar-heading"><h2>${t("マイリスト")}</h2><span>${s.lists.length}</span></div><nav class="lists" aria-label="${t("リスト")}">${s.lists.map((list) => `<button class="list-link${list.id === s.active?.id ? " selected" : ""}" data-action="select-list" data-id="${list.id}"${navigationDisabled} ${list.id === s.active?.id ? 'aria-current="true"' : ""}><span class="list-name">${e(list.name)}</span><small>${t("{count} 項目", { count: list.itemCount })}<span>${list.converged ? t("ほぼ確定 ✓") : t("{count} 回比較", { count: list.comparisonCount })}</span></small></button>`).join("")}</nav><div class="sidebar-actions"><button class="new-list secondary" data-action="create-list"${navigationDisabled}>${t("＋ リストを作成")}</button><button class="new-list secondary" data-action="import-list"${navigationDisabled}>${t("リストをインポート")}</button></div><div class="sidebar-footer"><span class="local-dot"></span>${t("この端末に自動保存")}<button class="text-button" data-view="settings"${navigationDisabled}>${t("設定")}</button></div></aside>`;
}
function progress(s: AppState): string {
  const list = s.active;
  if (!list) return "";
  const c = list.convergence;
  return `<details class="progress-details"><summary>${c.converged ? t("✓ 順位ほぼ確定") : t("◌ 順位を整理中")}<span>${t("{count} 回比較済み", { count: list.comparisonCount })}</span></summary><div class="progress-body"><p>${t("全項目の不確実性 σ が2.5以下になり、直近{count}回答で各項目の順位の動きが1位以内になると「順位ほぼ確定」です。", { count: c.requiredAnswers })}</p><div class="progress-numbers"><span>${t("最大 σ")} <strong>${number(c.maxSigma)}</strong> / 2.50</span><span>${t("順位の動き")} <strong>${c.maxRankSpan ?? "—"}</strong> ${t("/ 1位")}</span><span>${t("判定履歴")} <strong>${c.observedAnswers}</strong>${t(" / {count} 回", { count: c.requiredAnswers })}</span></div><progress aria-label="${t("終了判定に必要な履歴")}" max="${c.requiredAnswers}" value="${c.observedAnswers}"></progress></div></details>`;
}
function listHeader(s: AppState): string {
  const list = s.active;
  if (!list) return "";
  const tabs: { view: View; label: string }[] = [
    { view: "items", label: t("項目") },
    { view: "compare", label: t("比較する") },
    { view: "ranking", label: t("ランキング") },
    { view: "tier", label: t("Tier 表") },
  ];
  return `<header class="page-header"><div><p class="eyebrow">MY RANKING</p><h1>${e(list.name)}</h1><p class="muted">${t("{items} 項目 · {comparisons} 回の比較", { items: list.items.length, comparisons: list.comparisonCount })}</p></div><div class="header-actions"><button class="text-button" data-action="export-list"${disabled(s)}>${t("エクスポート")}</button><button class="text-button" data-action="duplicate-list"${disabled(s)}>${t("複製")}</button><button class="text-button" data-action="rename-list"${disabled(s)}>${t("名前を変更")}</button><button class="text-button danger-text" data-action="reset-comparisons"${disabled(s)}>${t("比較をリセット")}</button><button class="text-button danger-text" data-action="delete-list"${disabled(s)}>${t("削除")}</button></div></header><nav class="tabs" aria-label="${t("表示切り替え")}">${tabs.map(({ view, label }) => `<button data-view="${view}" class="${s.view === view ? "active" : ""}" ${s.view === view ? 'aria-current="page"' : ""}${disabled(s)} ${view === "compare" && list.items.length < 2 ? "disabled" : ""}>${label}</button>`).join("")}</nav>`;
}
function itemsView(s: AppState, assetUrl: (path: string) => string): string {
  const list = s.active;
  if (!list) return "";
  const missing = list.items.filter((item) => !item.image).length;
  const bulkDisabled = s.busy || !missing;
  const query = s.itemNameQuery.trim().toLocaleLowerCase();
  const visible = list.items.filter(
    (item) =>
      item.name.toLocaleLowerCase().includes(query) && (!s.itemsMissingImagesOnly || !item.image),
  );
  return `<section><div class="section-heading"><div><h2>${t("比べたいものを追加")}</h2><p class="muted">${t("1行に1項目。画像は追加後に選べます。")}</p></div></div><form data-form="add-items" class="add-items"><label class="sr-only" for="item-names">${t("項目名（改行で複数登録）")}</label><textarea id="item-names" data-draft="items" data-focus="items" rows="3" placeholder="${t("気になるもの、お気に入りのもの…")}" required${disabled(s)}>${e(s.drafts.items)}</textarea><div><span class="muted">${t("画像なしでも比較できます")}</span><button type="submit"${disabled(s)}>${t("項目を追加")}</button></div></form><div class="section-heading"><div><h2>${t("登録した項目")} <span class="count">${list.items.length}</span></h2><p class="muted">${t("画像未登録 {count} 件", { count: missing })}</p></div><div class="item-heading-actions"><button class="secondary" data-action="bulk-images"${bulkDisabled ? " disabled" : ""}>${t("未登録画像を一括登録")}</button>${list.items.length >= 2 ? `<button class="secondary" data-view="compare"${disabled(s)}>${t("比較を始める →")}</button>` : ""}</div></div><div class="item-filters"><label for="item-name-query">${t("項目名で検索")}<input type="search" id="item-name-query" data-focus="item-name-query" value="${e(s.itemNameQuery)}" placeholder="${t("名前の一部を入力")}"${disabled(s)} /></label><label class="item-filter-checkbox"><input type="checkbox" id="items-missing-images-only" data-focus="items-missing-images-only"${s.itemsMissingImagesOnly ? " checked" : ""}${disabled(s)} />${t("画像未登録だけ表示")}</label><button type="button" class="secondary" data-action="clear-item-filters"${disabled(s)}>${t("条件を解除")}</button><p class="muted" role="status">${t("{visible} / {total} 件表示", { visible: visible.length, total: list.items.length })}</p></div>${
    list.items.length === 0
      ? `<div class="empty compact"><span class="empty-symbol">＋</span><h3>${t("最初の2項目を追加しましょう")}</h3><p>${t("どちらが好きかを答えると、少しずつ順位が見えてきます。")}</p></div>`
      : visible.length === 0
        ? `<div class="empty compact"><p>${t("条件に一致する項目はありません。検索条件を変更してください。")}</p></div>`
        : `<div class="item-list">${[...visible]
            .sort((a, b) => a.id - b.id)
            .map(
              (item) =>
                `<article class="item-row">${picture(item, assetUrl)}<div class="item-description"><h3>${e(item.name)}</h3><small>${t("{count} 回比較", { count: item.comparisonCount })} ${item.comparisonCount === 0 ? `· <span class="new-badge">${t("未評価")}</span>` : ""}</small>${
                  item.tagIds.length
                    ? `<div class="tag-chips">${list.tags
                        .filter((tag) => item.tagIds.includes(tag.id))
                        .map((tag) => `<span class="tag-chip">${e(tag.name)}</span>`)
                        .join("")}</div>`
                    : ""
                }</div><div class="item-actions"><button class="secondary small" data-action="image" data-id="${item.id}"${disabled(s)}>${t("画像")}</button><button class="text-button small" data-action="tags" data-id="${item.id}" aria-label="${e(t("{name} のタグ", { name: item.name }))}"${disabled(s)}>${t("タグ")}</button><button class="text-button small" data-action="rename-item" data-id="${item.id}"${disabled(s)}>${t("名前")}</button><button class="text-button small danger-text" data-action="delete-item" data-id="${item.id}"${disabled(s)}>${t("削除")}</button></div></article>`,
            )
            .join("")}</div>`
  }</section><section class="tag-management"><div class="section-heading"><div><h2>${t("タグ管理")} <span class="count">${list.tags.length}</span></h2><p class="muted">${t("タグはこのリストの項目に付けられます。項目には複数のタグを付けられます。")}</p></div><button class="secondary" data-action="manage-tags"${disabled(s)}>${t("タグを管理")}</button></div>${list.tags.length ? `<div class="tag-summary">${list.tags.map((tag) => `<span class="tag-chip">${e(tag.name)} <small>${t("{count} 件", { count: list.items.filter((item) => item.tagIds.includes(tag.id)).length })}</small></span>`).join("")}</div>` : `<p class="muted">${t("タグはまだありません。")}</p>`}</section>`;
}
function undoAnswerButton(s: AppState): string {
  return `<div class="comparison-undo"><button class="secondary" data-action="undo-answer"${s.busy || !s.canUndoAnswer ? " disabled" : ""}>${t("直前の回答を取り消す")}</button></div>`;
}
function comparisonView(s: AppState, assetUrl: (path: string) => string): string {
  const pair = s.pair;
  if (!pair && s.comparisonPaused && !s.busy)
    return `<div class="empty"><div class="empty-symbol">⇄</div><h2>${t("比較できるペアをすべて保留しました")}</h2><p>${t("情報を確認できたら、保留を解除してもう一度比較できます。")}<br />${t("スキップは評価や比較回数に含まれません。")}</p><button data-action="resume-skipped-comparisons">${t("保留した比較を再開")}</button></div>${progress(s)}`;
  if (!pair)
    return `<div class="empty"><div class="empty-symbol">⇄</div><h2>${s.busy ? t("次の比較を選んでいます") : t("比較を再開できます")}</h2><p>${t("順位を知る手がかりになるペアを選びます。")}</p>${s.busy ? '<span class="spinner" aria-hidden="true"></span>' : `<button data-action="continue-comparison">${t("次のペアを表示")}</button>`}</div>`;
  return `<section class="comparison"><div class="comparison-heading"><p class="eyebrow">FOLLOW YOUR PREFERENCE</p><h2>${t("どちらが好きですか？")}</h2><p class="muted">${t("「大好き」は、相手よりかなり好き。「好き」は、少し好き。")}</p></div><div class="pair-cards">${(
    [
      ["A", pair.a],
      ["B", pair.b],
    ] as const
  )
    .map(
      ([side, item]) =>
        `<article class="pair-card side-${side.toLowerCase()}"><span class="side-label">${side}</span><div class="pair-image">${picture(item, assetUrl, true)}</div><h3>${e(item.name)}</h3></article>`,
    )
    .join(
      "",
    )}<span class="versus" aria-hidden="true">or</span></div><div class="answers" role="group" aria-label="${t("比較への回答")}">${answers.map((answer) => `<button class="answer ${answer.value}" data-answer="${answer.value}"${disabled(s)}>${t(answer.label)}<kbd>${answer.key}</kbd></button>`).join("")}</div><p class="comparison-note">${s.busy ? t("次の比較を選んでいます…") : t("直感で選んで大丈夫。キーボードの1〜5でも回答できます。")}</p>${undoAnswerButton(s)}<div class="comparison-skip"><button class="secondary" data-action="skip-comparison"${disabled(s)}>${t("情報が足りないのでスキップ")}</button><p class="muted small">${t("評価せずに保留します。他の比較に{count}回答すると再び候補になります。", { count: Math.max(1, (s.active?.items.length ?? 2) - 1) })}</p></div></section>${progress(s)}`;
}
function tagFilter(s: AppState): string {
  const tags = s.active?.tags ?? [];
  return `<div class="tag-filter"><p class="muted">${t("選択したタグをすべて持つ項目を表示します。もう一度クリックすると解除できます。")}</p>${tags.length ? `<div class="tag-chips" role="group" aria-label="${t("表示するタグ")}">${tags.map((tag) => `<button type="button" class="tag-chip" data-action="toggle-tag" data-id="${tag.id}" data-focus="filter-tag-${tag.id}" aria-pressed="${s.selectedTagIds.includes(tag.id)}"${disabled(s)}>${e(tag.name)}</button>`).join("")}</div>` : `<p class="muted">${t("タグはまだありません。")}</p>`}</div>`;
}

function rankingView(s: AppState, assetUrl: (path: string) => string): string {
  const list = s.active;
  if (!list) return "";
  const visible = list.items.filter((item) => matchesSelectedTags(item.tagIds, s.selectedTagIds));
  return `<section>${list.convergence.converged ? `<div class="converged-banner"><span>✓</span><div><h2>${t("順位ほぼ確定")}</h2><p>${t("好みの順番が落ち着きました。いつでも比較を続けられます。")}</p></div></div>` : ""}<div class="section-heading"><div><h2>${t("あなたのランキング")}</h2><p class="muted">${t("推定評価の高い順。評価が同じ場合は登録順です。")}</p></div><button class="secondary" data-action="export-report"${disabled(s)}>${t("表を出力")}</button></div>${tagFilter(s)}${visible.length ? `<ol class="ranking-list">${visible.map((item, index) => `<li class="rank-row"><span class="rank-number${index < 3 ? " top" : ""}">${index + 1}</span>${picture(item, assetUrl)}<div class="item-description"><h3>${e(item.name)}</h3><small>${t("{count} 回比較", { count: item.comparisonCount })}</small></div><div class="rating-values"><span>${t("評価")} <strong>${number(item.rating.mu)}</strong></span><span>σ <strong>${number(item.rating.sigma)}</strong></span></div></li>`).join("")}</ol>` : s.selectedTagIds.length ? `<div class="empty compact"><p>${t("選択したタグをすべて持つ項目はありません。")}</p></div>` : `<div class="empty compact"><p>${t("項目を追加すると、ここに順位が表示されます。")}</p></div>`}<p class="muted small">${t("評価と「順位ほぼ確定」の判定は、タグの表示に関係なくリスト全体で行います。")}</p></section>${progress(s)}`;
}
function tierView(s: AppState, assetUrl: (path: string) => string): string {
  const list = s.active;
  if (!list) return "";
  const allRows = tierRows(list.items);
  const rows = allRows.map((row) => ({
    ...row,
    entries: row.entries.filter(({ item }) => matchesSelectedTags(item.tagIds, s.selectedTagIds)),
  }));
  const hasVisibleItems = rows.some((row) => row.entries.length > 0);
  return `<section><div class="section-heading"><div><h2>${t("あなたの Tier 表")}</h2><p class="muted">${t("リスト全体の評価の最高値から最低値までを5等分。各 Tier は左ほど上位です。")}${list.items.length && allRows[3]?.entries.length === list.items.length ? t("全項目が同じ評価のときは C に表示します。") : ""}</p></div><button class="secondary" data-action="export-report"${disabled(s)}>${t("表を出力")}</button></div>${tagFilter(s)}${hasVisibleItems ? `<div class="tier-table" aria-label="${t("Tier 表")}">${rows.map(({ label, entries }) => `<section class="tier-row" aria-labelledby="tier-${label}"><h3 id="tier-${label}" class="tier-label">${label}</h3>${entries.length ? `<ol class="tier-items" role="list" tabindex="0" data-focus="tier-${label}" aria-label="${t("{tier} Tier の項目", { tier: label })}">${entries.map(({ item }) => `<li class="tier-card">${picture(item, assetUrl)}<span class="tier-name">${e(item.name)}</span></li>`).join("")}</ol>` : `<p class="tier-empty">${t("項目なし")}</p>`}</section>`).join("")}</div>` : s.selectedTagIds.length ? `<div class="empty compact"><p>${t("選択したタグをすべて持つ項目はありません。")}</p></div>` : `<div class="empty compact"><p>${t("項目を追加すると、ここに Tier 表が表示されます。")}</p></div>`}</section>${progress(s)}`;
}
function settingsView(s: AppState): string {
  const language = getLanguagePreference();
  return `<header class="page-header"><div><p class="eyebrow">SETTINGS</p><h1>${t("設定")}</h1><p class="muted">${t("表示言語と画像検索サービスを設定してください。")}</p></div></header>${s.imageSearchReturn ? `<button type="button" class="secondary" data-action="resume-image-search"${disabled(s)}>${t("画像検索に戻る")}</button>` : ""}<section class="settings-card"><label for="app-language">${t("表示言語")}</label><select id="app-language" data-focus="app-language"${disabled(s)}><option value="system"${language === "system" ? " selected" : ""}>${t("システム設定に従う")}</option><option value="ja"${language === "ja" ? " selected" : ""}>日本語</option><option value="en"${language === "en" ? " selected" : ""}>English</option></select></section>${credentialAccess(s)}<section class="settings-grid">${(
    ["brave", "ollama"] as const
  )
    .map((provider) => {
      const configured =
        provider === "brave" ? s.settings?.braveConfigured : s.settings?.ollamaConfigured;
      const draft = provider === "brave" ? "braveKey" : "ollamaKey";
      const error = s.settings?.errors?.[provider];
      const readError =
        error === undefined
          ? undefined
          : error
            ? localizeBackendError(error)
            : t("資格情報ストアから読み取れませんでした。");
      const status = keyStatus(s, provider);
      return `<article class="settings-card"><div class="section-heading"><h2>${provider === "brave" ? "Brave Search" : "Ollama Web Search"}</h2><span class="badge ${configured && !readError ? "ready" : ""}">${status}</span></div>${readError ? `<p class="error" role="alert">${e(readError)}</p>` : ""}<p>${provider === "brave" ? t("Web上の画像を検索し、候補を表示します。") : t("検索したWebページから代表画像を取得します。")}</p><p class="muted small">${t("{site} でAPIキーを取得できます。各サービスの料金・利用制限が適用されます。", { site: provider === "brave" ? "api-dashboard.search.brave.com" : "ollama.com/settings/keys" })}</p><form data-form="save-key" data-provider="${provider}"><label for="${draft}">${t("APIキー")}</label><input type="password" id="${draft}" data-draft="${draft}" data-focus="${draft}" value="${e(s.drafts[draft])}" autocomplete="off" placeholder="${configured ? t("変更する場合は新しいキーを入力") : t("APIキーを入力")}" required${disabled(s)} /><div class="form-actions"><button type="submit"${disabled(s)}>${t("保存")}</button>${configured || readError ? `<button class="text-button danger-text" type="button" data-action="remove-key" data-provider="${provider}"${disabled(s)}>${t("キーを削除")}</button>` : ""}</div></form></article>`;
    })
    .join(
      "",
    )}${openAiSettings(s)}</section><p class="settings-note">${t("APIキーはこの端末の資格情報ストアに保存されます。画像検索を設定しなくても、手動登録・画像なしで比較できます。")}</p><p class="muted small">Image search powered by Brave Search API / Ollama Web Search / OpenAI</p>`;
}
function openAiSettings(s: AppState): string {
  const settings = s.settings;
  const error = settings?.errors?.openai;
  const chatgpt = s.openaiAuthMode === "chatgpt";
  const connected = settings?.chatgptConnected;
  const sharing = settings?.chatgptSharing;
  const account = settings?.chatgptAccount;
  const profiles = settings?.chatgptProfiles ?? [];
  const profileChoice = profiles.length
    ? `<label for="chatgpt-profile">${t("保存済みのChatGPTアカウント")}</label><select id="chatgpt-profile" data-focus="chatgpt-profile" aria-describedby="chatgpt-profile-help"${disabled(s)}>${settings?.chatgptActiveProfile ? "" : `<option value="" selected>${t("選択してください")}</option>`}${profiles.map((profile) => `<option value="${e(profile.id)}"${profile.id === settings?.chatgptActiveProfile ? " selected" : ""}>${e(profile.account ?? t("保存済みアカウント"))} · ${e(profile.id)}</option>`).join("")}</select><p id="chatgpt-profile-help" class="muted small">${t("保存済みのアカウントへ切り替えます。モデルIDと取得方法は変わりません。")}</p>`
    : "";
  const authentication = chatgpt
    ? `${profileChoice}<p>${connected ? t("ChatGPTに接続済み{account}", { account: account ? `: ${e(account)}` : "" }) : t("ChatGPTに接続してください。")}</p>${connected && sharing ? '<p class="badge ready">Using ChatGPT plan</p>' : connected ? `<p class="muted">${t("このアプリへの共有を許可するため、もう一度接続してください。")}</p>` : ""}<p class="muted small">${t("ChatGPTプランの利用制限が適用されます。")}</p><div class="form-actions">${!connected || !sharing ? `<button type="button" data-action="connect-chatgpt"${disabled(s)}>Continue with ChatGPT</button>` : ""}${connected || profiles.length ? `<button type="button" class="secondary" data-action="new-chatgpt-account"${disabled(s)}>${t("別のアカウントを追加")}</button>` : ""}${connected ? `<button type="button" class="text-button" data-action="chatgpt-usage"${disabled(s)}>${t("利用状況を管理")}</button><button type="button" class="text-button danger-text" data-action="disconnect-chatgpt"${disabled(s)}>${t("接続を解除")}</button>` : ""}</div>`
    : `<p class="muted small">${t("APIの都度課金が適用されます。ChatGPTプランとは別の料金です。platform.openai.com でAPIキーを取得できます。")}</p><form data-form="save-key" data-provider="openai"><label for="openaiKey">${t("APIキー")}</label><input type="password" id="openaiKey" data-draft="openaiKey" data-focus="openaiKey" value="${e(s.drafts.openaiKey)}" autocomplete="off" placeholder="${t("APIキーを入力")}" required${disabled(s)} /><div class="form-actions"><button type="submit"${disabled(s)}>${t("保存")}</button><button class="text-button danger-text" type="button" data-action="remove-key" data-provider="openai"${disabled(s)}>${t("キーを削除")}</button></div></form>`;
  return `<article class="settings-card"><div class="section-heading"><h2>OpenAI Image Search</h2><span class="badge ${configuredSearchProviders(s).includes("openai") ? "ready" : ""}">${keyStatus(s, "openai")}</span></div>${error === undefined ? "" : `<p class="error" role="alert">${e(error ? localizeBackendError(error) : t("接続状態を確認できませんでした。"))}</p>`}<p>${t("Web上の画像を検索し、候補を表示します。")}</p>${settings && settings.openaiAuthMode !== s.openaiAuthMode ? `<p class="muted small" role="status">${t("取得方法の変更は未保存です。検索には保存済みの{method}を使います。", { method: settings.openaiAuthMode === "chatgpt" ? t("ChatGPTプラン") : t("API（都度課金）") })}</p>` : ""}<form data-form="save-openai-settings"><label for="openai-auth-mode">${t("取得方法")}</label><select id="openai-auth-mode" data-focus="openai-auth-mode"${disabled(s)}><option value="api"${chatgpt ? "" : " selected"}>${t("API（都度課金）")}</option><option value="chatgpt"${chatgpt ? " selected" : ""}>${t("ChatGPTプラン")}</option></select><label for="openaiModel">${t("モデルID")}</label><input id="openaiModel" data-draft="openaiModel" data-focus="openaiModel" list="openai-models" value="${e(s.drafts.openaiModel)}" required maxlength="128" aria-describedby="openai-model-help"${disabled(s)} /><datalist id="openai-models">${s.openaiModels.map((model) => `<option value="${e(model)}"></option>`).join("")}</datalist><p id="openai-model-help" class="muted small">${t("一覧にないモデルも直接入力して保存できます。一覧への掲載は画像検索への対応を保証しません。一覧は保存済みの取得方法で取得するため、取得方法を変更したら先に保存してください。保存した取得方法とモデルは次の検索に適用されます。")}</p><div class="form-actions"><button type="submit"${disabled(s)}>${t("取得方法とモデルを保存")}</button><button type="button" class="secondary" data-action="refresh-openai-models"${s.busy || (settings && settings.openaiAuthMode !== s.openaiAuthMode) ? " disabled" : ""}>${t("モデル一覧を更新")}</button></div></form>${authentication}</article>`;
}
function sourceHost(source: string): string {
  try {
    return new URL(source).hostname;
  } catch {
    return source;
  }
}
function exportImageOption(s: AppState): string {
  return `<label class="export-image-option" for="export-include-images"><input id="export-include-images" type="checkbox" data-focus="export-include-images" aria-describedby="export-image-help"${s.exportIncludeImages ? " checked" : ""}${disabled(s)} />${t("画像を含める")}</label><p id="export-image-help" class="muted small">${t("画像は既定では含みません。再配布できる権利のある画像だけを含めてください。")}</p>`;
}
function modalView(s: AppState): string {
  const modal = s.modal;
  if (!modal) return "";
  const item =
    "itemId" in modal ? s.active?.items.find((entry) => entry.id === modal.itemId) : null;
  let title: string;
  let body: string;
  if (modal.kind === "export-list") {
    title = t("リストをエクスポート");
    body = `<p>${e(t("「{name}」のリスト名・項目名・タグを ZIP ファイルに保存します。", { name: s.active?.name ?? "" }))}</p><p class="muted">${t("比較履歴・評価は含みません。")}</p>${exportImageOption(s)}${s.busy ? `<p role="status">${t("エクスポートしています…")}</p>` : ""}<div class="form-actions"><button type="button" data-action="confirm-export"${disabled(s)}>${t("保存先を選ぶ")}</button><button type="button" class="secondary" data-action="close-modal"${disabled(s)}>${t("キャンセル")}</button></div>`;
  } else if (modal.kind === "export-report") {
    title = t("{view}を出力", { view: modal.view === "ranking" ? t("ランキング") : t("Tier 表") });
    body = `<p>${e(t("「{name}」の、現在のタグ絞り込みに一致する全項目を保存します。", { name: s.active?.name ?? "" }))}</p><label for="report-format">${t("ファイル形式")}</label><select id="report-format" data-focus="report-format"${disabled(s)}><option value="pdf"${s.reportFormat === "pdf" ? " selected" : ""}>PDF</option><option value="html"${s.reportFormat === "html" ? " selected" : ""}>HTML</option></select>${exportImageOption(s)}${s.busy ? `<p role="status">${t("表を作成・保存しています…")}</p>` : ""}<div class="form-actions"><button type="button" data-action="confirm-report-export"${disabled(s)}>${t("保存先を選ぶ")}</button><button type="button" class="secondary" data-action="close-modal"${disabled(s)}>${t("キャンセル")}</button></div>`;
  } else if (modal.kind === "import-list") {
    title = t("リストをインポート");
    body = `<p>${t("pairrank でエクスポートした ZIP ファイルから、リスト名・項目名・タグを取り込みます。画像が含まれる場合は画像も取り込みます。")}</p><p class="muted">${t("新しいリストとして追加します。既存のリストは変更せず、比較は最初から始められます。")}</p>${s.busy ? `<p role="status">${t("インポートしています…")}</p>` : ""}<div class="form-actions"><button type="button" data-action="confirm-import"${disabled(s)}>${t("ファイルを選ぶ")}</button><button type="button" class="secondary" data-action="close-modal"${disabled(s)}>${t("キャンセル")}</button></div>`;
  } else if (modal.kind === "bulk-image") {
    title = t("未登録画像を一括登録");
    const run = s.bulkRun;
    const providers = configuredSearchProviders(s);
    const missing = s.active?.items.filter((entry) => !entry.image).length ?? 0;
    let providerChoice = `<p class="muted">${t("設定済みの検索元がまだ確認できていません。")}<button type="button" class="text-button" data-view="settings">${t("検索設定を開く")}</button></p>`;
    if (providers.length > 1) {
      providerChoice = `<label for="bulk-provider">${t("検索元")}</label><select id="bulk-provider" data-focus="bulk-provider"${run || s.busy ? " disabled" : ""}><option value=""${s.bulkProvider ? "" : " selected"}>${t("選択してください")}</option>${providers.map((provider) => `<option value="${provider}"${s.bulkProvider === provider ? " selected" : ""}>${providerNames[provider]}</option>`).join("")}</select>`;
    } else if (providers.length === 1) {
      providerChoice = `<p>${t("検索元: {provider}", { provider: providerNames[providers[0]!] })}</p>`;
    }
    const summary = run
      ? `<div class="bulk-progress" role="status"><p>${t("{done} / {total} 件を処理 · 登録 {registered} 件 · 見送り {skipped} 件", { done: run.done, total: run.total, registered: run.registered, skipped: run.skipped })}</p><progress max="${run.total}" value="${run.done}" aria-label="${t("画像登録の進捗")}"></progress>${run.running ? `<p class="muted">${run.stopping ? t("現在の項目が終わり次第停止します…") : t("画像を検索して登録しています…")}</p>` : `<p>${run.stopped ? t("停止しました。") : s.error ? t("途中で停止しました。") : t("一括処理が完了しました。")}</p>`}</div>`
      : "";
    body = `<p>${t("開いているリストの画像未登録 {count} 件を処理します。各サービスの料金・利用制限が適用されます。", { count: run?.total ?? missing })}</p>${run ? "" : credentialAccess(s) + providerChoice}${summary}<div class="form-actions">${run?.running ? `<button type="button" class="secondary" data-action="stop-bulk-images"${run.stopping ? " disabled" : ""}>${run.stopping ? t("停止待ち") : t("停止")}</button>` : run ? `<button type="button" class="secondary" data-action="close-modal">${t("閉じる")}</button>` : `<button type="button" data-action="start-bulk-images"${s.busy || !s.bulkProvider ? " disabled" : ""}>${t("一括登録を開始")}</button><button type="button" class="secondary" data-action="close-modal">${t("キャンセル")}</button>`}</div>`;
  } else if (modal.kind === "tags") {
    title =
      modal.itemId === undefined
        ? t("タグを管理")
        : t("{name} のタグ", { name: item?.name ?? t("項目") });
    const tags = s.active?.tags ?? [];
    body = `${modal.itemId === undefined ? `<p class="muted">${t("このリストのタグを作成・変更・削除できます。")}</p>` : `<p class="muted">${t("チェックを変えると、この項目のタグが保存されます。")}</p>`}<div class="tag-editor-list">${
      tags.length
        ? tags
            .map((tag) => {
              const count =
                s.active?.items.filter((entry) => entry.tagIds.includes(tag.id)).length ?? 0;
              const assignment =
                modal.itemId === undefined
                  ? ""
                  : `<label class="tag-assignment"><input id="item-tag-${tag.id}" type="checkbox" data-tag-id="${tag.id}" data-focus="tag-${tag.id}"${item?.tagIds.includes(tag.id) ? " checked" : ""}${disabled(s)} /><span class="sr-only">${e(t("{item} に {tag} を付ける", { item: item?.name ?? "", tag: tag.name }))}</span></label>`;
              const actions =
                s.tagDeletingId === tag.id
                  ? `<div class="tag-delete-confirm"><span>${e(t("「{name}」を削除しますか？項目からも外れます。", { name: tag.name }))}</span><button type="button" class="danger small" data-action="confirm-tag-delete"${disabled(s)}>${t("削除する")}</button><button type="button" class="secondary small" data-action="cancel-tag-delete"${disabled(s)}>${t("キャンセル")}</button></div>`
                  : `<button type="button" class="text-button small" data-action="edit-tag" data-id="${tag.id}" aria-label="${e(t("{name} の名前を変更", { name: tag.name }))}"${disabled(s)}>${t("名前")}</button><button type="button" class="text-button small danger-text" data-action="delete-tag" data-id="${tag.id}" aria-label="${e(t("{name} を削除", { name: tag.name }))}"${disabled(s)}>${t("削除")}</button>`;
              const nameElement = modal.itemId === undefined ? "span" : "label";
              return `<div class="tag-editor-row">${assignment}<${nameElement} class="tag-editor-name"${modal.itemId === undefined ? "" : ` for="item-tag-${tag.id}"`}>${e(tag.name)} <small>${t("{count} 件", { count })}</small></${nameElement}><div class="tag-editor-actions">${actions}</div></div>`;
            })
            .join("")
        : `<p class="muted">${t("タグはまだありません。")}</p>`
    }</div><form data-form="save-tag" class="tag-form"><label for="tag-name">${s.tagEditingId === null ? t("新しいタグ名") : t("タグ名を変更")}</label><input id="tag-name" data-tag-draft data-focus="tag-draft" aria-describedby="tag-name-limit" value="${e(s.tagDraft)}" required${disabled(s)} /><p id="tag-name-limit" class="muted small">${t("{count}文字以内", { count: tagNameMaxLength })}</p><div class="form-actions"><button type="submit"${disabled(s)}>${s.tagEditingId === null ? t("タグを追加") : t("変更を保存")}</button>${s.tagEditingId !== null ? `<button type="button" class="secondary" data-action="cancel-tag-edit"${disabled(s)}>${t("キャンセル")}</button>` : ""}</div></form>${modal.itemId === undefined ? "" : `<p class="muted small">${t("ここで作成したタグは、この項目にも付きます。")}</p>`}`;
  } else if (modal.kind === "image") {
    title = t("{name} の画像", { name: item?.name ?? t("項目") });
    const configured = configuredSearchProviders(s).includes(s.provider);
    const error = s.settings?.errors?.[s.provider];
    const readError =
      error === undefined
        ? undefined
        : error
          ? localizeBackendError(error)
          : t("資格情報ストアから読み取れませんでした。");
    const settingsMessage = readError
      ? e(t("検索の接続状態を確認できません: {error}", { error: readError }))
      : t("検索の接続状態は{status}です。", { status: keyStatus(s, s.provider) });
    body = `<div class="image-options"><button class="secondary" data-action="local-image"${disabled(s)}>${t("ファイルから登録")}</button><button class="text-button" data-action="no-image"${s.busy || !item?.image ? " disabled" : ""}>${t("画像なしにする")}</button></div><div class="divider"></div><h3>${t("ネットで画像を探す")}</h3>${credentialAccess(s)}<form data-form="search-images" class="image-search"><label class="sr-only" for="search-provider">${t("検索元")}</label><select id="search-provider" data-focus="provider"${disabled(s)}><option value="brave"${s.provider === "brave" ? " selected" : ""}>Brave</option><option value="ollama"${s.provider === "ollama" ? " selected" : ""}>Ollama</option><option value="openai"${s.provider === "openai" ? " selected" : ""}>OpenAI</option></select><label class="sr-only" for="image-query">${t("検索語")}</label><input id="image-query" data-draft="query" data-focus="query" value="${e(s.drafts.query)}" required${disabled(s)} /><button type="submit"${disabled(s)}${!configured ? " disabled" : ""}>${t("検索")}</button></form>${s.provider === "openai" ? `<p class="muted small">${e(t("モデル: {model}", { model: s.settings?.openaiModel ?? s.drafts.openaiModel }))} · ${s.settings?.openaiAuthMode === "chatgpt" ? (s.settings.chatgptConnected && s.settings.chatgptSharing ? "Using ChatGPT plan" : t("ChatGPTプラン（未接続）")) : t("API（都度課金）")} <button type="button" class="text-button" data-view="settings"${disabled(s)}>${t("モデル・取得方法を変更")}</button></p>` : ""}${readError || !configured ? `<p class="muted">${settingsMessage}<button class="text-button" data-view="settings"${disabled(s)}>${t("検索設定を開く")}</button></p>` : ""}${s.busy ? `<p role="status">${t("処理中です…")}</p>` : ""}${s.searched && !s.candidates.length ? `<p class="empty compact">${t("画像が見つかりませんでした。検索語や検索元を変えてみてください。")}</p>` : ""}<div class="image-results">${s.candidates.map((candidate, index) => `<button class="image-result" data-action="choose-image" data-index="${index}"${disabled(s)}><img src="${e(candidate.previewUrl)}" alt="${e(candidate.title)}" loading="lazy" /><span>${e(candidate.title)}</span><small title="${e(candidate.sourceUrl)}">${e(sourceHost(candidate.sourceUrl))}</small></button>`).join("")}</div>${item?.image?.sourceUrl ? `<p class="muted source-url">${e(t("現在の画像の出典：{url}", { url: item.image.sourceUrl }))}</p>` : ""}`;
  } else if (modal.kind === "reset-comparisons") {
    title = t("比較をリセット");
    body = `<p>${e(t("「{name}」の比較を最初からやり直しますか？", { name: s.active?.name ?? "" }))}</p><p>${t("比較履歴・評価・比較回数・順位ほぼ確定の判定履歴を初期化します。この操作は元に戻せません。")}</p><p class="muted">${t("リスト名・項目・タグ・画像は残ります。順位は登録順に戻ります。")}</p><div class="form-actions"><button class="danger" data-action="confirm-reset-comparisons"${disabled(s)}>${t("リセットする")}</button><button class="secondary" data-action="close-modal"${disabled(s)}>${t("キャンセル")}</button></div>`;
  } else if (modal.kind === "delete-list" || modal.kind === "delete-item") {
    title = modal.kind === "delete-list" ? t("リストを削除") : t("項目を削除");
    body = `<p>${e(t("「{name}」を削除しますか？", { name: modal.kind === "delete-list" ? (s.active?.name ?? "") : (item?.name ?? "") }))}</p><p class="muted">${modal.kind === "delete-list" ? t("このリストの項目と比較履歴も削除されます。") : t("比較とランキングから除外します。他の項目の評価は維持されます。")}</p><div class="form-actions"><button class="danger" data-action="confirm-delete"${disabled(s)}>${t("削除する")}</button><button class="secondary" data-action="close-modal"${disabled(s)}>${t("キャンセル")}</button></div>`;
  } else {
    title = modal.kind === "create-list" ? t("新しいリスト") : t("名前を変更");
    body = `<form data-form="save-name"><label for="name-input">${modal.kind === "rename-item" ? t("項目名") : t("リスト名")}</label><input id="name-input" data-draft="name" data-focus="name" value="${e(s.drafts.name)}" placeholder="${t("例：好きなゲーム")}" required autofocus${disabled(s)} /><div class="form-actions"><button type="submit"${disabled(s)}>${modal.kind === "create-list" ? t("作成") : t("保存")}</button><button type="button" class="secondary" data-action="close-modal"${disabled(s)}>${t("キャンセル")}</button></div></form>`;
  }
  return `<dialog id="app-dialog" class="${modal.kind === "image" || modal.kind === "tags" ? "wide" : ""}" aria-labelledby="dialog-title"><div class="dialog-heading"><h2 id="dialog-title">${e(title)}</h2><button class="close-button" data-action="close-modal" aria-label="${t("閉じる")}"${s.readPending === "image" ? "" : disabled(s)}>×</button></div>${s.error ? `<div class="error" role="alert">${e(s.error)}</div>` : ""}${body}</dialog>`;
}
export function renderApp(s: AppState, assetUrl: (path: string) => string): string {
  if (s.fatal)
    return `<main class="startup"><div class="empty-symbol">!</div><h1>${t("データを開けませんでした")}</h1><p class="error" role="alert">${e(s.error)}</p><p>${t("データは初期化していません。原因を解消してから、アプリを再起動してください。")}</p></main>`;
  if (!s.initialized)
    return `<main class="startup" role="status"><span class="spinner"></span><h1>${t("データを準備しています")}</h1><p>${t("必要なデータ更新を確認しています。")}</p></main>`;
  let content: string;
  if (s.view === "settings") content = settingsView(s);
  else if (!s.active)
    content = `<section class="empty welcome"><p class="eyebrow">A LITTLE CHOICE, A CLEARER RANKING</p><h1>${t("好き、を並べよう。")}</h1><p>${t("ふたつを比べて、ひとつ選ぶ。")}<br />${t("小さな選択を重ねて、あなたのランキングを作ります。")}</p><button data-action="create-list">${t("最初のリストを作成")}</button></section>`;
  else {
    content = listHeader(s);
    if (s.view === "compare") content += comparisonView(s, assetUrl);
    else if (s.view === "ranking") content += rankingView(s, assetUrl);
    else if (s.view === "tier") content += tierView(s, assetUrl);
    else content += itemsView(s, assetUrl);
  }
  return `${sidebar(s)}<main class="workspace" aria-busy="${s.busy}">${!s.modal && s.error ? `<div class="error" role="alert">${e(s.error)}</div>` : ""}${s.notice ? `<div class="notice" role="status">${e(s.notice)}</div>` : ""}${content}<div class="save-status" role="status">${s.busy ? t("処理中…") : ""}</div></main>${modalView(s)}`;
}
