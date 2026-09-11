# 管理画像は保存先から独立した参照で扱う

新規画像の `ImageAsset.path` はASCIIのUUIDファイル名とし、ネイティブの保存先 `PathBuf` とRust側で結合する。Unixの保存先に非UTF-8名が含まれても画像の表示と削除を維持するため、UTF-8へ変換するTauri標準asset protocolに代わり、管理画像だけを返す `pairrank-image` protocolを新規参照に使う。

既存DBの絶対UTF-8パスは書き換えず、従来のasset protocolによる表示と管理ディレクトリ内に限定した削除を維持する。DBの列・スキーマバージョンは変更しない。
