use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::Path;

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

use crate::images::ImageService;
use crate::models::{ImageAsset, ListState};

pub const MAX_FILE_BYTES: usize = 50 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const MAX_ITEMS: usize = 5_000;
const MAX_TAGS: usize = 1_000;
const MAX_ASSIGNMENTS: usize = 50_000;

// This is an allowlist, deliberately independent of the stored comparison model.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortableList {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub tags: Vec<String>,
    pub items: Vec<PortableItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortableItem {
    pub name: String,
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

impl PortableList {
    pub fn validate(&self) -> Result<(), String> {
        if self.format != "mizu-pairrank-list" || self.version != 1 {
            return Err("対応していないリスト形式・バージョンです。".to_owned());
        }
        validate_name(&self.name, 1_000)?;
        if self.items.len() > MAX_ITEMS || self.tags.len() > MAX_TAGS {
            return Err("リストは5,000項目・1,000タグまで取り込めます。".to_owned());
        }
        let mut tags = HashSet::new();
        for tag in &self.tags {
            validate_name(tag, 30)?;
            if !tags.insert(tag) {
                return Err("同じ名前のタグが重複しています。".to_owned());
            }
        }
        let mut assignments = 0;
        for item in &self.items {
            validate_name(&item.name, 1_000)?;
            assignments += item.tags.len();
            if assignments > MAX_ASSIGNMENTS {
                return Err("タグの割り当ては合計50,000件まで取り込めます。".to_owned());
            }
            let mut assigned = HashSet::new();
            for tag in &item.tags {
                if !tags.contains(tag) || !assigned.insert(tag) {
                    return Err("項目のタグが未定義、または重複しています。".to_owned());
                }
            }
            if let Some(image) = &item.image {
                // Bound decoding before allocating an image buffer.
                if image.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 {
                    return Err("画像は1枚20MiBまで取り込めます。".to_owned());
                }
            }
        }
        Ok(())
    }
}

fn validate_name(name: &str, maximum: usize) -> Result<(), String> {
    if name.trim().is_empty()
        || name.trim() != name
        || name.chars().count() > maximum
        || name.chars().any(char::is_control)
    {
        return Err(format!(
            "名前は空白のみ・制御文字を避け、{maximum}文字以内にしてください。"
        ));
    }
    Ok(())
}

pub fn encode(
    mut list: ListState,
    include_images: bool,
    images: Option<&ImageService>,
) -> Result<Vec<u8>, String> {
    // Ratings determine the application's display order; sharing uses registration order.
    list.items.sort_by_key(|item| item.id);
    let mut portable = PortableList {
        format: "mizu-pairrank-list".to_owned(),
        version: 1,
        name: list.name,
        tags: list.tags.iter().map(|tag| tag.name.clone()).collect(),
        items: Vec::new(),
    };
    let mut image_bytes = 0;
    for item in list.items {
        let image = if include_images {
            item.image
                .map(|image| {
                    let bytes = images
                        .ok_or_else(|| "画像の保存先を開けませんでした。".to_owned())?
                        .export_image(&image)?;
                    image_bytes += bytes.len().div_ceil(3) * 4;
                    if image_bytes > MAX_FILE_BYTES {
                        return Err(
                            "50MiBを超えます。画像を含めずにエクスポートしてください。".to_owned()
                        );
                    }
                    Ok(STANDARD.encode(bytes))
                })
                .transpose()?
        } else {
            None
        };
        portable.items.push(PortableItem {
            name: item.name,
            tags: list
                .tags
                .iter()
                .filter(|tag| item.tag_ids.contains(&tag.id))
                .map(|tag| tag.name.clone())
                .collect(),
            image,
        });
    }
    portable.validate()?;
    let bytes = serde_json::to_vec_pretty(&portable)
        .map_err(|_| "リストを書き出せませんでした。".to_owned())?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err("50MiBを超えます。画像を含めずにエクスポートしてください。".to_owned());
    }
    Ok(bytes)
}

pub fn read(path: &Path) -> Result<PortableList, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOFOLLOW).bits() as i32,
        );
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| "選択したリストファイルを確認できませんでした。".to_owned())?;
    if !metadata.is_file() {
        return Err("通常のリストファイルを選択してください。".to_owned());
    }
    let file = options
        .open(path)
        .map_err(|_| "選択したリストファイルを開けませんでした。".to_owned())?;
    let metadata = file
        .metadata()
        .map_err(|_| "選択したリストファイルを確認できませんでした。".to_owned())?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES as u64 {
        return Err("50MiB以下のリストファイルを選択してください。".to_owned());
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "選択したリストファイルを読み取れませんでした。".to_owned())?;
    parse(&bytes)
}

fn parse(bytes: &[u8]) -> Result<PortableList, String> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err("50MiB以下のリストファイルを選択してください。".to_owned());
    }
    let list: PortableList = serde_json::from_slice(bytes)
        .map_err(|_| "リスト形式が不正、またはファイルが破損しています。".to_owned())?;
    list.validate()?;
    Ok(list)
}

pub fn import_images(
    list: &PortableList,
    service: Option<&ImageService>,
    imported: &mut Vec<Option<ImageAsset>>,
) -> Result<(), String> {
    for item in &list.items {
        let image = item
            .image
            .as_ref()
            .map(|encoded| {
                let bytes = STANDARD
                    .decode(encoded)
                    .map_err(|_| "画像のBase64データが不正です。".to_owned())?;
                if bytes.len() > MAX_IMAGE_BYTES || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                    return Err("画像は20MiB以下のPNGデータにしてください。".to_owned());
                }
                service
                    .ok_or_else(|| "画像の保存先を開けませんでした。".to_owned())?
                    .import_bytes(&bytes)
            })
            .transpose()?;
        imported.push(image);
    }
    Ok(())
}

pub fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path
        .extension()
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("json"))
    {
        return Err("保存先の拡張子は .json にしてください。".to_owned());
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            return Err("通常のファイルを保存先に選択してください。".to_owned());
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err("保存先を確認できませんでした。".to_owned());
        }
        _ => {}
    }
    // Write completely before replacing the chosen destination, so failures preserve it.
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let save = || -> std::io::Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(bytes)?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    };
    save().map_err(|_| {
        "リストを保存できませんでした。保存先の権限と空き容量を確認してください。".to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::rating::Preference;

    fn portable() -> PortableList {
        PortableList {
            format: "mizu-pairrank-list".to_owned(),
            version: 1,
            name: "好きな果物".to_owned(),
            tags: vec!["季節".to_owned()],
            items: vec![PortableItem {
                name: "りんご".to_owned(),
                tags: vec!["季節".to_owned()],
                image: None,
            }],
        }
    }

    #[test]
    fn export_is_an_allowlist_in_registration_order_even_after_comparing() {
        let mut db = Database::open(Path::new(":memory:")).unwrap();
        let list = db.create_list("果物".to_owned()).unwrap();
        let list = db
            .add_items(
                list.id,
                vec!["りんご".to_owned(), "みかん".to_owned(), "削除".to_owned()],
            )
            .unwrap();
        let a = list.items[0].id;
        let b = list.items[1].id;
        db.delete_item(list.id, list.items[2].id).unwrap();
        let list = db.get_list(list.id).unwrap();
        db.answer(list.id, a, b, Preference::BStrong, list.revision)
            .unwrap();
        let asset = ImageAsset {
            path: "/private/secret/image.png".to_owned(),
            source_url: Some("https://private.test/token".to_owned()),
        };
        db.set_image(list.id, a, Some(asset)).unwrap();
        let list = db.create_tag(list.id, "季節".to_owned(), Some(a)).unwrap();
        assert_eq!(list.items[0].id, b);
        let bytes = encode(list, false, None).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "format":"mizu-pairrank-list","version":1,"name":"果物","tags":["季節"],
                "items":[{"name":"りんご","tags":["季節"]},{"name":"みかん","tags":[]}]
            })
        );
        assert_eq!(parse(&bytes).unwrap().items.len(), 2);
    }

    #[test]
    fn malformed_unknown_private_and_path_fields_are_rejected() {
        assert!(parse(b"not json").is_err());
        assert!(parse(b"{\"version\":1").is_err());
        for field in [
            "rating",
            "comparisons",
            "path",
            "sourceUrl",
            "apiKey",
            "__proto__",
        ] {
            let mut value = serde_json::to_value(portable()).unwrap();
            value[field] = serde_json::json!("../../secret");
            assert!(
                parse(&serde_json::to_vec(&value).unwrap()).is_err(),
                "{field}"
            );
            let mut value = serde_json::to_value(portable()).unwrap();
            value["items"][0][field] = serde_json::json!("../../secret");
            assert!(
                parse(&serde_json::to_vec(&value).unwrap()).is_err(),
                "{field}"
            );
        }
        let mut list = portable();
        list.version = 2;
        assert!(list.validate().is_err());
        list.version = 1;
        list.format = "other".to_owned();
        assert!(list.validate().is_err());
    }

    #[test]
    fn structural_limits_and_tag_relationships_are_checked() {
        for name in ["", " ", "\0evil", "a\nb", " padded "] {
            let mut list = portable();
            list.name = name.to_owned();
            assert!(list.validate().is_err());
        }
        let mut list = portable();
        list.items[0].name = "あ".repeat(1_001);
        assert!(list.validate().is_err());
        list = portable();
        list.tags.push("季節".to_owned());
        assert!(list.validate().is_err());
        list = portable();
        list.items[0].tags.push("未定義".to_owned());
        assert!(list.validate().is_err());
        list = portable();
        list.items[0].tags.push("季節".to_owned());
        assert!(list.validate().is_err());
        list = portable();
        list.items = vec![list.items[0].clone(); MAX_ITEMS + 1];
        assert!(list.validate().is_err());
        list = portable();
        list.tags = (0..=MAX_TAGS).map(|i| i.to_string()).collect();
        assert!(list.validate().is_err());
        list = portable();
        list.tags = (0..100).map(|i| i.to_string()).collect();
        list.items = vec![
            PortableItem {
                name: "項目".to_owned(),
                tags: list.tags.clone(),
                image: None
            };
            501
        ];
        assert!(list.validate().is_err());
    }

    #[test]
    fn files_are_bounded_and_failed_saves_preserve_existing_content() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("list.json");
        let bytes = serde_json::to_vec(&portable()).unwrap();
        write(&path, &bytes).unwrap();
        assert_eq!(read(&path).unwrap().name, portable().name);
        write(&path, b"replacement").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
        let database = root.path().join("pairrank.sqlite3");
        std::fs::write(&database, b"existing").unwrap();
        assert!(write(&database, &bytes).is_err());
        assert_eq!(std::fs::read(&database).unwrap(), b"existing");
        assert!(write(&root.path().join("missing/list.json"), &bytes).is_err());
        assert!(read(root.path()).is_err());
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_FILE_BYTES as u64 + 1).unwrap();
        assert!(read(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_fifos_are_rejected_without_touching_their_targets() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.json");
        std::fs::write(&source, b"private source").unwrap();
        let link = root.path().join("link.json");
        symlink(&source, &link).unwrap();
        assert!(read(&link).is_err());
        assert!(write(&link, b"public").is_err());
        assert_eq!(std::fs::read(source).unwrap(), b"private source");
        let fifo = root.path().join("fifo.json");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(read(&fifo).is_err());
        assert!(write(&fifo, b"public").is_err());
    }

    #[test]
    fn image_input_rejects_paths_base64_and_non_png_data() {
        let mut list = portable();
        for image in ["../../private", "https://example.test/a.png", "bad!!!"] {
            list.items[0].image = Some(image.to_owned());
            assert!(import_images(&list, None, &mut Vec::new()).is_err());
        }
        list.items[0].image = Some(STANDARD.encode(b"GIF89a"));
        assert!(import_images(&list, None, &mut Vec::new()).is_err());
        list.items[0].image = Some("a".repeat(MAX_IMAGE_BYTES.div_ceil(3) * 4 + 1));
        assert!(list.validate().is_err());
    }
}
