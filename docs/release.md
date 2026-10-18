# 採番・公開

## 設定

- squash merge を使う。分類基準、公開の選択と投稿者・メンテナーの手順は [CONTRIBUTING.md](../CONTRIBUTING.md#リリース分類) に従う。通常のテスト・lint は公開の選択にかかわらず実行する。
- 手動公開では main branch を指定する。main 以外の branch・tag を指定すると、checkout・認証設定・採番の前に失敗し、main を指定する案内を表示する。
- 標準 GITHUB_TOKEN に contents:write と pull-requests:read を許す。main とタグへの直接 push を許す repository 設定を使う。force push は不要。PR 必須等の設定で拒否される場合は設定を確認し、workflow 側で保護を回避しない。
- Docker Hub は DOCKERHUB_TOKEN、ECR は AWS_ROLE_ARN と OIDC の権限を設定する。採番時にも公開先を照会するため、image の照会権限が必要。
- version source、lockfile、公開タグ・画像は .github/release.json の version と publication に宣言する。専用 App、署名鍵、CI 登録、初回移行 helper は不要。

## 通常の流れ

PR 品質 CI → squash merge → 最新のマージ済み PR の分類で最新 main を採番 → 採番 commit とタグの atomic push → 同じ Actions run 内でビルド・検証・公開。

公開 workflow 全体は release-main の concurrency group に入り、queue:max、cancel-in-progress:false で直列化する。実行定義・script・設定が待機中に変わった場合は、その変更の新しい run に任せる。開始時点の最新 main の履歴を新しい順に確認し、前回採番以後で最後にマージされた PR 一つの分類だけを使う。main 上の commit と PR のマージイベントの対応で選び、作成日時・PR 番号・run の待機順をマージ順の代用にしない。未マージ PR、別 branch へのマージ、採番 commit は分類対象にしない。

公開の可否は [CONTRIBUTING.md](../CONTRIBUTING.md#リリース分類) の方針に従う。マージイベントの証拠が取得できない場合は、古い PR の分類へ戻らず停止する。公開対象は現在の main 全体とする。

選んだ PR のマージイベント、記録された base SHA、main の first-parent 履歴、各 commit を取り込んだ PR の GitHub API 応答で、main に一つの commit として取り込まれたことを検証する。元 PR が複数 commit でも、通常の squash merge は受け入れる。merge commit と、base 以後の main に同じ PR の中間 commit がある複数 commit の rebase merge は、採番・公開の前に停止する。base の証拠や API 照会が不明な場合も停止する。

履歴の関連 PR は GraphQL で 50 commit ずつまとめて取得し、PR 接続のページ送りも確認する。古い base から commit ごとに REST リクエストを発行しない。ECR の事前 intent と採番 prepare はそれぞれ最新 main の証拠を取り直し、事前照会後に履歴が更新されても古い判定を再利用しない。GraphQL のエラー・部分応答・取得不能も公開前に停止する。

記録された base は、別 PR や直接 push による main 更新より古いことがあるため、base 以後の総 commit 数だけでは判定しない。base 以前の関連 commit と過去 PR の取り込み方を再検証したり、過去 PR の分類を集計したりしない。一つの commit だけを取り込む rebase と squash はこの証拠では区別できず、一つの main commit として受け入れる。GitHub のマージ方法は squash を選ぶ。

初回は新公開設定を導入した commit の親以後を対象とする。新規 repository の root commit だけでは公開しない。最初のラベル付き PR のマージ後、設定した初期 version とその分類を使って採番する。設定導入前の過去 PR を再分類しない。main への通常変更は squash-merged PR で行う。

旧テンプレートの plain `version` が `v1.2.5` のような値の場合、Copier update の PR で現在の値を厳格な SemVer（`1.2.5`）に直す。SemVer 方式の初回採番だけ、設定導入 commit の親にある text 形式の `version` の先頭の小文字 `v` を一つ除き、残りを厳格に検証して基準にする。ラベルなし導入後の最初の公開でも同じ基準を使う。JSON/TOML、他の path・番号方式、設定導入後の値と採番済み履歴には適用せず、不正値や複数 source の不整合は停止する。過去の version・tag・image・Release は変更しない。

番号規則は SemVer、Chrome manifest version、upstream-revision を使う。SemVer 0.x の major は 1.0.0。prerelease patch は末尾数値を増やし、channel・core・安定版への変更は明示 version を必要とする。明示 version は分類に必要な更新幅以上の下限指定。Chrome の各成分は 0～65535で、上限到達時は停止する。upstream-revision は上流 version を保持して revision を進め、新しい上流 version では r0 から始める。

SemVer の core を増やす場合、その core の prerelease も明示できる。例えば 0.2.3 から patch は 0.2.4-rc.1、minor は 0.3.0-rc.1、major は 1.0.0-rc.1 を開始できる。既存 prerelease の patch は次の番号以上を必要とし、同じ番号や前の番号への巻き戻しは拒否する。

Git tag、draft を含む Release、設定した image tag の未使用を確認する。衝突は次の番号へ進むが、認証・通信・照会の失敗を未作成と扱わない。latest は衝突対象にしない。version/manifest と既存 lockfile の version 項目だけを更新する。

採番 commit は元の main を親とし、番号、タグ、分類対象の PR 一つ、元の SHA、run ID を trailer に記録する。main とタグを一度に push する。競合したら最新 main のマージ済み PR を選び直し、最大三回まで再計算する。保護設定や認証の拒否では停止する。

後続 job は採番 commit の SHA を checkout する。GITHUB_SHA は元イベントの SHA のままなので、公開対象には使わない。タグ作成による別 workflow の起動を待たず、同じ run の後続 job で完結させる。

全検証・配布物が完成してから Release を公開する。GitHub Latest・Docker latest は最新の完成済み安定版 Release にだけ更新する。汎用 GitHub Release、Docker、Docker project、Tauri の prerelease は Latest の照会・昇格対象外で、初回公開でも Latest を変更しない。過去に通常 Release として公開された prerelease も採番記録から除外する。Tauri の prerelease は Tap 通知の対象外。

Release 作成応答の ID を後続の asset 添付・配布案内・draft 公開へ引き継ぐため、作成直後の一覧反映を待たずに完了できる。再実行時は全ページの一覧から既存 Release と重複を確認し、完成済み asset を保持する。

Release の分類は共通公開計画で採番済み VERSION と方式から決める。build metadata は分類に影響しない。通常 SemVer の 1.2.3-r1 は prerelease。upstream-revision の上流 VERSION が 1.2.3 なら公開 tag 1.2.3-r1 は通常 Release・Latest 候補で、上流 VERSION が 1.2.3-rc.1 なら prerelease。公開 tag の revision suffix だけで分類しない。

共通 `release.py` と Tauri・Chrome Extension の配布 workflow は、既存公開 Release の tag・ID・`prerelease` の型と公開計画との一致を確認してから再利用する。完成済み asset が揃っていても検証する。不一致では本文・asset・分類を変更せず停止する。採番記録と実際の公開状態を確認し、運用者が metadata を正しい計画へ修正してから元 run を再実行する。stable が `prerelease: true` のままだと Latest 候補から外れる。draft の分類は配布完了時に計画へ揃える。

Docker image tag は展開後の `+` を `_` に変換する（例 `1.2.3+build.1` → `1.2.3_build.1`）。SemVer に `_` は使えないため prerelease の `-` と衝突せず、Git version・tag・Release 名は元の値を保持する。有効な既存 image tag は変わらない。変換後にも tag の長さ・形式・使用済み状態・所有者を検証する。

Docker Release の公開後確認は `image-verify` で同じ immutable tag を最大 6 回照会し、不在の間だけ 5 秒間隔で待つ（待機は最大 5 回・計 25 秒、HTTP request の timeout は各 30 秒）。Docker Hub は既知 repository の Registry manifest HEAD の 404、ECR は ImageNotFoundException が待機対象。repository 自体の 404、401/403、429、5xx、redirect、通信失敗・timeout、不正 digest は再試行せず失敗する。上限到達時は試行回数と間隔を error に示し、GitHub Release の作成・公開と latest 昇格へ進まない。途中ログには試行回数と待機間隔だけを載せ、認証 token や応答本文は載せない。

Docker Hub の共通 image 照会は、repository-level の権限・可視性確認を保ち、タグの存在と digest は実レジストリ `registry-1.docker.io` の manifest HEAD と `Docker-Content-Digest` で検証する。Hub のタグ管理 API は、Registry で取得できる image に対して古い情報や 404 を返すことがあるため使わない。採番時の衝突確認、`image-state`、`image-verify`、配布案内も同じ照会を使う。

既存の `DOCKERHUB_USERNAME` / `DOCKERHUB_TOKEN` がある場合は Hub 用 JWT と Registry 用の `repository:<name>:pull` scope token を別々に取得する。PAT は固定 HTTPS の認証先だけへ渡し、各 bearer は対応する API だけへ送る。設定済み認証の失敗から匿名へ fallback せず、HTTP redirect も拒否する。資格情報がない公開 repository は匿名の Registry token で確認できる。Docker / OCI の manifest と index を受け入れ、digest の欠落・不正値は失敗にする。HEAD は image や layer を download しない。

採番時の未使用判定と公開前の `image-state` は単発照会のまま維持する。確認中に image を再 build/push・上書きしない。部分公開後は元 run を再実行し、同じ commit/tag の所有権を検証した既存 image と完成済み native assets を再利用して、不足工程だけを続ける。公開直後の不在は反映遅延の可能性があるが、再試行成功だけで原因を断定しない。

## Docker 所有記録の完了後整理

標準 Docker Release と Docker project pipeline は、同じ `cleanup-image-owners` job を使う。`release-main` の直列化を保持し、公開 job（全 image・Rust 配布物・Downloads の更新・draft 公開）と必要な `promote-latest` が成功した後だけ実行する。prerelease と `latest_image: null` は Latest job の skip を許す。失敗・キャンセル・進行中・部分公開では記録を残す。cleanup のみ失敗した場合も公開は維持し、全 job を再実行して再確認する。

共通 `release.py image-owner-complete` は採番 trailer の元 run ID と Actions API の最新 attempt・job 結果を照合する（`actions: read` が必要）。公開 Release の tag・ID・分類、全 image の registry digest、同じ run の検証済み Rust manifest・配布物と対応する asset（実行ファイルを作らない library-only 等は archive 不要）、現在の Downloads 本文を再確認する。公開 archive の digest（API 未提供なら実 bytes）から checksum を求め、公開済み `.sha256` の実内容と名前を照合する。不一致・asset の欠落や重複・API/ダウンロード失敗では証拠の保存と所有記録の削除前に停止する。再ビルドによる圧縮 bytes の違いは許し、公開済み archive を上書きしない。最新の完成済み安定版なら GitHub Latest と Docker latest の digest も確認する。古い版は新しい Latest を巻き戻さない。

全 image の所有記録 ref が `RELEASE_SHA` と一致することを確認してから、Release 本文の専用 HTML comment `repo-template:image-owners:v1` に schema・採番 SHA・release tag・全 image の宣言（registry/account/region/repository/tag を含む）と digest を保存し、読み返して確認する。既存の記録が異なる場合は書き換えない。リリースノートと Downloads は保持する。この証拠は既存の Git ref・Release と同じ repository の書き込み権限を信頼する。Release 編集時も専用 comment を保持する。

その後、公開計画にある `refs/heads/automation/docker-images/<image-tag>` だけを削除する。各 ref を読み直し、`--force-with-lease=<ref>:<RELEASE_SHA>` を付けて削除するため、別 SHA への競合では停止する。既に削除済みなら成功する。途中で一部だけ削除された場合も Release の証拠は残り、全 job の再実行で再確認・再試行できる。image、Release、Git tag、main、通常の作業 branch は削除しない。

同版の再実行で image が存在し ref がない場合、`verify` は採番 commit/tag と公開計画を検証し、完成済み Release の証拠と現在の**全** image の digest・参照先・`RELEASE_SHA` が完全一致した場合だけ `record` を再生成する。既存 ref が別 SHA、証拠欠落・不正・不一致、image 不在、registry/API 照会失敗は停止する。欠落 ref を未使用 image とみなしたり、現在の SHA を無条件に記録したり、既存 image を再 push しない。再実行が全工程に成功した後に同じ cleanup を行う。

### 既存の記録 branch を個別に整理する

既存 remote branch を一括削除しない。利用先の別作業で、更新済み共通 helper を使う保守用 Actions job をレビューしてから、次の順で**一版ずつ**実行する。保守 workflow も `release-main`・`cancel-in-progress: false`・`queue: max` を使い、通常公開と重ねない。

1. `git ls-remote --refs origin 'refs/heads/automation/docker-images/*'` は一覧確認だけに使う。対象版の exact ref/SHA、採番 trailer の Run/Tag、タグの指す commit、公開計画の全 image と対応する Release を確認する。通常の branch は対象外。
2. 元 run の最新 attempt が完了・成功し、公開と必要な Latest job が成功している版だけ選ぶ。実行中・queued・失敗・キャンセル・draft・部分公開・証拠不明の版は除外する。古い workflow による再実行は古い helper を使うため、削除後の再実行にも更新済み helper を使える保守経路を先に確保する。確保できない版は残す。
3. 保守 job は選択した採番 SHA を checkout する。レビュー済み更新 commit から新しい `release.py` と `manage-docker-image-owner.sh` を同じ別 directory（例 `$RUNNER_TEMP/owner-control`）へ取り出す。採番 commit の source・設定は編集しない。Rust 配布を持つ版は元 run の `rust-release-*` artifacts も取得し、`RUST_RELEASE_DIR` に配置する。manifest/配布物が失効・欠落して検証できない版は除外する。registry と GitHub の既存認証を使い、`contents: write` と `actions: read` を設定する。
4. checkout の root で次を実行する。`RELEASE_SHA` は確認した exact SHA、`IMAGE_OWNER_COMPLETED_RUN_ID` はその trailer の元 run ID に設定し、`GITHUB_RUN_ID` は保守 workflow 自身の値を保持する。`GITHUB_REF` は main のまま実行する。

   ```sh
   python3 -I "$RUNNER_TEMP/owner-control/release.py" image-owner-complete
   ```

5. helper は元 run の完了・成功を API で再確認し、公開状態・全 ref/SHA の検証、証拠の保存と読み返し後に対象 ref だけを削除する。元 run を検証できない版、削除前に所有元が変わった版は停止する。記録保存後の削除失敗は、原因確認後に同じ版を同じ直列化 job で再実行する。遠隔 ref と残った証拠を再確認し、失敗を成功扱いにしない。

更新前に誰かが記録 ref を消し、Release にも証拠がない image は自動採用しない。対応を別の信頼できる公開記録で調査し、復旧方針を決めるまで保持・停止する。

## 配布案内

Tauri の Windows 配布 ZIP には exe と `NotoSansJP-OFL.txt`、`NotoEmoji-OFL.txt`、`THIRD_PARTY_NOTICES.txt` を同梱し、ライセンスファイルが欠けていれば ZIP 作成を失敗させる。macOS は同じ3ファイルを app bundle の `Contents/Resources/licenses` 配下へ含め、原文との一致を ZIP 作成前に確認する。

NOTICE は全 production JavaScript 依存と pdfmake の内包コードを対象にする。offline 入力は原文の取得元 version と bundle/source map の SHA-256 を記録し、未公開の component version は推測しない。依存更新時は入力を確認し、`npm run notices:generate` の結果をレビューする。bundle 変更時は内包コードの inventory も再確認する。`npm run check` 内の `notices:check` は原文の欠落・不明な license・NOTICE の差分・bundle 変更で失敗する。

汎用 Release・Docker Release・Docker project pipeline は、添付済み asset と公開設定の image を本文の管理ブロックに案内する。既存のリリースノート・手書き本文は保持し、再実行では同じブロックを更新する。Docker Hub はリンクと実際の version tag の PULL コマンドを載せ、ECR は ECR の PULL コマンドを載せる。公開先の照会失敗・未提供 image は公開前に停止する。

Docker Release（Rust の有無にかかわらず）と Docker project pipeline は新規 Release を draft として作り、最終 image 照会と配布案内の本文更新が成功した時点で公開する。照会・本文更新が失敗した場合は draft のまま元 run を再実行する。

`use_rust=true` の汎用 Release・Docker Release は `rust_release_targets` で通常の Rust CLI / Exporter の配布対象を選ぶ。既定は既存の Linux x64（GNU）。root Cargo package の default features で有効な全 executable を同梱し、library のみ・無効な optional CLI のみの場合は配布 archive を作らない。Tauri の bundle 経路とは独立しており、Python のみの project や Docker project pipeline にこの配布 job は生成しない。

| Rust target                | Native runner                  | Archive                               |
| -------------------------- | ------------------------------ | ------------------------------------- |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` (x64)          | `<package>-<tag>-linux-x86_64.tar.gz` |
| `x86_64-pc-windows-msvc`   | `windows-latest` (x64)         | `<package>-<tag>-windows-x86_64.zip`  |
| `aarch64-pc-windows-msvc`  | `windows-11-arm` (ARM64)       | `<package>-<tag>-windows-arm64.zip`   |
| `aarch64-apple-darwin`     | `macos-latest` (Apple Silicon) | `<package>-<tag>-macos-arm64.tar.gz`  |

各 archive の隣に `<archive>.sha256` を添付する。形式は SHA-256 の小文字 hexadecimal、空白二つ、archive の実名、改行。既存 Linux の archive 名・default features・実行権限を維持する。Windows は `.exe`、macOS/Linux は executable を archive 直下へ置く。tag の `+build` は実名に保持し、`/` 等はファイル名で percent encode、upload URL ではさらに transport 用 encode を行う。Downloads は公開前に GitHub へ再照会した添付ファイルの実名と `browser_download_url` を載せ、予測 URL は作らない。

採番 commit にある `.github/release.json` の `publication.rust.targets` から matrix を読み、全 runner が同じ `release_sha` を checkout・採番記録と tag を検証する。既存設定で optional な `Cargo.lock` が無い binary project は、旧品質 gate と同様に `cargo generate-lockfile` で build 前に作る。存在する lock は保持し、library のみなら作らない。各 runner は `cargo build --release --locked --package <package> --bins --target <target>` の Cargo artifact から実行ファイルを集め、archive を別 directory へ展開し、各 executable を `smoke_args`（既定 `["--help"]`）で起動する。起動の timeout は各 30 秒。native OS / CPU と target が一致しなければ失敗する。ARM64 クロスビルド成功を実行確認として扱わない。依存機能の実動作や署名・notarization・一般利用者環境での起動は、この最小 smoke と別の製品検証である。

全 target の build・展開・起動成功後、共通公開 job が manifest の commit/tag/target/package/native smoke、archive 名・内容・checksum と target 間の executable 一致を検証する。不足・不一致では公開しない。全 archive と checksum の upload と照会、Docker の場合は image 照会、Downloads の本文更新が揃った時点で draft を公開する。失敗時には採番 commit/tag と draft を残す。元 run の再実行は完成済み asset を上書きせず、不足分と incomplete asset のみ継続する。再ビルドで圧縮 bytes が変わっても、checksum は保持した GitHub archive の digest（未提供なら実 bytes）から作る。既存の完成済み checksum が archive と違えば停止する。公開済み Release の不足 asset は補完せず停止する。

### 製品固有処理との境界

共通処理は target 選択・Cargo artifact 収集・命名・archive・checksum・最小 smoke・公開保護を所有する。OS の追加依存を導入する step は製品の生成済み `rust-builds` job に追記し、Copier update の three-way merge で保持・確認する。追加同梱物は製品が所有する script を `.github/release.json` の `publication.rust.stage_command`（既定 `[]`、shell を介さない argv）から呼ぶ。hook の作業 directory は project root、`RUST_RELEASE_STAGE_DIR` は executable を配置済みの一時 directory、`RUST_RELEASE_TARGET` / `RUST_RELEASE_TAG` は対象と tag。同梱する license・DLL・設定等を staging directory へコピーする。全 regular file を相対 path で同梱する。symlink は許可しない。hook の失敗、executable の削除・起動失敗は公開を止める。

```json
"rust": {
  "targets": ["x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc", "aarch64-apple-darwin"],
  "smoke_args": ["--help"],
  "stage_command": ["python", "scripts/package-release.py"]
}
```

製品固有の CLI 引数は `smoke_args` を変更する。GUI や外部サービスへの接続確認、独自 archive、既存 checksum の形式、依存・license 検証、既存の公開保護を持つ製品は、製品 Issue で接続方法と追加 smoke を検討する。共通 template に製品名や製品依存を持ち込まない。この共通化だけで下流製品の release は変更されない。

### Copier 更新

clean な専用 branch で上流を更新し、必要な対象を選択する。通常の three-way update を使い、回答だけを変更する場合は `--vcs-ref=:current:` にする。

```sh
copier update --trust --defaults --vcs-ref HEAD \
  -d 'rust_release_targets=["x86_64-unknown-linux-gnu","x86_64-pc-windows-msvc","aarch64-pc-windows-msvc","aarch64-apple-darwin"]'
```

`.copier-answers.yml`、`release.json`、通常または Docker Release、`release.py`、製品の依存導入 step・stage hook の差分と競合を確認する。旧 Linux 固定処理から更新すると checksum 添付と native build job が追加される。Linux 配布を維持する製品は Linux target を残す。生成・更新テスト、製品の品質 gate、選択した全 OS/CPU の archive 展開と起動を PR で検証し、未実機確認の範囲を記載する。認証・repository 設定・製品 release は更新 PR と別に扱う。

## 失敗と復旧

| 状態                               | 操作                                                                                                                                                                     |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 指定した分類の不正・設定・権限不足 | 示された PR ラベル・設定を修正して元 run を再実行                                                                                                                        |
| squash の検証失敗                  | 取り込み履歴・PR base・API を確認。複数 commit の rebase や merge commit の後は、新しい squash PR で必要な修正を取り込み、その最新 PR の分類で現在の main 全体を公開する |
| main 更新による競合                | 自動で再計算。三回とも競合したら元 run を再実行                                                                                                                          |
| push 応答が不明                    | remote の採番記録とタグを確認し、既に成功していれば同じ commit を再利用                                                                                                  |
| build・検証失敗                    | 原因を確認。元 run の再実行は同じ番号・ソース。製品修正は新 PR・新番号                                                                                                   |
| asset/image の部分公開             | 元 run を再実行し、同じ commit の不足分だけ継続                                                                                                                          |
| 公開済み状態の不整合               | 上書きせず停止。原因と実際の公開状態を確認                                                                                                                               |
| 古い run の再実行                  | 公開は復旧できるが、新しい完成済み Release の latest を巻き戻さない                                                                                                      |

採番 commit とタグは build 失敗時も残す。元 run の再実行は run ID に対応する commit を使い、新しい PR がマージされていても再採番しない。前回採番以後に新しいマージ済み PR がない通常 dispatch は新しい release を作らない。過去の公開を復旧するときは元 run を使う。

Copier update では最新構成へ一度に切り替え、旧採番・公開 workflow と旧 required check を整理する。二重公開を避け、既存 run の停止と実設定の変更は利用 repository の導入作業として行う。旧方式の run を新方式で復旧する互換処理は用意しない。
