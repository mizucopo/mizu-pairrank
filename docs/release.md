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

番号規則は SemVer、Chrome manifest version、upstream-revision を使う。SemVer 0.x の major は 1.0.0。prerelease patch は末尾数値を増やし、channel・core・安定版への変更は明示 version を必要とする。明示 version は分類に必要な更新幅以上の下限指定。Chrome の各成分は 0～65535で、上限到達時は停止する。upstream-revision は上流 version を保持して revision を進め、新しい上流 version では r0 から始める。

SemVer の core を増やす場合、その core の prerelease も明示できる。例えば 0.2.3 から patch は 0.2.4-rc.1、minor は 0.3.0-rc.1、major は 1.0.0-rc.1 を開始できる。既存 prerelease の patch は次の番号以上を必要とし、同じ番号や前の番号への巻き戻しは拒否する。

Git tag、draft を含む Release、設定した image tag の未使用を確認する。衝突は次の番号へ進むが、認証・通信・照会の失敗を未作成と扱わない。latest は衝突対象にしない。version/manifest と既存 lockfile の version 項目だけを更新する。

採番 commit は元の main を親とし、番号、タグ、分類対象の PR 一つ、元の SHA、run ID を trailer に記録する。main とタグを一度に push する。競合したら最新 main のマージ済み PR を選び直し、最大三回まで再計算する。保護設定や認証の拒否では停止する。

後続 job は採番 commit の SHA を checkout する。GITHUB_SHA は元イベントの SHA のままなので、公開対象には使わない。タグ作成による別 workflow の起動を待たず、同じ run の後続 job で完結させる。

全検証・配布物が完成してから Release を公開する。GitHub Latest・Docker latest は最新の完成済み安定版 Release にだけ更新する。汎用 GitHub Release、Docker、Docker project、Tauri の prerelease は Latest の照会・昇格対象外で、初回公開でも Latest を変更しない。過去に通常 Release として公開された prerelease も採番記録から除外する。Tauri の prerelease は Tap 通知の対象外。

Release の分類は共通公開計画で採番済み VERSION と方式から決める。build metadata は分類に影響しない。通常 SemVer の 1.2.3-r1 は prerelease。upstream-revision の上流 VERSION が 1.2.3 なら公開 tag 1.2.3-r1 は通常 Release・Latest 候補で、上流 VERSION が 1.2.3-rc.1 なら prerelease。公開 tag の revision suffix だけで分類しない。

Docker image tag は展開後の `+` を `_` に変換する（例 `1.2.3+build.1` → `1.2.3_build.1`）。SemVer に `_` は使えないため prerelease の `-` と衝突せず、Git version・tag・Release 名は元の値を保持する。有効な既存 image tag は変わらない。変換後にも tag の長さ・形式・使用済み状態・所有者を検証する。

## 配布案内

汎用 Release・Docker Release・Docker project pipeline は、添付済み asset と公開設定の image を本文の管理ブロックに案内する。既存のリリースノート・手書き本文は保持し、再実行では同じブロックを更新する。Docker Hub はリンクと実際の version tag の PULL コマンドを載せ、ECR は ECR の PULL コマンドを載せる。公開先の照会失敗・未提供 image は公開前に停止する。

`use_rust=true` の汎用 Release・Docker Release は、root Cargo package のdefault features の実行バイナリを Linux x64（GNU）向けにビルドし、`<package>-<tag>-linux-x86_64.tar.gz` を添付してから draft を公開する。library のみなら実行バイナリは添付しない。他の OS・architecture は配布しない。Python project 等に Rust バイナリの案内は生成しない。

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
