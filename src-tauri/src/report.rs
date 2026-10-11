use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

use base64::{Engine, engine::general_purpose::STANDARD};
use tauri::http::HeaderMap;
use tauri::ipc::InvokeBody;

use crate::database::Database;
use crate::images::ImageService;
use crate::models::ImageAsset;

const MAX_IMAGES: usize = 500;
const MAX_TOTAL_IMAGE_PIXELS: u64 = 100_000_000;
const DATA_URL_PREFIX: &str = "data:image/png;base64,";
const FORMAT_HEADER: &str = "x-pairrank-report-format";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    Html,
    Pdf,
}

pub fn read_request<'a>(
    body: &'a InvokeBody,
    headers: &HeaderMap,
) -> Result<(ReportFormat, &'a [u8]), String> {
    let InvokeBody::Raw(bytes) = body else {
        return Err("レポートはバイナリデータで送信してください。再試行してください。".to_owned());
    };
    let format = match headers
        .get(FORMAT_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        Some("html") => ReportFormat::Html,
        Some("pdf") => ReportFormat::Pdf,
        _ => return Err("レポート形式の指定が正しくありません。再試行してください。".to_owned()),
    };
    if headers.get_all(FORMAT_HEADER).iter().count() != 1 {
        return Err("レポート形式の指定が正しくありません。再試行してください。".to_owned());
    }
    validate_payload(format, bytes)?;
    Ok((format, bytes))
}

impl ReportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Pdf => "pdf",
        }
    }
}

pub fn selected_images(
    database: &Database,
    list_id: i64,
    expected_revision: i64,
    item_ids: &[i64],
) -> Result<Vec<(i64, ImageAsset)>, String> {
    if item_ids.len() > MAX_IMAGES {
        return Err("画像は500枚まで含められます。画像なしでも保存できます。".to_owned());
    }
    let list = database.get_list(list_id)?;
    if u64::try_from(expected_revision).ok() != Some(list.revision) {
        return Err(
            "リストが更新されています。ランキングを読み直して再試行してください。".to_owned(),
        );
    }
    let mut requested = HashSet::with_capacity(item_ids.len());
    for &id in item_ids {
        if !requested.insert(id) {
            return Err(
                "画像の項目が重複しています。ランキングを読み直して再試行してください。".to_owned(),
            );
        }
    }
    // Only database-owned references from this list can reach the image reader.
    let mut selected = Vec::with_capacity(requested.len());
    for item in list.items {
        if requested.remove(&item.id) {
            let image = item.image.ok_or_else(|| {
                "項目の画像が見つかりません。ランキングを読み直すか、画像なしで再試行してください。".to_owned()
            })?;
            selected.push((item.id, image));
        }
    }
    if !requested.is_empty() {
        return Err("項目が見つかりません。ランキングを読み直して再試行してください。".to_owned());
    }
    Ok(selected)
}

pub fn image_urls(
    selected: Vec<(i64, ImageAsset)>,
    images: &ImageService,
) -> Result<HashMap<i64, String>, String> {
    image_urls_with_limits(
        selected,
        images,
        MAX_TOTAL_IMAGE_PIXELS,
        crate::transfer::MAX_FILE_BYTES,
    )
}

fn image_urls_with_limits(
    selected: Vec<(i64, ImageAsset)>,
    images: &ImageService,
    mut pixels_left: u64,
    maximum_output_bytes: usize,
) -> Result<HashMap<i64, String>, String> {
    let mut output = HashMap::with_capacity(selected.len());
    let mut output_bytes = 2_usize;
    for (id, image) in selected {
        let bytes = images.export_image(&image, &mut pixels_left)?;
        // Data URLs need no JSON escaping; count keys and punctuation as well.
        let encoded_bytes = bytes.len().div_ceil(3) * 4
            + DATA_URL_PREFIX.len()
            + id.to_string().len()
            + 5
            + usize::from(!output.is_empty());
        output_bytes = output_bytes
            .checked_add(encoded_bytes)
            .filter(|&sum| sum <= maximum_output_bytes)
            .ok_or_else(|| "画像が50MiBを超えます。画像なしで再試行してください。".to_owned())?;
        output.insert(id, format!("{DATA_URL_PREFIX}{}", STANDARD.encode(bytes)));
    }
    Ok(output)
}

pub fn validate_payload(format: ReportFormat, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > crate::transfer::MAX_FILE_BYTES {
        return Err("レポートは50MiBまで保存できます。画像なしで再試行してください。".to_owned());
    }
    let valid = match format {
        ReportFormat::Pdf => bytes.starts_with(b"%PDF-"),
        ReportFormat::Html => std::str::from_utf8(bytes).is_ok_and(|html| {
            let html = html.trim_start_matches('\u{feff}').trim_start();
            html.get(..15)
                .is_some_and(|header| header.eq_ignore_ascii_case("<!doctype html>"))
                || html.get(..5).is_some_and(|header| {
                    header.eq_ignore_ascii_case("<html")
                        && html
                            .as_bytes()
                            .get(5)
                            .is_some_and(|&next| next == b'>' || next.is_ascii_whitespace())
                })
        }),
    };
    if !valid {
        return Err(
            "レポートの形式が正しくありません。ランキングを読み直して再試行してください。"
                .to_owned(),
        );
    }
    Ok(())
}

pub fn write(path: &Path, format: ReportFormat, bytes: &[u8]) -> Result<(), String> {
    validate_payload(format, bytes)?;
    if path
        .extension()
        .is_none_or(|extension| !extension.eq_ignore_ascii_case(format.extension()))
    {
        return Err(format!(
            "保存先の拡張子は .{} にしてください。",
            format.extension()
        ));
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
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let save = || -> std::io::Result<()> {
        // Keep the existing report until the complete raw document is durable.
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    };
    save().map_err(|_| {
        "レポートを保存できませんでした。保存先の権限と空き容量を確認してください。".to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ListState;
    use crate::rating::Preference;
    use std::io::Cursor;

    fn request_headers(format: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(FORMAT_HEADER, format.parse().unwrap());
        headers
    }

    #[test]
    fn report_requests_accept_only_raw_bytes_with_explicit_format_metadata() {
        for (format, document) in [
            (
                ReportFormat::Html,
                "<!doctype html><html>日本語</html>".as_bytes(),
            ),
            (
                ReportFormat::Pdf,
                b"%PDF-1.7\n\xff\0binary\n%%EOF\n".as_slice(),
            ),
        ] {
            let body = InvokeBody::Raw(document.to_vec());
            let InvokeBody::Raw(original) = &body else {
                unreachable!()
            };
            let (received_format, bytes) =
                read_request(&body, &request_headers(format.extension())).unwrap();
            assert_eq!(received_format, format);
            assert_eq!(bytes, document);
            assert_eq!(bytes.as_ptr(), original.as_ptr());
        }

        let mut headers = request_headers("pdf");
        headers.insert("content-type", "application/octet-stream".parse().unwrap());
        for body in [
            InvokeBody::Json(serde_json::json!({"format":"pdf","bytes":[37,80,68,70,45]})),
            InvokeBody::Json(serde_json::json!([37, 80, 68, 70, 45])),
        ] {
            assert!(
                read_request(&body, &headers)
                    .unwrap_err()
                    .contains("バイナリ")
            );
        }
    }

    #[test]
    fn report_requests_reject_missing_unsupported_and_ambiguous_format_headers() {
        let body = InvokeBody::Raw(b"%PDF-1.7\n%%EOF".to_vec());
        assert!(read_request(&body, &HeaderMap::new()).is_err());
        let mut content_type_only = HeaderMap::new();
        content_type_only.insert("content-type", "application/pdf".parse().unwrap());
        assert!(read_request(&body, &content_type_only).is_err());
        for format in ["", "zip", "PDF", "application/pdf", "pdf,html", " pdf"] {
            assert!(read_request(&body, &request_headers(format)).is_err());
        }
        let mut headers = request_headers("pdf");
        headers.append(FORMAT_HEADER, "html".parse().unwrap());
        assert!(read_request(&body, &headers).is_err());
        headers.insert(
            FORMAT_HEADER,
            tauri::http::HeaderValue::from_bytes(b"\xff").unwrap(),
        );
        assert!(read_request(&body, &headers).is_err());
    }

    #[test]
    fn report_requests_validate_utf8_and_document_magic_before_save() {
        for document in [
            b"<!doctype html>\xff".as_slice(),
            b"%PDF-1.7",
            b"PK ZIP archive",
            b"",
        ] {
            let body = InvokeBody::Raw(document.to_vec());
            assert!(read_request(&body, &request_headers("html")).is_err());
        }
        let html = InvokeBody::Raw(b"<!doctype html><html></html>".to_vec());
        assert!(read_request(&html, &request_headers("pdf")).is_err());
    }

    fn fixture() -> (tempfile::TempDir, Database, ImageService, ListState) {
        let directory = tempfile::tempdir().unwrap();
        let mut database = Database::open(&directory.path().join("report.sqlite3")).unwrap();
        let images = ImageService::open(&database).unwrap();
        let list = database.create_list("ランキング".into()).unwrap();
        let list = database
            .add_items(
                list.id,
                vec!["First".into(), "Second".into(), "No image".into()],
            )
            .unwrap();
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(3, 2)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        for item in &list.items[..2] {
            let mut asset = images.import_bytes(bytes.get_ref()).unwrap();
            asset.source_url = Some("https://private.test/source?token=secret".into());
            database.set_image(list.id, item.id, Some(asset)).unwrap();
        }
        let list = database.get_list(list.id).unwrap();
        (directory, database, images, list)
    }

    #[test]
    fn report_reads_only_requested_items_from_the_expected_list_revision() {
        let (_directory, mut database, images, list) = fixture();
        let first_id = list.items[0].id;
        let second_id = list.items[1].id;
        let no_image_id = list.items[2].id;
        let other = database.create_list("Other list".into()).unwrap();
        let other = database
            .add_items(other.id, vec!["Foreign".into()])
            .unwrap();
        let list = database
            .answer(
                list.id,
                first_id,
                second_id,
                Preference::AStrong,
                list.revision,
            )
            .unwrap();
        let revision = i64::try_from(list.revision).unwrap();
        let before = serde_json::to_value(&list).unwrap();
        let selected = selected_images(&database, list.id, revision, &[second_id]).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].0, second_id);
        let output = image_urls(selected, &images).unwrap();
        assert_eq!(output.len(), 1);
        let url = &output[&second_id];
        let bytes = STANDARD
            .decode(url.strip_prefix(DATA_URL_PREFIX).unwrap())
            .unwrap();
        let png = image::load_from_memory(&bytes).unwrap();
        assert_eq!((png.width(), png.height()), (3, 2));
        assert!(!url.contains("private.test"));
        assert!(!url.contains(".png"));

        for ids in [
            vec![other.items[0].id],
            vec![first_id, other.items[0].id],
            vec![no_image_id],
            vec![first_id, first_id],
            vec![i64::MAX],
        ] {
            assert!(selected_images(&database, list.id, revision, &ids).is_err());
        }
        assert!(selected_images(&database, list.id, revision - 1, &[first_id]).is_err());
        assert!(selected_images(&database, list.id, -1, &[]).is_err());
        assert!(selected_images(&database, i64::MAX, revision, &[]).is_err());
        assert_eq!(
            serde_json::to_value(database.get_list(list.id).unwrap()).unwrap(),
            before
        );
        // Export leaves the last answer's session-local undo receipt available.
        assert_eq!(
            database
                .undo_answer(list.id, list.revision)
                .unwrap()
                .comparison_count,
            0
        );
    }

    #[test]
    fn report_rejects_excess_image_count_before_loading_any_image() {
        let (_directory, database, _images, list) = fixture();
        let ids = vec![list.items[0].id; MAX_IMAGES + 1];
        assert!(
            selected_images(&database, list.id, list.revision as i64, &ids)
                .unwrap_err()
                .contains("500枚")
        );
    }

    #[test]
    fn report_images_share_source_pixel_and_encoded_output_limits() {
        let (_directory, database, images, list) = fixture();
        let ids = [list.items[0].id, list.items[1].id];
        let selected = || selected_images(&database, list.id, list.revision as i64, &ids).unwrap();
        let output = image_urls_with_limits(selected(), &images, 12, usize::MAX).unwrap();
        let length = serde_json::to_vec(&output).unwrap().len();
        assert!(
            image_urls_with_limits(selected(), &images, 11, usize::MAX)
                .unwrap_err()
                .contains("画素")
        );
        assert!(
            image_urls_with_limits(selected(), &images, 12, length - 1)
                .unwrap_err()
                .contains("50MiB")
        );
        assert_eq!(
            image_urls_with_limits(selected(), &images, 12, length).unwrap(),
            output
        );
    }

    #[test]
    fn report_refuses_unmanaged_paths_and_missing_files_without_reading_private_sources() {
        let (directory, mut database, images, list) = fixture();
        let item_id = list.items[0].id;
        let selected =
            selected_images(&database, list.id, list.revision as i64, &[item_id]).unwrap();
        std::fs::remove_file(directory.path().join("images").join(&selected[0].1.path)).unwrap();
        assert!(
            image_urls(selected, &images)
                .unwrap_err()
                .contains("再試行")
        );

        let source = directory.path().join("private.png");
        let private = b"private content outside managed image storage";
        std::fs::write(&source, private).unwrap();
        for path in [
            source.to_str().unwrap().to_owned(),
            "../private.png".to_owned(),
        ] {
            let list = database
                .set_image(
                    list.id,
                    item_id,
                    Some(ImageAsset {
                        path,
                        source_url: None,
                    }),
                )
                .unwrap()
                .value;
            let selected =
                selected_images(&database, list.id, list.revision as i64, &[item_id]).unwrap();
            assert!(image_urls(selected, &images).is_err());
            assert_eq!(std::fs::read(&source).unwrap(), private);
        }
    }

    #[test]
    fn reports_are_raw_documents_and_atomic_replacements_leave_no_temporary_files() {
        let directory = tempfile::tempdir().unwrap();
        let html = "<!doctype html><html lang=\"ja\"><body>ランキング</body></html>".as_bytes();
        let pdf = b"%PDF-1.7\n\xff\0binary document\n%%EOF\n";
        for (format, bytes) in [
            (ReportFormat::Html, html),
            (ReportFormat::Pdf, pdf.as_slice()),
        ] {
            let path = directory
                .path()
                .join(format!("ranking.{}", format.extension()));
            std::fs::write(&path, b"previous document").unwrap();
            write(&path, format, bytes).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert!(!std::fs::read(&path).unwrap().starts_with(b"PK"));
            assert!(write(&path, format, b"invalid document").is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        let files = std::fs::read_dir(directory.path()).unwrap().count();
        assert_eq!(files, 2);
        let wrong_extension = directory.path().join("ranking.zip");
        std::fs::write(&wrong_extension, b"existing ZIP").unwrap();
        assert!(write(&wrong_extension, ReportFormat::Pdf, pdf).is_err());
        assert_eq!(std::fs::read(&wrong_extension).unwrap(), b"existing ZIP");
        assert!(
            write(
                &directory.path().join("missing/ranking.pdf"),
                ReportFormat::Pdf,
                pdf
            )
            .is_err()
        );
        let subdirectory = directory.path().join("directory.pdf");
        std::fs::create_dir(&subdirectory).unwrap();
        assert!(write(&subdirectory, ReportFormat::Pdf, pdf).is_err());
        assert!(subdirectory.is_dir());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 4);
    }

    #[test]
    fn report_payloads_validate_format_encoding_and_size_before_save() {
        assert!(validate_payload(ReportFormat::Html, b" \n<!DOCTYPE HTML><html></html>").is_ok());
        assert!(validate_payload(ReportFormat::Html, b"<html lang=\"ja\"></html>").is_ok());
        for bytes in [
            b"".as_slice(),
            b"PK archive",
            b"<htmlx>text",
            b"<!doctype html>\xff",
        ] {
            assert!(validate_payload(ReportFormat::Html, bytes).is_err());
            assert!(validate_payload(ReportFormat::Pdf, bytes).is_err());
        }
        assert!(validate_payload(ReportFormat::Html, b"%PDF-1.7").is_err());
        assert!(validate_payload(ReportFormat::Pdf, b"<!doctype html>").is_err());
        let mut maximum = vec![b' '; crate::transfer::MAX_FILE_BYTES];
        maximum[..5].copy_from_slice(b"%PDF-");
        let mut body = InvokeBody::Raw(maximum);
        assert!(read_request(&body, &request_headers("pdf")).is_ok());
        let InvokeBody::Raw(maximum) = &mut body else {
            unreachable!()
        };
        maximum.push(b' ');
        assert!(
            read_request(&body, &request_headers("pdf"))
                .unwrap_err()
                .contains("50MiB")
        );
        let InvokeBody::Raw(maximum) = &body else {
            unreachable!()
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ranking.pdf");
        std::fs::write(&path, b"existing document").unwrap();
        assert!(write(&path, ReportFormat::Pdf, maximum).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"existing document");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn report_save_refuses_symlinks_and_preserves_their_targets() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("private.pdf");
        let link = directory.path().join("ranking.pdf");
        std::fs::write(&target, b"private document").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(write(&link, ReportFormat::Pdf, b"%PDF-1.7\n%%EOF").is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"private document");
        assert!(std::fs::symlink_metadata(link).unwrap().is_symlink());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }
}
