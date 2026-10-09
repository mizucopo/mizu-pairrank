# Homebrew Tap notification

この文書と `tauri-build.yml` の通知ジョブは repo-template の Copier-managed codebase に含まれます。機能変更はテンプレート側で行い、利用先には `copier update` で取り込んでください。

## 通知先と契約

- Tap: `mizucopo/homebrew-tap`
- Workflow: `.github/workflows/update-casks.yml`、ref は `main`
- 入力: boolean 型の `apply`。REST の送信 JSON では `{"ref":"main","inputs":{"apply":"true"}}` と文字列で渡します

Tauri Release の公開後、`promote-latest` がその Release を最新の完成済み Release として昇格させた場合に `workflow_dispatch` を送ります。対象は数値3要素の安定版です。prerelease と build metadata 付き version は通知しません。公開済み Release を `GITHUB_TOKEN` で作るため、別の `release: published` workflow が必ず起動するとは仮定しません。

Tap 側 workflow は事前に main へ配置し、登録済みアプリの公開 Release と実際の配布物を独立に検証する必要があります。通知本文には version・URL・SHA・release本文を渡しません。重複通知を冪等に扱い、同一版の二重更新や旧版への後退を防ぎ、複数アプリの通知が並ぶ場合も最新状態を照合してください。テンプレートは Tap の Cask や公開ロジックを生成しません。

## 資格情報と有効化

1. 専用 GitHub App を作り、`mizucopo/homebrew-tap` だけにインストールします。Repository permissions は Actions read/write と暗黙の Metadata read-only のみ。Contents・Administration・Secrets・Workflows 権限や Webhook は不要です。Actions write は起動以外の Actions 操作も含み、名前付き workflow だけの権限ではありません。
2. 利用先 repository に変数 `HOMEBREW_TAP_APP_CLIENT_ID` と secret `HOMEBREW_TAP_APP_PRIVATE_KEY` を登録します。秘密鍵は所有者が GitHub の安全な入力欄に直接登録し、チャット・コード・ログには載せません。
   - 固定した `actions/create-github-app-token` v3.2.0 は [`client-id` を定義し、`app-id` を非推奨にしています](https://github.com/actions/create-github-app-token/blob/bcd2ba49218906704ab6c1aa796996da409d3eb1/action.yml#L7-L13)。変数には App の Client ID を使います。
3. Tap の受信・検証・公開の設定を完了し、まず Tap 側で dry run を確認します。
4. 利用先の変数 `HOMEBREW_TAP_NOTIFY_ENABLED=true` を設定します。この変数が未設定または false の間、通知ジョブは実行しません。

通常の `GITHUB_TOKEN` は他の repository にアクセスできません。通知ジョブの通常トークン権限は空です。App の短時間トークンを指定した Tap の Actions write のみに絞り、公式 action の終了処理で破棄します。App の秘密鍵自体には更新・失効管理が必要です。新しい利用先に同じ鍵を登録すると、その利用先も App の権限を行使できるようになるため、明示的な承認・レビューを行ってください。

## 失敗・停止・更新

- 通知APIを最大3回試し、それでも失敗したらジョブを失敗にします。失敗ジョブを再実行するか、Tap 側を手動実行します
- 受付成功は Tap の検証・公開成功ではありません。Tap 側の実行結果と Cask のコミットを確認してください
- 停止するには利用先の `HOMEBREW_TAP_NOTIFY_ENABLED=false`。生成ジョブと文書を削除するには clean な非 main branch で `copier update --trust --defaults --vcs-ref HEAD -d use_gh_actions_tauri_homebrew_notify=false` を実行して差分をレビューします
- Tauri配布を無効にした場合も通知ジョブ・文書は生成されません
- 通知設定の変更を公開するかどうかと分類は [CONTRIBUTING.md](../CONTRIBUTING.md#リリース分類) に従います。マージ後は最新のマージ済み PR 一つの分類で最新 main を自動採番し、採番 commit と同じタグの Release を同じ Actions run で公開します。通常は手動の version 更新は不要です
- 指定した分類の不正・設定・権限不足、照会失敗、公開済み状態の不整合では採番・公開を停止します。復旧は [採番・公開](release.md) に従い、原因を確認して元 run を再実行します。通知ジョブ自体は採番や既存 Release の変更を行いません

生成後の有効化はそのプロジェクトの選択です。他の利用先に既定で通知や資格情報が追加されることはありません。

参考: [workflow dispatch API](https://docs.github.com/en/rest/actions/workflows#create-a-workflow-dispatch-event)、[GITHUB_TOKEN と workflow 起動](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow)、[GitHub App の認証](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/making-authenticated-api-requests-with-a-github-app-in-a-github-actions-workflow)
