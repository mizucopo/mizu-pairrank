# 言語ツールと製品設定の境界

共通の品質設定を変更するときは、下流の生成済みファイルを手でコピーせず、
repo-template 側を変更してから、専用ブランチで通常の `copier update` を実行する。
生成値と製品固有の値が同じ箇所で変わり競合した場合は、テストして解決する。
最初から上書きする `copier recopy` は通常の更新に使わない。

## Rust / Cargo

- Rust edition、`rust-toolchain.toml` の共通コンポーネント、
  fmt／Clippy（警告をエラー扱い）／test の品質検査は上流が管理する。
- ツールチェーンは Copier の `rust_version` で選ぶ。Cargo の `rust-version`（MSRV）、
  リリース済みバージョン、workspace、crate 名、依存・features、`Cargo.lock`、
  build script と配布時の追加処理は製品側で管理する。

## Node / npm

- Oxlint、Prettier、TypeScript、Vitest の共通設定、共通の開発ツールと
  `package.json` の品質コマンド（lint／format／typecheck／test／check）は上流が管理する。
  PR ポリシーは、生成プロファイルごとの `tsconfig.json` と
  `tsconfig.build.json` の型検査・コンパイル共通オプションをキー単位で検査する。
  共通の `lib`／`types` は保持しつつ製品で追加でき、`include`／`exclude`、
  入出力のパス、独自のオプションは製品側で管理する。
- 製品の package 名、版と識別子、ランタイム依存、固有 dev 依存、lockfile、
  bundler、build コマンド、アセットとテストのフィクスチャは製品側で管理する。
- 製品固有の追加チェックは npm の `precheck` または `postcheck` に置く。
  たとえば `"precheck": "npm run notices:check"` とすれば、`npm run check` の実行時に
  先に NOTICE の検証が走り、共通コマンドをコピーせずに済む。
  npm が用意する各コマンドの pre/post hook も同様に利用できる。
- 共通 dev 依存を変更する上流更新では、lockfile を機械的に置換しない。
  既存 lockfile と適合しない場合だけ、採用 PR 内で lockfile を意図的に更新する。
  製品向けの追加依存はそのまま保持する。

Copier の version 回答は初回生成値であり、製品の現在のリリースバージョン
ではない。更新時は版、lockfile、追加設定が保持されたことと、既存の
品質コマンドが引き続き通ることを確認する。
