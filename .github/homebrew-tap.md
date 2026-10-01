# Homebrew Tap notification

`Tauri Distribution Release` の `notify-homebrew` ジョブは、安定版の ZIP 公開と `promote-latest` 成功後に [mizucopo/homebrew-tap](https://github.com/mizucopo/homebrew-tap) の `update-casks.yml` を起動します。

通知は Release を作成する同じワークフローの後続ジョブに置きます。`GITHUB_TOKEN` で作った Release のイベントから別ワークフローが必ず起動する、とは仮定しません。

## 設定

- Repository variable: `HOMEBREW_TAP_APP_CLIENT_ID`
- Repository secret: `HOMEBREW_TAP_APP_PRIVATE_KEY`
- Repository variable: `HOMEBREW_TAP_NOTIFY_ENABLED=true`（初回検証後に有効化）
- 専用 App は Tap だけにインストールし、Actions read/write と暗黙の Metadata read のみ付与します。Actions write は起動以外の Actions 操作も含みます
- このリポジトリの通常の `GITHUB_TOKEN` は他リポジトリにアクセスできません。通知ジョブは Contents 権限を持たず、App の短時間トークンで Tap の Actions API を呼びます
- 秘密鍵は所有者が GitHub の secret 入力欄に直接登録します。コミットやログに含めないでください

Tap は通知データを信用して更新するのではなく、登録済みアプリの最新 Release・実際の ZIP・SHA256 を自分で検証します。通知の受付成功だけでは Cask の公開完了ではありません。結果は [Tap の実行履歴](https://github.com/mizucopo/homebrew-tap/actions/workflows/update-casks.yml) と Cask のコミットで確認します。

詳しい設定、権限、停止方法、アプリ追加は [Tap の運用手順](https://github.com/mizucopo/homebrew-tap/blob/main/automation/README.md) を参照してください。

## 再実行と導入時の注意

- 一時的な通知失敗は3回まで再試行します。失敗した場合は通知ジョブだけ再実行するか、Tap のワークフローを手動実行します
- 重複通知は同一版の二重コミットを作りません
- prerelease やリリース失敗では通知しません
- ワークフローのみの導入 PR でも、既存の main リリース処理は package/Cargo/Tauri のバージョンとタグの対応を検査します。現在の `0.7.0` を変更せずに PR をマージすると、既存タグが以前のコミットを指すため、その main リリース実行は preflight で停止します。この変更ではバージョンを勝手に増やしたり、既存 Release を上書きしたりしません
- 初回は Tap の `apply=false` による既存 `0.7.0` の検証で安全に準備できます。次の正式なバージョン更新・安定版公開時に source → Tap の全経路を確認してください
