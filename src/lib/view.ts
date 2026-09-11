import { answers } from "./controller.js";
import type { AppState, View } from "./controller.js";
import type { Item } from "./types.js";

export function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (c) => {
    switch (c) {
      case "&":
        return "&amp;";
      case "<":
        return "&lt;";
      case ">":
        return "&gt;";
      case '"':
        return "&quot;";
      default:
        return "&#39;";
    }
  });
}
const e = escapeHtml;
const disabled = (state: AppState): string => (state.busy ? " disabled" : "");
const number = (value: number): string => value.toFixed(2);

function picture(item: Item, assetUrl: (path: string) => string, large = false): string {
  return item.image
    ? `<img class="item-image${large ? " large" : ""}" src="${e(assetUrl(item.image.path))}" alt="${e(item.name)}" ${large ? "" : 'loading="lazy"'} /><span class="image-fallback" hidden>画像を表示できません</span>`
    : `<span class="item-image placeholder${large ? " large" : ""}" aria-hidden="true">${e(Array.from(item.name)[0] ?? "・")}</span>`;
}
function sidebar(s: AppState): string {
  return `<aside class="sidebar"><div class="brand"><span class="brand-mark" aria-hidden="true">p<span>r</span></span><div>pairrank<small>あなたの好みを、順番に。</small></div></div><div class="sidebar-heading"><h2>マイリスト</h2><span>${s.lists.length}</span></div><nav class="lists" aria-label="リスト">${s.lists.map((list) => `<button class="list-link${list.id === s.active?.id ? " selected" : ""}" data-action="select-list" data-id="${list.id}"${disabled(s)} ${list.id === s.active?.id ? 'aria-current="true"' : ""}><span class="list-name">${e(list.name)}</span><small>${list.itemCount} 項目<span>${list.converged ? "ほぼ確定 ✓" : `${list.comparisonCount} 回比較`}</span></small></button>`).join("")}</nav><button class="new-list secondary" data-action="create-list"${disabled(s)}>＋ リストを作成</button><div class="sidebar-footer"><span class="local-dot"></span>この端末に自動保存<button class="text-button" data-view="settings"${disabled(s)}>検索設定</button></div></aside>`;
}
function progress(s: AppState): string {
  const list = s.active;
  if (!list) return "";
  const c = list.convergence;
  return `<details class="progress-details"><summary>${c.converged ? "✓ 順位ほぼ確定" : "◌ 順位を整理中"}<span>${list.comparisonCount} 回比較済み</span></summary><div class="progress-body"><p>全項目の不確実性 σ が2.5以下になり、直近${c.requiredAnswers}回答で各項目の順位の動きが1位以内になると「順位ほぼ確定」です。</p><div class="progress-numbers"><span>最大 σ <strong>${number(c.maxSigma)}</strong> / 2.50</span><span>順位の動き <strong>${c.maxRankSpan ?? "—"}</strong> / 1位</span><span>判定履歴 <strong>${c.observedAnswers}</strong> / ${c.requiredAnswers} 回</span></div><progress aria-label="終了判定に必要な履歴" max="${c.requiredAnswers}" value="${c.observedAnswers}"></progress></div></details>`;
}
function listHeader(s: AppState): string {
  const list = s.active;
  if (!list) return "";
  const tabs: { view: View; label: string }[] = [
    { view: "items", label: "項目" },
    { view: "compare", label: "比較する" },
    { view: "ranking", label: "ランキング" },
  ];
  return `<header class="page-header"><div><p class="eyebrow">MY RANKING</p><h1>${e(list.name)}</h1><p class="muted">${list.items.length} 項目 · ${list.comparisonCount} 回の比較</p></div><div class="header-actions"><button class="text-button" data-action="rename-list"${disabled(s)}>名前を変更</button><button class="text-button danger-text" data-action="delete-list"${disabled(s)}>削除</button></div></header><nav class="tabs" aria-label="表示切り替え">${tabs.map(({ view, label }) => `<button data-view="${view}" class="${s.view === view ? "active" : ""}" ${s.view === view ? 'aria-current="page"' : ""}${disabled(s)} ${view === "compare" && list.items.length < 2 ? "disabled" : ""}>${label}</button>`).join("")}</nav>`;
}
function itemsView(s: AppState, assetUrl: (path: string) => string): string {
  const list = s.active;
  if (!list) return "";
  return `<section><div class="section-heading"><div><h2>比べたいものを追加</h2><p class="muted">1行に1項目。画像は追加後に選べます。</p></div></div><form data-form="add-items" class="add-items"><label class="sr-only" for="item-names">項目名（改行で複数登録）</label><textarea id="item-names" data-draft="items" data-focus="items" rows="3" placeholder="気になるもの、お気に入りのもの…" required${disabled(s)}>${e(s.drafts.items)}</textarea><div><span class="muted">画像なしでも比較できます</span><button type="submit"${disabled(s)}>項目を追加</button></div></form><div class="section-heading"><h2>登録した項目 <span class="count">${list.items.length}</span></h2>${list.items.length >= 2 ? `<button class="secondary" data-view="compare"${disabled(s)}>比較を始める →</button>` : ""}</div>${
    list.items.length === 0
      ? '<div class="empty compact"><span class="empty-symbol">＋</span><h3>最初の2項目を追加しましょう</h3><p>どちらが好きかを答えると、少しずつ順位が見えてきます。</p></div>'
      : `<div class="item-list">${[...list.items]
          .sort((a, b) => a.id - b.id)
          .map(
            (item) =>
              `<article class="item-row">${picture(item, assetUrl)}<div class="item-description"><h3>${e(item.name)}</h3><small>${item.comparisonCount} 回比較 ${item.comparisonCount === 0 ? '· <span class="new-badge">未評価</span>' : ""}</small></div><div class="item-actions"><button class="secondary small" data-action="image" data-id="${item.id}"${disabled(s)}>画像</button><button class="text-button small" data-action="rename-item" data-id="${item.id}"${disabled(s)}>名前</button><button class="text-button small danger-text" data-action="delete-item" data-id="${item.id}"${disabled(s)}>削除</button></div></article>`,
          )
          .join("")}</div>`
  }</section>`;
}
function comparisonView(s: AppState, assetUrl: (path: string) => string): string {
  const pair = s.pair;
  if (!pair)
    return `<div class="empty"><div class="empty-symbol">⇄</div><h2>${s.busy ? "次の比較を選んでいます" : "比較を再開できます"}</h2><p>順位を知る手がかりになるペアを選びます。</p>${s.busy ? '<span class="spinner" aria-hidden="true"></span>' : '<button data-view="compare">次のペアを表示</button>'}</div>`;
  return `<section class="comparison"><div class="comparison-heading"><p class="eyebrow">FOLLOW YOUR PREFERENCE</p><h2>どちらが好きですか？</h2><p class="muted">「大好き」は、相手よりかなり好き。「好き」は、少し好き。</p></div><div class="pair-cards">${(
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
    )}<span class="versus" aria-hidden="true">or</span></div><div class="answers" role="group" aria-label="比較への回答">${answers.map((answer) => `<button class="answer ${answer.value}" data-answer="${answer.value}"${disabled(s)}>${answer.label}<kbd>${answer.key}</kbd></button>`).join("")}</div><p class="comparison-note">${s.busy ? "回答を保存して、次の比較を選んでいます…" : "直感で選んで大丈夫。キーボードの1〜5でも回答できます。"}</p></section>${progress(s)}`;
}
function rankingView(s: AppState, assetUrl: (path: string) => string): string {
  const list = s.active;
  if (!list) return "";
  return `<section>${list.convergence.converged ? '<div class="converged-banner"><span>✓</span><div><h2>順位ほぼ確定</h2><p>好みの順番が落ち着きました。いつでも比較を続けられます。</p></div></div>' : ""}<div class="section-heading"><div><h2>あなたのランキング</h2><p class="muted">推定評価の高い順。評価が同じ場合は登録順です。</p></div>${list.items.length >= 2 ? `<button data-view="compare"${disabled(s)}>${list.convergence.converged ? "比較を続ける" : "比較する"} →</button>` : ""}</div>${list.items.length ? `<ol class="ranking-list">${list.items.map((item, index) => `<li class="rank-row"><span class="rank-number${index < 3 ? " top" : ""}">${index + 1}</span>${picture(item, assetUrl)}<div class="item-description"><h3>${e(item.name)}</h3><small>${item.comparisonCount} 回比較</small></div><div class="rating-values"><span>評価 <strong>${number(item.rating.mu)}</strong></span><span>σ <strong>${number(item.rating.sigma)}</strong></span></div></li>`).join("")}</ol>` : '<div class="empty compact"><p>項目を追加すると、ここに順位が表示されます。</p></div>'}</section>${progress(s)}`;
}
function settingsView(s: AppState): string {
  return `<header class="page-header"><div><p class="eyebrow">SETTINGS</p><h1>画像検索の設定</h1><p class="muted">使いたいサービスのAPIキーを登録してください。</p></div></header><section class="settings-grid">${(
    ["brave", "ollama"] as const
  )
    .map((provider) => {
      const configured =
        provider === "brave" ? s.settings?.braveConfigured : s.settings?.ollamaConfigured;
      const draft = provider === "brave" ? "braveKey" : "ollamaKey";
      return `<article class="settings-card"><div class="section-heading"><h2>${provider === "brave" ? "Brave Search" : "Ollama Web Search"}</h2><span class="badge ${configured ? "ready" : ""}">${configured ? "設定済み" : "未設定"}</span></div><p>${provider === "brave" ? "Web上の画像を検索し、候補を表示します。" : "検索したWebページから代表画像を取得します。"}</p><p class="muted small">${provider === "brave" ? "api-dashboard.search.brave.com" : "ollama.com/settings/keys"} でAPIキーを取得できます。各サービスの料金・利用制限が適用されます。</p><form data-form="save-key" data-provider="${provider}"><label for="${draft}">APIキー</label><input type="password" id="${draft}" data-draft="${draft}" data-focus="${draft}" value="${e(s.drafts[draft])}" autocomplete="off" placeholder="${configured ? "変更する場合は新しいキーを入力" : "APIキーを入力"}" required${disabled(s)} /><div class="form-actions"><button type="submit"${disabled(s)}>保存</button>${configured ? `<button class="text-button danger-text" type="button" data-action="remove-key" data-provider="${provider}"${disabled(s)}>キーを削除</button>` : ""}</div></form></article>`;
    })
    .join(
      "",
    )}</section><p class="settings-note">APIキーはこの端末の資格情報ストアに保存されます。画像検索を設定しなくても、手動登録・画像なしで比較できます。</p><p class="muted small">Image search powered by Brave Search API / Ollama Web Search</p>`;
}
function sourceHost(source: string): string {
  try {
    return new URL(source).hostname;
  } catch {
    return source;
  }
}
function modalView(s: AppState): string {
  const modal = s.modal;
  if (!modal) return "";
  const item =
    "itemId" in modal ? s.active?.items.find((entry) => entry.id === modal.itemId) : null;
  let title: string;
  let body: string;
  if (modal.kind === "image") {
    title = `${item?.name ?? "項目"} の画像`;
    const configured =
      s.provider === "brave" ? s.settings?.braveConfigured : s.settings?.ollamaConfigured;
    body = `<div class="image-options"><button class="secondary" data-action="local-image"${disabled(s)}>ファイルから登録</button><button class="text-button" data-action="no-image"${disabled(s)}>画像なしにする</button></div><div class="divider"></div><h3>ネットで画像を探す</h3><form data-form="search-images" class="image-search"><label class="sr-only" for="search-provider">検索元</label><select id="search-provider" data-focus="provider"${disabled(s)}><option value="brave"${s.provider === "brave" ? " selected" : ""}>Brave</option><option value="ollama"${s.provider === "ollama" ? " selected" : ""}>Ollama</option></select><label class="sr-only" for="image-query">検索語</label><input id="image-query" data-draft="query" data-focus="query" value="${e(s.drafts.query)}" required${disabled(s)} /><button type="submit"${disabled(s)}${!configured ? " disabled" : ""}>検索</button></form>${!configured ? '<p class="muted">APIキーが未設定です。<button class="text-button" data-view="settings">検索設定を開く</button></p>' : ""}${s.busy ? '<p role="status">処理中です…</p>' : ""}${s.searched && !s.candidates.length ? '<p class="empty compact">画像が見つかりませんでした。検索語や検索元を変えてみてください。</p>' : ""}<div class="image-results">${s.candidates.map((candidate, index) => `<button class="image-result" data-action="choose-image" data-index="${index}"${disabled(s)}><img src="${e(candidate.previewUrl)}" alt="${e(candidate.title)}" loading="lazy" /><span>${e(candidate.title)}</span><small title="${e(candidate.sourceUrl)}">${e(sourceHost(candidate.sourceUrl))}</small></button>`).join("")}</div>${item?.image?.sourceUrl ? `<p class="muted source-url">現在の画像の出典：${e(item.image.sourceUrl)}</p>` : ""}`;
  } else if (modal.kind === "delete-list" || modal.kind === "delete-item") {
    title = modal.kind === "delete-list" ? "リストを削除" : "項目を削除";
    body = `<p>「${e(modal.kind === "delete-list" ? (s.active?.name ?? "") : (item?.name ?? ""))}」を削除しますか？</p><p class="muted">${modal.kind === "delete-list" ? "このリストの項目と比較履歴も削除されます。" : "比較とランキングから除外します。他の項目の評価は維持されます。"}</p><div class="form-actions"><button class="danger" data-action="confirm-delete"${disabled(s)}>削除する</button><button class="secondary" data-action="close-modal"${disabled(s)}>キャンセル</button></div>`;
  } else {
    title = modal.kind === "create-list" ? "新しいリスト" : "名前を変更";
    body = `<form data-form="save-name"><label for="name-input">${modal.kind === "rename-item" ? "項目名" : "リスト名"}</label><input id="name-input" data-draft="name" data-focus="name" value="${e(s.drafts.name)}" placeholder="例：好きなゲーム" required autofocus${disabled(s)} /><div class="form-actions"><button type="submit"${disabled(s)}>${modal.kind === "create-list" ? "作成" : "保存"}</button><button type="button" class="secondary" data-action="close-modal"${disabled(s)}>キャンセル</button></div></form>`;
  }
  return `<dialog id="app-dialog" class="${modal.kind === "image" ? "wide" : ""}" aria-labelledby="dialog-title"><div class="dialog-heading"><h2 id="dialog-title">${e(title)}</h2><button class="close-button" data-action="close-modal" aria-label="閉じる"${s.searching ? "" : disabled(s)}>×</button></div>${s.error ? `<div class="error" role="alert">${e(s.error)}</div>` : ""}${body}</dialog>`;
}
export function renderApp(s: AppState, assetUrl: (path: string) => string): string {
  if (s.fatal)
    return `<main class="startup"><div class="empty-symbol">!</div><h1>データを開けませんでした</h1><p class="error" role="alert">${e(s.error)}</p><p>データは初期化していません。原因を解消してから、アプリを再起動してください。</p></main>`;
  if (!s.initialized)
    return '<main class="startup" role="status"><span class="spinner"></span><h1>データを準備しています</h1><p>必要なデータ更新を確認しています。</p></main>';
  let content: string;
  if (s.view === "settings") content = settingsView(s);
  else if (!s.active)
    content =
      '<section class="empty welcome"><p class="eyebrow">A LITTLE CHOICE, A CLEARER RANKING</p><h1>好き、を並べよう。</h1><p>ふたつを比べて、ひとつ選ぶ。<br />小さな選択を重ねて、あなたのランキングを作ります。</p><button data-action="create-list">最初のリストを作成</button></section>';
  else {
    content = listHeader(s);
    if (s.view === "compare") content += comparisonView(s, assetUrl);
    else if (s.view === "ranking") content += rankingView(s, assetUrl);
    else content += itemsView(s, assetUrl);
  }
  return `${sidebar(s)}<main class="workspace" aria-busy="${s.busy}">${!s.modal && s.error ? `<div class="error" role="alert">${e(s.error)}</div>` : ""}${s.notice ? `<div class="notice" role="status">${e(s.notice)}</div>` : ""}${content}<div class="save-status" role="status">${s.busy ? "処理中…" : ""}</div></main>${modalView(s)}`;
}
