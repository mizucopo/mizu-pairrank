# ネイティブ PR 検証

`use_gh_actions_native_pr=true` で既存の Linux 品質ジョブに
`native-pr-checks` のマトリクスを追加します。PR と手動実行に対応し、
読み取り権限のみで動作します。Release・タグ・レジストリは更新しません。

対象は `tauri_native_pr_targets` です。Windows x64 / ARM64、macOS ARM64
が初期値で、既存の Linux 品質ジョブはそのままです。
コミット済みの `package-lock.json` と `src-tauri/Cargo.lock` を用意し、
`npm ci`、frontend build、`cargo clippy --locked` と `cargo test --locked` でネイティブの Rust 回帰を検証します。
Tauri Release の配布先や公開用マトリクスは変更しません。

## 製品固有の拡張

`use_native_pr_hook=true` を選んだ場合のみ、プロジェクトが所有する
`scripts/native-pr-hook.sh` を `before` と `after` 引数で各1回実行します。
LF 改行を使い、Git for Windows の Bash でも実行できるようにします。共通の runner 準備・
検査・検証 artifact の処理は workflow に残し、FFmpeg、ライセンス確認、
診断および追加データなど製品固有の処理を hook に入れます。
`NATIVE_PR_TARGET`、Rust の `after` では `NATIVE_PR_ARTIFACT_DIR` が参照できます。
いずれかの hook の失敗も CI を失敗させます。
認証情報、公開、production リソースへの書き込みを hook に含めません。

## 既存 PR CI から移行

1. 現行の検査・必須チェック名と固有の配布要件を記録します。
2. 分離ブランチで Copier を更新し、必要なオプションだけ有効にします。
   既存の `rust-quality-checks` / `tauri-quality-checks` ジョブ名は変わりません。
3. ネイティブ runner の実行結果を確認し、旧ワークフローと同じ検証範囲を
   確認してから重複を削除します。旧ネイティブジョブを必須チェックとして
   登録済みなら、先にマトリクス展開後の新しいチェック名を確認し、
   プロジェクトの管理者がブランチ保護を移行します。
   Copier はブランチ保護を変更しません。
