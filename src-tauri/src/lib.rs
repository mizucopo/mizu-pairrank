mod credentials;
mod database;
mod images;
mod models;
mod rating;
mod storage;

use database::{Database, ImageChange};
use images::{ImageCandidate, ImageService, SearchProvider, SearchSettings};
use models::{ImageAsset, ListState, ListSummary};
use rating::Preference;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

struct Backend {
    database: Arc<Mutex<Result<Database, String>>>,
    images: Result<Arc<ImageService>, String>,
}

impl Backend {
    fn open_with(
        directory: Result<std::path::PathBuf, String>,
        mut checkpoint: impl FnMut(u8),
    ) -> Self {
        let directory = directory.and_then(|path| {
            storage::PreparedAppData::open(path)
                .map_err(|error| format!("保存先を作成できません: {error}"))
        });
        let verify = |directory: &storage::PreparedAppData| {
            directory.verify().map_err(|_| {
                "保存先が起動中に変更されました。アプリを再起動してください。".to_owned()
            })
        };
        checkpoint(0);
        let mut database = directory
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|directory| {
                verify(directory)?;
                let database = Database::open(&directory.path().join("pairrank.sqlite3"))?;
                verify(directory)?;
                Ok(database)
            });
        checkpoint(1);
        let mut images = directory
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|directory| {
                verify(directory)?;
                let database = database.as_ref().map_err(Clone::clone)?;
                let images = ImageService::open(directory.path().to_owned(), database)?;
                verify(directory)?;
                Ok(images)
            })
            .map(Arc::new);
        checkpoint(2);
        if let Ok(directory) = &directory
            && let Err(error) = verify(directory)
        {
            database = Err(error.clone());
            images = Err(error);
        }
        Self {
            database: Arc::new(Mutex::new(database)),
            images,
        }
    }

    async fn database_job<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Database) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let database = self.database.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let mut guard = database.lock().map_err(|_| {
                "データ操作を続けられません。アプリを再起動してください。".to_string()
            })?;
            let database = guard.as_mut().map_err(|error| error.clone())?;
            operation(database)
        })
        .await
        .map_err(|error| format!("データ処理に失敗しました: {error}"))?
    }

    async fn change_images<T: Send + 'static>(
        &self,
        image: Option<ImageAsset>,
        operation: impl FnOnce(&mut Database, Option<ImageAsset>) -> Result<ImageChange<T>, String>
        + Send
        + 'static,
    ) -> Result<T, String> {
        let images = self.images.clone();
        let imported = image.clone();
        let result = self
            .database_job(move |db| {
                if let Some(image) = &image {
                    images
                        .as_ref()
                        .map_err(Clone::clone)?
                        .validate_import(image)?;
                }
                let change = operation(db, image)?;
                if let Ok(images) = &images {
                    remove_unused_images(db, images, change.cleanup_paths);
                }
                Ok(change.value)
            })
            .await;
        if let (Err(_), Some(image), Ok(images)) = (&result, imported, &self.images) {
            let database = self.database.clone();
            let images = images.clone();
            // Inspect even a poisoned lock for cleanup, without allowing another mutation.
            let _ = tauri::async_runtime::spawn_blocking(move || {
                let guard = database
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Ok(db) = &*guard {
                    remove_unused_images(db, &images, [image.path]);
                }
            })
            .await;
        }
        result
    }
}

fn remove_unused_images(
    db: &Database,
    service: &ImageService,
    paths: impl IntoIterator<Item = String>,
) {
    for path in paths {
        if db.image_in_use(&path) == Ok(false) {
            service.remove_managed_file(&path);
        }
    }
}

async fn database_job<T: Send + 'static>(
    app: AppHandle,
    operation: impl FnOnce(&mut Database) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    app.state::<Backend>().database_job(operation).await
}

fn image_service(app: &AppHandle) -> Result<Arc<ImageService>, String> {
    app.state::<Backend>().images.clone()
}

#[tauri::command]
async fn list_summaries(app: AppHandle) -> Result<Vec<ListSummary>, String> {
    database_job(app, |db| db.list_summaries()).await
}
#[tauri::command]
async fn get_list(app: AppHandle, list_id: i64) -> Result<ListState, String> {
    database_job(app, move |db| db.get_list(list_id)).await
}
#[tauri::command]
async fn create_list(app: AppHandle, name: String) -> Result<ListState, String> {
    database_job(app, move |db| db.create_list(name)).await
}
#[tauri::command]
async fn rename_list(app: AppHandle, list_id: i64, name: String) -> Result<ListState, String> {
    database_job(app, move |db| db.rename_list(list_id, name)).await
}
#[tauri::command]
async fn delete_list(app: AppHandle, list_id: i64) -> Result<(), String> {
    app.state::<Backend>()
        .change_images(None, move |db, _| db.delete_list(list_id))
        .await
}
#[tauri::command]
async fn add_items(app: AppHandle, list_id: i64, names: Vec<String>) -> Result<ListState, String> {
    database_job(app, move |db| db.add_items(list_id, names)).await
}
#[tauri::command]
async fn rename_item(
    app: AppHandle,
    list_id: i64,
    item_id: i64,
    name: String,
) -> Result<ListState, String> {
    database_job(app, move |db| db.rename_item(list_id, item_id, name)).await
}
#[tauri::command]
async fn delete_item(app: AppHandle, list_id: i64, item_id: i64) -> Result<ListState, String> {
    app.state::<Backend>()
        .change_images(None, move |db, _| db.delete_item(list_id, item_id))
        .await
}
#[tauri::command]
async fn resume_list(app: AppHandle, list_id: i64) -> Result<ListState, String> {
    database_job(app, move |db| db.resume_list(list_id)).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PairProposal {
    list_id: i64,
    revision: u64,
    a: models::Item,
    b: models::Item,
}

#[tauri::command]
async fn next_pair(app: AppHandle, list_id: i64) -> Result<Option<PairProposal>, String> {
    database_job(app, move |db| {
        let comparison = db.comparison_state(list_id)?;
        let list = comparison.list;
        let ratings = list
            .items
            .iter()
            .map(|item| (item.id, item.rating))
            .collect::<Vec<_>>();
        let Some((mut a_id, mut b_id)) = rating::select_pair(&ratings, &comparison.pair_counts)?
        else {
            return Ok(None);
        };
        if rand::random::<bool>() {
            std::mem::swap(&mut a_id, &mut b_id);
        }
        let find = |id| {
            list.items
                .iter()
                .find(|item| item.id == id)
                .cloned()
                .ok_or_else(|| "比較項目が見つかりません。".to_string())
        };
        Ok(Some(PairProposal {
            list_id,
            revision: list.revision,
            a: find(a_id)?,
            b: find(b_id)?,
        }))
    })
    .await
}
#[tauri::command]
async fn answer(
    app: AppHandle,
    list_id: i64,
    a_id: i64,
    b_id: i64,
    preference: Preference,
    expected_revision: u64,
) -> Result<ListState, String> {
    database_job(app, move |db| {
        db.answer(list_id, a_id, b_id, preference, expected_revision)
    })
    .await
}
#[tauri::command]
async fn search_settings(app: AppHandle) -> Result<SearchSettings, String> {
    image_service(&app)?.settings().await
}
#[tauri::command]
async fn set_api_key(app: AppHandle, provider: SearchProvider, key: String) -> Result<(), String> {
    let service = image_service(&app)?;
    tauri::async_runtime::spawn_blocking(move || service.set_api_key(provider, key))
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn search_images(
    app: AppHandle,
    provider: SearchProvider,
    query: String,
) -> Result<Vec<ImageCandidate>, String> {
    image_service(&app)?.search(provider, query).await
}
#[tauri::command]
async fn set_local_image(
    app: AppHandle,
    list_id: i64,
    item_id: i64,
) -> Result<Option<ListState>, String> {
    let service = image_service(&app)?;
    let dialog_app = app.clone();
    let image = tauri::async_runtime::spawn_blocking(move || {
        let Some(file) = dialog_app
            .dialog()
            .file()
            .add_filter("画像", &["png", "jpg", "jpeg", "webp"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = file.into_path().map_err(|error| error.to_string())?;
        service.import_local(path).map(Some)
    })
    .await
    .map_err(|error| error.to_string())??;
    let Some(image) = image else {
        return Ok(None);
    };
    app.state::<Backend>()
        .change_images(Some(image), move |db, image| {
            db.set_image(list_id, item_id, image)
        })
        .await
        .map(Some)
}
#[tauri::command]
async fn set_remote_image(
    app: AppHandle,
    list_id: i64,
    item_id: i64,
    candidate: ImageCandidate,
) -> Result<ListState, String> {
    let image = image_service(&app)?.import_remote(candidate).await?;
    app.state::<Backend>()
        .change_images(Some(image), move |db, image| {
            db.set_image(list_id, item_id, image)
        })
        .await
}
#[tauri::command]
async fn remove_image(app: AppHandle, list_id: i64, item_id: i64) -> Result<ListState, String> {
    app.state::<Backend>()
        .change_images(None, move |db, image| db.set_image(list_id, item_id, image))
        .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol(
            "pairrank-image",
            |context, request, responder| {
                let images = context.app_handle().state::<Backend>().images.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let response = match images {
                        Ok(service) => service.image_response(&request),
                        Err(_) => tauri::http::Response::builder()
                            .status(503)
                            .body(Vec::new())
                            .unwrap(),
                    };
                    responder.respond(response);
                });
            },
        )
        .setup(|app| {
            let directory = app.path().app_data_dir().map_err(|error| error.to_string());
            app.manage(Backend::open_with(directory, |_| {}));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_summaries,
            get_list,
            create_list,
            rename_list,
            delete_list,
            add_items,
            rename_item,
            delete_item,
            resume_list,
            next_pair,
            answer,
            search_settings,
            set_api_key,
            search_images,
            set_local_image,
            set_remote_image,
            remove_image
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[cfg(unix)]
    #[test]
    fn backend_initialization_rejects_app_data_replacement_between_child_stores() {
        for phase in 0..=2 {
            let root = tempfile::tempdir().unwrap();
            let app_data = root.path().join("app-data");
            let previous = root.path().join("previous-app-data");
            let first = Backend::open_with(Ok(app_data.clone()), |_| {});
            first
                .database
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .create_list("Saved rankings".into())
                .unwrap();
            drop(first);
            let contents = std::fs::read(app_data.join("pairrank.sqlite3")).unwrap();
            let backend = Backend::open_with(Ok(app_data.clone()), |stage| {
                if stage == phase {
                    std::fs::rename(&app_data, &previous).unwrap();
                    std::fs::create_dir(&app_data).unwrap();
                    std::fs::write(app_data.join("keep"), b"replacement").unwrap();
                }
            });
            assert!(backend.database.lock().unwrap().is_err(), "phase {phase}");
            assert!(backend.images.is_err(), "phase {phase}");
            assert!(!app_data.join("pairrank.sqlite3").exists());
            assert!(!app_data.join("images").exists());
            assert_eq!(
                std::fs::read(previous.join("pairrank.sqlite3")).unwrap(),
                contents
            );
            assert_eq!(
                std::fs::read(app_data.join("keep")).unwrap(),
                b"replacement"
            );
        }
    }

    fn backend() -> (tempfile::TempDir, Backend) {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("test.sqlite3")).unwrap();
        let images = ImageService::new(directory.path().to_owned()).unwrap();
        (
            directory,
            Backend {
                database: Arc::new(Mutex::new(Ok(database))),
                images: Ok(Arc::new(images)),
            },
        )
    }

    async fn list_with_image(backend: &Backend) -> (i64, i64, ImageAsset) {
        let image = backend
            .images
            .as_ref()
            .unwrap()
            .import_local(Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/32x32.png"))
            .unwrap();
        let asset = image.clone();
        backend
            .database_job(move |db| {
                let list = db.create_list("Images".into())?;
                let list = db.add_items(list.id, vec!["First".into(), "Second".into()])?;
                let item_id = list.items[0].id;
                db.set_image(list.id, item_id, Some(image))?;
                Ok((list.id, item_id, asset))
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn removing_an_image_removes_its_managed_file_after_saving_the_item() {
        let (directory, backend) = backend();
        let (list_id, item_id, image) = list_with_image(&backend).await;
        let state = backend
            .change_images(None, move |db, image| db.set_image(list_id, item_id, image))
            .await
            .unwrap();
        assert!(
            state
                .items
                .iter()
                .find(|item| item.id == item_id)
                .unwrap()
                .image
                .is_none()
        );
        assert!(!directory.path().join("images").join(&image.path).exists());
    }
    #[tokio::test]
    async fn failed_image_association_cleans_the_import_and_preserves_the_previous_image() {
        let (directory, backend) = backend();
        let (list_id, item_id, previous) = list_with_image(&backend).await;
        let image = backend
            .images
            .as_ref()
            .unwrap()
            .import_local(Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/32x32.png"))
            .unwrap();
        let imported_path = image.path.clone();
        rusqlite::Connection::open(directory.path().join("test.sqlite3")).unwrap()
            .execute_batch("CREATE TRIGGER fail_image AFTER UPDATE OF image_path ON items BEGIN SELECT RAISE(ABORT, 'disk failure'); END;").unwrap();
        assert!(
            backend
                .change_images(Some(image), move |db, image| db
                    .set_image(list_id, item_id, image))
                .await
                .is_err()
        );
        let state = backend
            .database_job(move |db| db.get_list(list_id))
            .await
            .unwrap();
        assert_eq!(
            state
                .items
                .iter()
                .find(|item| item.id == item_id)
                .unwrap()
                .image
                .as_ref()
                .unwrap()
                .path,
            previous.path
        );
        assert!(
            directory
                .path()
                .join("images")
                .join(&previous.path)
                .exists()
        );
        assert!(
            !directory
                .path()
                .join("images")
                .join(&imported_path)
                .exists()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn image_association_rejects_storage_replaced_after_import() {
        let (directory, backend) = backend();
        let (list_id, item_id, previous) = list_with_image(&backend).await;
        let image = backend
            .images
            .as_ref()
            .unwrap()
            .import_local(Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/32x32.png"))
            .unwrap();
        let imported_name = image.path.clone();
        let managed = directory.path().join("images");
        let displaced = directory.path().join("previous-images");
        std::fs::rename(&managed, &displaced).unwrap();
        std::fs::create_dir(&managed).unwrap();
        let foreign = managed.join(&imported_name);
        std::fs::write(&foreign, b"replacement image").unwrap();

        let result = backend
            .change_images(Some(image), move |db, image| {
                db.set_image(list_id, item_id, image)
            })
            .await;
        assert!(result.is_err());
        let state = backend
            .database_job(move |db| db.get_list(list_id))
            .await
            .unwrap();
        assert_eq!(
            state
                .items
                .iter()
                .find(|item| item.id == item_id)
                .unwrap()
                .image
                .as_ref()
                .unwrap()
                .path,
            previous.path
        );
        assert!(displaced.join(previous.path).exists());
        assert!(displaced.join(imported_name).exists());
        assert_eq!(std::fs::read(foreign).unwrap(), b"replacement image");
    }

    #[tokio::test]
    async fn replacements_and_deletions_keep_shared_and_in_flight_images_until_unused() {
        let (directory, backend) = backend();
        let (list_id, item_id, previous) = list_with_image(&backend).await;
        let service = backend.images.as_ref().unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/32x32.png");
        let image = service.import_local(source.clone()).unwrap();
        let image_path = image.path.clone();
        let in_flight = service.import_local(source.clone()).unwrap();
        let shared = image.clone();
        backend
            .change_images(Some(image), move |db, image| {
                db.set_image(list_id, item_id, image)
            })
            .await
            .unwrap();
        assert!(
            !directory
                .path()
                .join("images")
                .join(&previous.path)
                .exists()
        );
        assert!(directory.path().join("images").join(&image_path).exists());
        let second_id = backend
            .database_job(move |db| {
                let list = db.create_list("Second list".into())?;
                let list = db.add_items(list.id, vec!["Shared image".into()])?;
                db.set_image(list.id, list.items[0].id, Some(shared))?;
                Ok(list.id)
            })
            .await
            .unwrap();
        backend
            .change_images(None, move |db, _| db.delete_item(list_id, item_id))
            .await
            .unwrap();
        assert!(directory.path().join("images").join(&image_path).exists());
        backend
            .change_images(None, move |db, _| db.delete_list(second_id))
            .await
            .unwrap();
        assert!(!directory.path().join("images").join(&image_path).exists());
        assert!(
            directory
                .path()
                .join("images")
                .join(&in_flight.path)
                .exists()
        );
        assert!(source.exists());
    }

    #[tokio::test]
    async fn failed_association_preserves_referenced_images_even_when_database_lock_is_poisoned() {
        let (directory, backend) = backend();
        let (list_id, item_id, image) = list_with_image(&backend).await;
        let path = image.path.clone();
        let database = backend.database.clone();
        let _ = std::thread::spawn(move || {
            let _guard = database.lock().unwrap();
            panic!("simulated prior worker failure");
        })
        .join();
        assert!(
            backend
                .change_images(Some(image), move |db, image| db
                    .set_image(list_id, item_id, image))
                .await
                .is_err()
        );
        assert!(directory.path().join("images").join(&path).exists());
        let unused = backend
            .images
            .as_ref()
            .unwrap()
            .import_local(Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/32x32.png"))
            .unwrap();
        let unused_path = unused.path.clone();
        assert!(
            backend
                .change_images(Some(unused), move |db, image| db
                    .set_image(list_id, item_id, image))
                .await
                .is_err()
        );
        assert!(!directory.path().join("images").join(&unused_path).exists());
    }

    #[tokio::test]
    async fn cleanup_preserves_external_files_and_does_not_fail_a_committed_removal() {
        let (directory, backend) = backend();
        let (list_id, item_id, image) = list_with_image(&backend).await;
        std::fs::remove_file(directory.path().join("images").join(&image.path)).unwrap();
        std::fs::create_dir(directory.path().join("images").join(&image.path)).unwrap();
        let state = backend
            .change_images(None, move |db, image| db.set_image(list_id, item_id, image))
            .await
            .unwrap();
        assert!(
            state
                .items
                .iter()
                .find(|item| item.id == item_id)
                .unwrap()
                .image
                .is_none()
        );
        assert!(directory.path().join("images").join(&image.path).is_dir());
        let external = directory
            .path()
            .join(format!("{}.png", uuid::Uuid::new_v4()));
        std::fs::write(&external, b"keep external source").unwrap();
        let asset = ImageAsset {
            path: external.to_string_lossy().into_owned(),
            source_url: None,
        };
        backend
            .database_job(move |db| db.set_image(list_id, item_id, Some(asset)))
            .await
            .unwrap();
        backend
            .change_images(None, move |db, _| db.delete_list(list_id))
            .await
            .unwrap();
        assert_eq!(std::fs::read(external).unwrap(), b"keep external source");
    }
    #[tokio::test]
    async fn image_cleanup_tracks_a_replacement_committed_by_another_connection() {
        let (directory, backend) = backend();
        let (list_id, item_id, _) = list_with_image(&backend).await;
        let service = backend.images.as_ref().unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/32x32.png");
        let replacement = service.import_local(source.clone()).unwrap();
        let replacement_path = replacement.path.clone();
        let in_flight = service.import_local(source).unwrap();
        let mut other = Database::open(&directory.path().join("test.sqlite3")).unwrap();
        backend
            .change_images(None, move |db, image| {
                other.set_image(list_id, item_id, Some(replacement))?;
                db.set_image(list_id, item_id, image)
            })
            .await
            .unwrap();
        assert!(
            !directory
                .path()
                .join("images")
                .join(&replacement_path)
                .exists(),
            "cleanup must include the image replaced immediately before this transaction"
        );
        assert!(
            directory
                .path()
                .join("images")
                .join(&in_flight.path)
                .exists()
        );
    }

    #[tokio::test]
    async fn list_deletion_cleans_a_replacement_committed_by_another_connection() {
        let (directory, backend) = backend();
        let (list_id, item_id, _) = list_with_image(&backend).await;
        let replacement = backend
            .images
            .as_ref()
            .unwrap()
            .import_local(Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/32x32.png"))
            .unwrap();
        let replacement_path = replacement.path.clone();
        let mut other = Database::open(&directory.path().join("test.sqlite3")).unwrap();
        backend
            .change_images(None, move |db, _| {
                other.set_image(list_id, item_id, Some(replacement))?;
                db.delete_list(list_id)
            })
            .await
            .unwrap();
        assert!(
            !directory
                .path()
                .join("images")
                .join(&replacement_path)
                .exists()
        );
    }
}
