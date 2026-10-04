# 採番・公開

## 設定

- squash merge を使い、通常 PR・Dependabot とも実装者が `release:patch/minor/major` を一つ付け、PR 本文に分類理由を書く。欠落・競合・未対応値は公開を停止する。docs/tests だけの変更も patch として分類する。
- 標準 GITHUB_TOKEN に contents:write と pull-requests:read を許す。main とタグへの直接 push を許す repository 設定を使う。force push は不要。PR 必須等の設定で拒否される場合は設定を確認し、workflow 側で保護を回避しない。
- Docker Hub は DOCKERHUB_TOKEN、ECR は AWS_ROLE_ARN と OIDC の権限を設定する。採番時にも公開先を照会するため、image の照会権限が必要。
- version source、lockfile、公開タグ・画像は .github/release.json の version と publication に宣言する。専用 App、署名鍵、CI 登録、初回移行 helper は不要。

## 通常の流れ

PR 品質 CI → squash merge → 最新 main のまとめ採番 → 採番 commit とタグの atomic push → 同じ Actions run 内でビルド・検証・公開。

公開 workflow 全体は release-main の concurrency group に入り、queue:max、cancel-in-progress:false で直列化する。実行定義・script・設定が待機中に変わった場合は、その変更の新しい run に任せる。開始時点の最新 main から、前回採番以後の PR をまとめる。分類は major > minor > patch の最大。run の待機順を commit 順の代用にしない。

初回は新公開設定を導入した commit の親以後を対象とする。新規 repository の root commit は設定した初期 version を最初の候補にする。設定導入前の過去 PR を再分類しない。main への通常変更は squash-merged PR で行う。

番号規則は SemVer、Chrome manifest version、upstream-revision を使う。SemVer 0.x の major は 1.0.0。prerelease patch は末尾数値を増やし、channel・core・安定版への変更は明示 version を必要とする。明示 version は分類に必要な更新幅以上の下限指定。Chrome の各成分は 0～65535で、上限到達時は停止する。upstream-revision は上流 version を保持して revision を進め、新しい上流 version では r0 から始める。

Git tag、draft を含む Release、設定した image tag の未使用を確認する。衝突は次の番号へ進むが、認証・通信・照会の失敗を未作成と扱わない。latest は衝突対象にしない。version/manifest と既存 lockfile の version 項目だけを更新する。

採番 commit は元の main を親とし、番号、タグ、対象 PR、元の SHA、run ID を trailer に記録する。main とタグを一度に push する。競合したら最新 main から最大三回まで再計算する。保護設定や認証の拒否では停止する。

後続 job は採番 commit の SHA を checkout する。GITHUB_SHA は元イベントの SHA のままなので、公開対象には使わない。タグ作成による別 workflow の起動を待たず、同じ run の後続 job で完結させる。

全検証・配布物が完成してから Release を公開する。GitHub Latest・Docker latest は最新の完成済み Release にだけ更新する。Tauri の prerelease は Latest と Tap 通知の対象外。

## 失敗と復旧

| 状態                   | 操作                                                                    |
| ---------------------- | ----------------------------------------------------------------------- |
| 分類・設定・権限不足   | 示された PR ラベル・設定を修正して元 run を再実行                       |
| main 更新による競合    | 自動で再計算。三回とも競合したら元 run を再実行                         |
| push 応答が不明        | remote の採番記録とタグを確認し、既に成功していれば同じ commit を再利用 |
| build・検証失敗        | 原因を確認。元 run の再実行は同じ番号・ソース。製品修正は新 PR・新番号  |
| asset/image の部分公開 | 元 run を再実行し、同じ commit の不足分だけ継続                         |
| 公開済み状態の不整合   | 上書きせず停止。原因と実際の公開状態を確認                              |
| 古い run の再実行      | 公開は復旧できるが、新しい完成済み Release の latest を巻き戻さない     |

採番 commit とタグは build 失敗時も残す。元 run の再実行は run ID に対応する commit を使い、再採番しない。新しい変更がない通常 dispatch は新しい release を作らない。過去の公開を復旧するときは元 run を使う。

Copier update では最新構成へ一度に切り替え、旧採番・公開 workflow と旧 required check を整理する。二重公開を避け、既存 run の停止と実設定の変更は利用 repository の導入作業として行う。旧方式の run を新方式で復旧する互換処理は用意しない。
