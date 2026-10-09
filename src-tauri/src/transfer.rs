use std::collections::HashSet;
use std::io::{Cursor, Read, Write};
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
const MAX_IMAGES: usize = 500;
const MAX_TOTAL_IMAGE_PIXELS: u64 = 100_000_000;
const ARCHIVE_ENTRY: &str = "list.json";

// This is an allowlist, deliberately independent of the stored comparison model.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortableList {
    pub format: String,
    pub version: u32,
    pub name: String,
    #[serde(deserialize_with = "read_tags")]
    pub tags: Vec<String>,
    #[serde(deserialize_with = "read_items")]
    pub items: Vec<PortableItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortableItem {
    pub name: String,
    #[serde(deserialize_with = "read_tags")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

fn read_tags<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    read_bounded_sequence(deserializer, MAX_TAGS, MAX_TAGS, |_| 1)
}

fn read_items<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<PortableItem>, D::Error> {
    read_bounded_sequence(deserializer, MAX_ITEMS, MAX_ASSIGNMENTS, |item| {
        item.tags.len()
    })
}

// Apply collection limits while parsing, before untrusted arrays can grow in memory.
fn read_bounded_sequence<'de, D, T>(
    deserializer: D,
    maximum: usize,
    budget: usize,
    weight: fn(&T) -> usize,
) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct BoundedSequence<T> {
        maximum: usize,
        budget: usize,
        weight: fn(&T) -> usize,
    }
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for BoundedSequence<T> {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an array within the list collection limits")
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            mut self,
            mut seq: A,
        ) -> Result<Vec<T>, A::Error> {
            let mut values = Vec::new();
            while let Some(value) = seq.next_element()? {
                if values.len() == self.maximum {
                    return Err(serde::de::Error::custom("list collection limit exceeded"));
                }
                self.budget = self
                    .budget
                    .checked_sub((self.weight)(&value))
                    .ok_or_else(|| serde::de::Error::custom("list collection limit exceeded"))?;
                values.push(value);
            }
            Ok(values)
        }
    }
    deserializer.deserialize_seq(BoundedSequence {
        maximum,
        budget,
        weight,
    })
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
        validate_image_work(self)
    }
}

fn decode_image(encoded: &str) -> Result<Vec<u8>, String> {
    if encoded.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 {
        return Err("画像は1枚20MiBまで取り込めます。".to_owned());
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| "画像のBase64データが不正です。".to_owned())?;
    if bytes.len() > MAX_IMAGE_BYTES || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("画像は20MiB以下のPNGデータにしてください。".to_owned());
    }
    Ok(bytes)
}

fn validate_image_work(list: &PortableList) -> Result<(), String> {
    if list
        .items
        .iter()
        .filter(|item| item.image.is_some())
        .count()
        > MAX_IMAGES
    {
        return Err("画像は500枚まで含められます。画像なしでも共有できます。".to_owned());
    }
    let mut pixels = 0_u64;
    for encoded in list.items.iter().filter_map(|item| item.image.as_deref()) {
        let bytes = decode_image(encoded)?;
        // PNG requires IHDR first. Inspect dimensions without decoding pixels;
        // the normal image importer still checks the complete PNG and its CRC.
        if bytes.len() < 33 || &bytes[8..16] != b"\0\0\0\rIHDR" {
            return Err("PNGの画像サイズを読み取れませんでした。".to_owned());
        }
        let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        let image_pixels = u64::from(width) * u64::from(height);
        if width == 0
            || height == 0
            || width > 12_000
            || height > 12_000
            || image_pixels > crate::images::MAX_PIXELS
        {
            return Err("画像のサイズ・画素数が上限を超えています。".to_owned());
        }
        pixels = pixels
            .checked_add(image_pixels)
            .filter(|&sum| sum <= MAX_TOTAL_IMAGE_PIXELS)
            .ok_or_else(|| {
                "画像は合計1億画素まで含められます。画像なしでも共有できます。".to_owned()
            })?;
    }
    Ok(())
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
    if include_images
        && list
            .items
            .iter()
            .filter(|item| item.image.is_some())
            .count()
            > MAX_IMAGES
    {
        return Err(
            "画像は500枚まで含められます。画像を含めずにエクスポートしてください。".to_owned(),
        );
    }
    let mut portable = PortableList {
        format: "mizu-pairrank-list".to_owned(),
        version: 1,
        name: list.name,
        tags: list.tags.iter().map(|tag| tag.name.clone()).collect(),
        items: Vec::new(),
    };
    let mut image_bytes = 0;
    let mut image_pixels_left = MAX_TOTAL_IMAGE_PIXELS;
    for item in list.items {
        let image = if include_images {
            item.image
                .map(|image| {
                    let bytes = images
                        .ok_or_else(|| "画像の保存先を開けませんでした。".to_owned())?
                        .export_image(&image, &mut image_pixels_left)?;
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
    parse(&decode_archive(&bytes)?)
}

fn decode_archive(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let invalid = || "対応するZIP形式ではないか、ファイルが破損しています。".to_owned();
    if bytes.len() > MAX_FILE_BYTES || bytes.len() < 22 {
        return Err(invalid());
    }
    // Our single-file ZIP format needs neither ZIP64, split disks, nor comments.
    // Bound the central directory before the ZIP reader can allocate its entries.
    let end = &bytes[bytes.len() - 22..];
    if &bytes[..4] != b"PK\x03\x04"
        || &end[..4] != b"PK\x05\x06"
        || end[4..8] != [0; 4]
        || end[8..12] != [1, 0, 1, 0]
        || end[20..22] != [0; 2]
    {
        return Err(invalid());
    }
    let directory_size = u32::from_le_bytes(end[12..16].try_into().unwrap()) as usize;
    let directory_start = u32::from_le_bytes(end[16..20].try_into().unwrap()) as usize;
    if directory_size > 256 * 1024
        || directory_start.checked_add(directory_size) != Some(bytes.len() - 22)
    {
        return Err(invalid());
    }
    let directory = bytes
        .get(directory_start..bytes.len() - 22)
        .ok_or_else(invalid)?;
    if directory.len() < 46 || &directory[..4] != b"PK\x01\x02" {
        return Err(invalid());
    }
    let name_length = u16::from_le_bytes(directory[28..30].try_into().unwrap()) as usize;
    let extra_length = u16::from_le_bytes(directory[30..32].try_into().unwrap()) as usize;
    let comment_length = u16::from_le_bytes(directory[32..34].try_into().unwrap()) as usize;
    let mode = u32::from_le_bytes(directory[38..42].try_into().unwrap()) >> 16;
    if directory.len() != 46 + name_length + extra_length + comment_length
        || directory.get(46..46 + name_length) != Some(ARCHIVE_ENTRY.as_bytes())
        || directory[34..36] != [0; 2]
        || directory[42..46] != [0; 4]
        || comment_length != 0
        || directory[8..10] != bytes[6..8]
        || mode & 0o170000 == 0o120000
    {
        return Err(invalid());
    }
    // A streaming reader allocates only this entry, even if hostile metadata
    // advertises a huge number of entries in a nested or inconsistent footer.
    let checksum = u32::from_le_bytes(directory[16..20].try_into().unwrap());
    let compressed_size = u64::from(u32::from_le_bytes(directory[20..24].try_into().unwrap()));
    let expected_size = u64::from(u32::from_le_bytes(directory[24..28].try_into().unwrap()));
    if expected_size > MAX_FILE_BYTES as u64 {
        return Err("展開後も50MiB以下のリストファイルを選択してください。".to_owned());
    }
    let uses_descriptor = bytes[6] & 8 != 0;
    let options = if uses_descriptor {
        // Streaming writers leave these values out of the local header.
        zip::ZipReadOptions::default()
            .override_compressed_size(compressed_size)
            .override_uncompressed_size(expected_size)
            .override_crc(checksum)
    } else {
        zip::ZipReadOptions::default()
    };
    let mut reader = Cursor::new(&bytes[..directory_start]);
    let mut file = zip::read::read_zipfile_from_stream_with_options(&mut reader, options)
        .map_err(|_| invalid())?
        .ok_or_else(invalid)?;
    // Read in memory only; no archive path is ever extracted to the filesystem.
    if file.name_raw() != ARCHIVE_ENTRY.as_bytes()
        || !file.is_file()
        || file.is_symlink()
        || file.encrypted()
    {
        return Err(invalid());
    }
    let compression = match file.compression() {
        zip::CompressionMethod::Stored => 0,
        zip::CompressionMethod::Deflated => 8,
        _ => return Err(invalid()),
    };
    if file.size() != expected_size
        || file.compressed_size() != compressed_size
        || file.crc32() != checksum
        || u16::from_le_bytes(directory[10..12].try_into().unwrap()) != compression
    {
        return Err(invalid());
    }
    let mut json = Vec::new();
    (&mut file)
        .take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut json)
        .map_err(|_| invalid())?;
    if json.len() > MAX_FILE_BYTES || json.len() as u64 != expected_size {
        return Err("ZIPの展開サイズが不正、または50MiBを超えています。".to_owned());
    }
    drop(file);
    let tail = &bytes[reader.position() as usize..directory_start];
    if uses_descriptor {
        let descriptor = if tail.len() == 16 && tail.starts_with(b"PK\x07\x08") {
            &tail[4..]
        } else {
            tail
        };
        if descriptor != &directory[16..28] {
            return Err(invalid());
        }
    } else if !tail.is_empty() {
        return Err(invalid());
    }
    Ok(json)
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
    // Validate the whole image budget before decoding or saving the first image.
    validate_image_work(list)?;
    for item in &list.items {
        let image = item
            .image
            .as_ref()
            .map(|encoded| {
                let bytes = decode_image(encoded)?;
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
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("zip"))
    {
        return Err("保存先の拡張子は .zip にしてください。".to_owned());
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
        let mut archive = zip::ZipWriter::new(file.as_file_mut());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        archive.start_file(ARCHIVE_ENTRY, options)?;
        archive.write_all(bytes)?;
        archive.finish()?;
        if file.as_file().metadata()?.len() > MAX_FILE_BYTES as u64 {
            return Err(std::io::Error::other("ZIP file exceeds 50MiB"));
        }
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

    fn archive(entries: &[(&str, &[u8])], compression: zip::CompressionMethod) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            writer
                .start_file(
                    *name,
                    zip::write::SimpleFileOptions::default().compression_method(compression),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn directory_offset(bytes: &[u8]) -> usize {
        let end = bytes.len() - 22;
        u32::from_le_bytes(bytes[end + 16..end + 20].try_into().unwrap()) as usize
    }

    #[test]
    fn deserialization_stops_at_collection_limits_before_reading_the_rest() {
        for field in ["tags", "items"] {
            let (count, value) = if field == "tags" {
                (MAX_TAGS + 1, "\"tag\"")
            } else {
                (MAX_ITEMS + 1, "{\"name\":\"item\",\"tags\":[]}")
            };
            let mut json = format!(
                "{{\"format\":\"mizu-pairrank-list\",\"version\":1,\"name\":\"list\",\"{field}\":["
            );
            json.push_str(&vec![value; count].join(","));
            json.push_str(",malformed tail");
            let error = serde_json::from_str::<PortableList>(&json).unwrap_err();
            assert!(error.to_string().contains("collection limit"), "{error}");
        }
        let mut list = portable();
        list.items[0].tags = vec!["tag".to_owned(); MAX_TAGS + 1];
        let error = serde_json::from_slice::<PortableList>(&serde_json::to_vec(&list).unwrap())
            .map(|_| ())
            .expect_err("deserialization must stop at the collection limit");
        assert!(error.to_string().contains("collection limit"));
    }

    #[test]
    fn deserialization_bounds_total_tag_assignments_and_accepts_boundaries() {
        let mut list = portable();
        list.tags = (0..MAX_TAGS).map(|index| format!("tag{index}")).collect();
        list.items[0].tags = list.tags.clone();
        list.items = vec![list.items[0].clone(); MAX_ASSIGNMENTS / MAX_TAGS];
        let parsed = parse(&serde_json::to_vec(&list).unwrap()).unwrap();
        assert_eq!(parsed.items.len(), 50);
        list.items.push(list.items[0].clone());
        let error = serde_json::from_slice::<PortableList>(&serde_json::to_vec(&list).unwrap())
            .map(|_| ())
            .expect_err("deserialization must stop at the collection limit");
        assert!(error.to_string().contains("collection limit"));
        list.items[0].tags.clear();
        list.items = vec![list.items[0].clone(); MAX_ITEMS];
        assert!(parse(&serde_json::to_vec(&list).unwrap()).is_ok());
    }

    #[test]
    fn import_rejects_aggregate_image_work_before_decoding_or_saving() {
        // Only IHDR is needed for the preflight; no pixel decoding is permitted.
        let mut header = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        header.extend_from_slice(&10_000_u32.to_be_bytes());
        header.extend_from_slice(&3_000_u32.to_be_bytes());
        header.resize(33, 0);
        let mut list = portable();
        list.items[0].image = Some(STANDARD.encode(&header));
        list.items = vec![list.items[0].clone(); 5];
        let mut imported = Vec::new();
        assert!(
            import_images(&list, None, &mut imported)
                .unwrap_err()
                .contains("1億画素")
        );
        assert!(imported.is_empty());
        list.items[0].image = Some(STANDARD.encode(include_bytes!("../icons/32x32.png")));
        list.items = vec![list.items[0].clone(); 500];
        assert!(list.validate().is_ok());
        list.items = vec![list.items[0].clone(); 501];
        assert!(
            import_images(&list, None, &mut imported)
                .unwrap_err()
                .contains("500枚")
        );
        assert!(imported.is_empty());
    }

    #[test]
    fn image_budget_checks_header_dimensions_and_exact_pixel_boundary() {
        let mut header = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        header.extend_from_slice(&10_000_u32.to_be_bytes());
        header.extend_from_slice(&2_500_u32.to_be_bytes());
        header.resize(33, 0);
        let mut list = portable();
        list.items[0].image = Some(STANDARD.encode(&header));
        list.items = vec![list.items[0].clone(); 4];
        assert!(validate_image_work(&list).is_ok());
        list.items.push(list.items[0].clone());
        assert!(validate_image_work(&list).is_err());
        list.items.truncate(1);
        for (width, height) in [
            (0_u32, 1_u32),
            (12_001, 1),
            (6_000, 6_000),
            (u32::MAX, u32::MAX),
        ] {
            header[16..20].copy_from_slice(&width.to_be_bytes());
            header[20..24].copy_from_slice(&height.to_be_bytes());
            list.items[0].image = Some(STANDARD.encode(&header));
            assert!(validate_image_work(&list).is_err());
        }
        list.items[0].image = Some(STANDARD.encode(&header[..24]));
        assert!(validate_image_work(&list).is_err());
        header[8..12].copy_from_slice(&14_u32.to_be_bytes());
        list.items[0].image = Some(STANDARD.encode(&header));
        assert!(validate_image_work(&list).is_err());
    }

    #[test]
    fn export_rejects_too_many_images_before_loading_any_managed_file() {
        let mut db = Database::open(Path::new(":memory:")).unwrap();
        let list = db.create_list("多数の画像".to_owned()).unwrap();
        let mut list = db.add_items(list.id, vec!["項目".to_owned(); 501]).unwrap();
        for item in &mut list.items {
            item.image = Some(ImageAsset {
                path: "not-opened.png".to_owned(),
                source_url: None,
            });
        }
        assert!(encode(list, true, None).unwrap_err().contains("500枚"));
    }

    #[test]
    fn zip_accepts_streaming_data_descriptors_and_checks_their_integrity() {
        let json = serde_json::to_vec(&portable()).unwrap();
        for compression in [
            zip::CompressionMethod::Stored,
            zip::CompressionMethod::Deflated,
        ] {
            let mut writer = zip::ZipWriter::new_stream(Vec::new());
            writer
                .start_file(
                    ARCHIVE_ENTRY,
                    zip::write::SimpleFileOptions::default().compression_method(compression),
                )
                .unwrap();
            writer.write_all(&json).unwrap();
            let bytes = writer.finish().unwrap().into_inner();
            assert_ne!(bytes[6] & 8, 0);
            assert_eq!(decode_archive(&bytes).unwrap(), json);
            let directory = directory_offset(&bytes);
            assert_eq!(&bytes[directory - 16..directory - 12], b"PK\x07\x08");
            for offset in [directory - 12, directory - 8, directory - 4] {
                let mut corrupt = bytes.clone();
                corrupt[offset] ^= 1;
                assert!(decode_archive(&corrupt).is_err());
            }
            let mut unsigned = bytes;
            unsigned.drain(directory - 16..directory - 12);
            let end = unsigned.len() - 22;
            unsigned[end + 16..end + 20].copy_from_slice(&((directory - 4) as u32).to_le_bytes());
            assert_eq!(decode_archive(&unsigned).unwrap(), json);
        }
    }

    #[test]
    fn zip_roundtrip_supports_compressed_and_stored_single_json_entries() {
        let json = serde_json::to_vec(&portable()).unwrap();
        for compression in [
            zip::CompressionMethod::Deflated,
            zip::CompressionMethod::Stored,
        ] {
            let bytes = archive(&[(ARCHIVE_ENTRY, &json)], compression);
            assert_eq!(decode_archive(&bytes).unwrap(), json);
            assert_eq!(
                parse(&decode_archive(&bytes).unwrap()).unwrap().name,
                portable().name
            );
        }
    }

    #[test]
    fn zip_rejects_extra_entries_paths_links_and_unsupported_metadata() {
        for name in [
            "../list.json",
            "/list.json",
            "nested/list.json",
            "list.json/",
        ] {
            let bytes = archive(&[(name, b"{}")], zip::CompressionMethod::Stored);
            assert!(decode_archive(&bytes).is_err(), "{name}");
        }
        let mut extra = archive(
            &[(ARCHIVE_ENTRY, b"{}"), ("copy.json", b"{}")],
            zip::CompressionMethod::Stored,
        );
        assert!(decode_archive(&extra).is_err());
        // Hand-edit a duplicate name because the ZIP writer rejects duplicates.
        for offset in (0..extra.len())
            .filter(|&i| extra[i..].starts_with(b"copy.json"))
            .collect::<Vec<_>>()
        {
            extra[offset..offset + ARCHIVE_ENTRY.len()].copy_from_slice(ARCHIVE_ENTRY.as_bytes());
        }
        assert!(decode_archive(&extra).is_err());
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .add_symlink(
                ARCHIVE_ENTRY,
                "../../private",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        assert!(decode_archive(&writer.finish().unwrap().into_inner()).is_err());

        let bytes = archive(&[(ARCHIVE_ENTRY, b"{}")], zip::CompressionMethod::Stored);
        let directory = directory_offset(&bytes);
        for (offset, replacement) in [
            (6, vec![1, 0]),                    // encrypted local entry
            (8, vec![12, 0]),                   // unsupported compression
            (directory + 10, vec![8, 0]),       // central/local method mismatch
            (directory + 42, vec![1, 0, 0, 0]), // redirected local header
            (bytes.len() - 14, vec![255; 4]),   // huge/ZIP64 entry counts
            (bytes.len() - 18, vec![1, 0]),     // split archive
        ] {
            let mut changed = bytes.clone();
            changed[offset..offset + replacement.len()].copy_from_slice(&replacement);
            assert!(decode_archive(&changed).is_err(), "offset {offset}");
        }
        let mut commented = bytes.clone();
        let end = commented.len() - 22;
        commented[end + 20..end + 22].copy_from_slice(&1_u16.to_le_bytes());
        commented.push(b'x');
        assert!(decode_archive(&commented).is_err());
    }

    #[test]
    fn zip_rejects_corruption_and_bounds_declared_and_actual_expansion() {
        let bytes = archive(&[(ARCHIVE_ENTRY, b"{}")], zip::CompressionMethod::Stored);
        assert!(decode_archive(b"not a zip").is_err());
        assert!(decode_archive(&bytes[..bytes.len() - 1]).is_err());
        let mut corrupted = bytes.clone();
        corrupted[30 + ARCHIVE_ENTRY.len()] ^= 1;
        assert!(decode_archive(&corrupted).is_err()); // CRC mismatch
        let mut contradictory = bytes;
        let directory = directory_offset(&contradictory);
        contradictory[directory + 24..directory + 28].copy_from_slice(&3_u32.to_le_bytes());
        assert!(decode_archive(&contradictory).is_err());

        // Highly compressible input remains small on disk but exceeds the read cap.
        let mut oversized = archive(
            &[(ARCHIVE_ENTRY, &vec![b' '; MAX_FILE_BYTES + 1])],
            zip::CompressionMethod::Deflated,
        );
        assert!(oversized.len() < 1024 * 1024);
        assert!(decode_archive(&oversized).unwrap_err().contains("50MiB"));
        let directory = directory_offset(&oversized);
        // Both headers can lie about size; the bounded reader still refuses the data.
        oversized[22..26].copy_from_slice(&1_u32.to_le_bytes());
        oversized[directory + 24..directory + 28].copy_from_slice(&1_u32.to_le_bytes());
        assert!(decode_archive(&oversized).is_err());
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
        let path = root.path().join("list.zip");
        let bytes = serde_json::to_vec(&portable()).unwrap();
        write(&path, &bytes).unwrap();
        assert_eq!(read(&path).unwrap().name, portable().name);
        write(&path, b"replacement").unwrap();
        assert_eq!(
            decode_archive(&std::fs::read(&path).unwrap()).unwrap(),
            b"replacement"
        );
        let database = root.path().join("pairrank.sqlite3");
        std::fs::write(&database, b"existing").unwrap();
        assert!(write(&database, &bytes).is_err());
        assert_eq!(std::fs::read(&database).unwrap(), b"existing");
        assert!(write(&root.path().join("missing/list.zip"), &bytes).is_err());
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
        let source = root.path().join("source.zip");
        std::fs::write(&source, b"private source").unwrap();
        let link = root.path().join("link.zip");
        symlink(&source, &link).unwrap();
        assert!(read(&link).is_err());
        assert!(write(&link, b"public").is_err());
        assert_eq!(std::fs::read(source).unwrap(), b"private source");
        let fifo = root.path().join("fifo.zip");
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
