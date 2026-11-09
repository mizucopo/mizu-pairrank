# Security Checks

`.github/workflows/security-checks.yml` と検査用ファイルはCopierが管理します。共通の修正は `repo-template` で行い、生成先には `copier update` で取り込みます。両方の検査は初期状態でOFFです。既存の品質CI、Dependabot、CodeQLとは独立しています。

## 有効化と無効化

cleanな専用branchで、Python projectには次の回答を設定します。

```sh
copier update --trust --defaults --vcs-ref HEAD -d use_gh_actions_bandit=true
```

RustまたはTauri projectには次の回答を設定します。

```sh
copier update --trust --defaults --vcs-ref HEAD -d use_gh_actions_cargo_audit=true
```

両方のruntimeを使うprojectでは、両方の回答を同じコマンドへ追加できます。Python検査には `use_python=true`、Rust検査には `use_rust=true` または `use_tauri=true` が必要です。CodeQLの回答・GitHub設定は変更しません。無効化するときは対応する回答をfalseにしてupdateし、生成ファイルの削除差分も確認します。独自に変更したファイルはCopierが競合として残す場合があるため、不要な検査だけを取り除きます。

検査はmain向けPR、mainへのpush、毎週月曜03:37 UTC、手動実行で動きます。Actionsの **Security Checks** と各jobのログで対象・結果を確認してください。検出と検査そのものの失敗を区別して確認し、失敗を無視して成功扱いにしません。必要な必須チェック設定は運用者が別途判断します。

## Python: Bandit

Pythonはprojectの `.python-version` を使用します。固定版Banditを隔離した環境へ導入し、`src` と `.github/scripts` を再帰的に検査します。製品依存のinstall、製品のimport・実行、実Secret取得は行いません。全severity／confidenceの指摘を失敗として扱います。

```sh
uv run --isolated --no-project --with bandit==1.9.4 python -I .github/scripts/run-bandit.py
```

実行補助はBanditの終了コードに加えてJSONの `errors` と `results`、内部エラーのログを確認します。構文エラーや読取り失敗で対象が飛ばされても成功にはしません。対象ディレクトリ・`.bandit.yml` の欠落、壊れた出力、Pythonファイルが一つも解析されない場合も失敗です。新規applicationの空の `src` は許容しますが、検査用Python script自体は解析されます。したがって空starterの成功は製品コードを検査したという意味ではありません。

対象に `tests`、venv、依存ライブラリを含めません。製品コードが既定の2ディレクトリ外にもある場合は、`.github/scripts/run-bandit.py` の `TARGETS` へ追加し、Copier更新時にその変更を保持・検証します。root外への参照や広い除外を追加しないでください。

初回有効化時には、既存の指摘も確認・修正します。誤検知または受容する指摘は理由を記録し、ルールIDを限定した `# nosec Bxxx` を使います。`.bandit.yml` は必要な限定除外・ルール設定用です。無条件の `nosec`、全体的なskip、大量のbaseline、`--exit-zero` で検査を形だけ通す運用はしません。設定・個別例外はprojectごとにレビューし、Copier更新時に維持します。

Banditはよくある危険パターンを検出します。全ての脆弱性、データフロー、実行時の秘密漏洩、アクセス制御を保証する検査ではありません。コードレビュー、テスト、依存更新を継続します。

## Rust: cargo-audit

RustSecのadvisory DBを毎回取得し、コミット済みの `Cargo.lock` を検査します。通常のRust projectはroot、Tauriは `src-tauri` を作業ディレクトリにします。元のRust toolchain設定は変更せず、scannerはproject外の一時ディレクトリでrunnerの既存Rust toolchainを使って導入します（cargo-audit 0.22.2にはRust 1.88以降が必要です）。検査は導入済みバイナリを直接呼び、projectのCargo aliasを経由しません。

新規projectにはlockfileがないため、開発環境で `cargo generate-lockfile` を実行し、内容を確認してからコミットしてください。Tauriでは `src-tauri` 内で実行します。CIがlockfileを自動生成・変更することはありません。未追跡・空・不正なlockfileは失敗します。

```sh
SECURITY_TOOLS="$(mktemp -d)"
(cd "$SECURITY_TOOLS" && cargo install cargo-audit --version 0.22.2 --locked --root "$SECURITY_TOOLS")
# Tauriの場合は、ここから src-tauri 内で実行する。
git ls-files --error-unmatch Cargo.lock && test -s Cargo.lock
"$SECURITY_TOOLS/bin/cargo-audit" audit --file Cargo.lock --no-yanked
```

advisory DBの取得・読取り失敗や脆弱性の検出はjobを失敗させます。`--file` の明示によりlockfileの自動生成を避けます。`--no-yanked` は既知脆弱性への照合に範囲を限定し、registry取得失敗を警告だけで続行するyanked検査を使わないための指定です。運用で `--no-fetch`、`--stale`、`continue-on-error` を追加しないでください。

依存関係に脆弱性がある場合は更新・置換を優先し、受容する場合は影響と理由をレビューしてください。既存の `.cargo/audit.toml` がある場合は、適用範囲・除外・DB設定も確認します。Tauriでは `src-tauri/.cargo/audit.toml` が対象です。

cargo-auditはRustソースの静的セキュリティ解析ではなく、ライセンス・禁止crate・取得元を制限するcargo-denyの代わりでもありません。通常のClippyとテストを維持します。ソースの追加解析には、利用条件を満たすrepositoryで既存のCodeQLオプションを検討します。

## 権限、結果、更新

jobの権限は `contents: read` だけで、checkout credentialを保存しません。`pull_request_target`、独自token、本番Secret、外部SASTサービス、SARIFのCode scanning uploadを使いません。ソース・検査結果はrunnerと既存のActionsログ内で扱います。別のSAST SaaSへ送る構成ではありません。依存パッケージ・advisory DBの取得にはネットワーク接続が必要です。

private repositoryではGitHub Actionsの利用枠・料金・保存条件が適用されます。GitHub Code scanningがDisabledのままでも、このworkflowは独立して実行できます。検査成功は脆弱性が一切ないことの保証ではありません。

Actionsはfull commit SHA、scannerはBandit 1.9.4とcargo-audit 0.22.2を固定しています。Banditの推移依存は隔離環境で解決し、cargo-auditのinstallは公開版のlockfileを使います。DependabotのGitHub Actions監視だけではshell内のscanner版を更新しません。scannerの版はtemplateのworkflow・実行例・回帰テスト・template品質CIを同じPRで更新し、生成・更新テストと固定版scannerの実動作を確認してから採用先へCopier更新します。定期的に新しいscanner releaseを確認し、advisory DBの更新とscanner自体の更新を混同しません。

- [Banditの設定と限定例外](https://bandit.readthedocs.io/en/latest/config.html)
- [RustSecとcargo-audit](https://rustsec.org/)
- [Code scanningの利用条件](https://docs.github.com/en/code-security/concepts/code-scanning/code-scanning)
