mod credentials;
mod database;
mod images;
mod models;
mod rating;
mod sqlite_vfs;
mod storage;

use database::{Database, ImageChange};
use images::{ImageCandidate, ImageService, SearchProvider, SearchSettings};
use models::{ImageAsset, ListState, ListSummary};
use rating::Preference;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

const MAX_IMAGE_RESPONSES: usize = 4;

struct Backend {
    database: Arc<Mutex<Result<Database, String>>>,
    database_slots: Arc<tokio::sync::Semaphore>,
    images: Result<Arc<ImageService>, String>,
    image_response_slots: Arc<tokio::sync::Semaphore>,
}

impl Backend {
    fn open_with(
        directory: Result<std::path::PathBuf, String>,
        mut checkpoint: impl FnMut(u8),
    ) -> Self {
        let directory = directory.and_then(|path| {
            storage::PreparedAppData::open(path)
                .map(Arc::new)
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
                let database =
                    Database::open_in(directory.clone(), std::ffi::OsStr::new("pairrank.sqlite3"))?;
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
                let images = ImageService::open(database)?;
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
            database_slots: Arc::new(tokio::sync::Semaphore::new(1)),
            images,
            image_response_slots: Arc::new(tokio::sync::Semaphore::new(MAX_IMAGE_RESPONSES)),
        }
    }

    async fn database_job<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Database) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let permit = self
            .database_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "データ処理を続けられません。アプリを再起動してください。".to_owned())?;
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
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
            let Ok(permit) = self.database_slots.clone().acquire_owned().await else {
                return result;
            };
            let database = self.database.clone();
            let images = images.clone();
            // Inspect even a poisoned lock for cleanup, without allowing another mutation.
            let _ = tokio::task::spawn_blocking(move || {
                let _permit = permit;
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
    let mut paths = paths.into_iter().peekable();
    if paths.peek().is_none() {
        return;
    }
    if let Ok(references) = db.image_paths() {
        service.remove_unused_files(paths, &references);
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
    image_service(&app)?.set_api_key(provider, key).await
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
    let image = service
        .pick_local(move || {
            let Some(file) = dialog_app
                .dialog()
                .file()
                .add_filter("画像", &["png", "jpg", "jpeg", "webp"])
                .blocking_pick_file()
            else {
                return Ok(None);
            };
            file.into_path()
                .map(Some)
                .map_err(|error| error.to_string())
        })
        .await?;
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

async fn respond_to_image_request(
    images: Result<Arc<ImageService>, String>,
    slots: Arc<tokio::sync::Semaphore>,
    request: tauri::http::Request<Vec<u8>>,
    respond: impl FnOnce(tauri::http::Response<Vec<u8>>) + Send + 'static,
) -> Result<(), tokio::task::JoinError> {
    let Ok(permit) = slots.acquire_owned().await else {
        respond(
            tauri::http::Response::builder()
                .status(503)
                .body(Vec::new())
                .unwrap(),
        );
        return Ok(());
    };
    tokio::task::spawn_blocking(move || {
        // Cancellation cannot release capacity while this worker still owns the response.
        let _permit = permit;
        let response = match images {
            Ok(service) => service.image_response(&request),
            Err(_) => tauri::http::Response::builder()
                .status(503)
                .body(Vec::new())
                .unwrap(),
        };
        respond(response);
    })
    .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol(
            "pairrank-image",
            |context, request, responder| {
                let backend = context.app_handle().state::<Backend>();
                tauri::async_runtime::spawn(respond_to_image_request(
                    backend.images.clone(),
                    backend.image_response_slots.clone(),
                    request,
                    move |response| responder.respond(response),
                ));
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

    fn image_request(path: &str, method: &str) -> tauri::http::Request<Vec<u8>> {
        tauri::http::Request::builder()
            .method(method)
            .uri(format!("pairrank-image://localhost/{path}"))
            .body(Vec::new())
            .unwrap()
    }

    #[test]
    fn image_requests_wait_before_spawning_workers() {
        use std::future::Future;
        use std::task::Poll;
        use std::time::Duration;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(MAX_IMAGE_RESPONSES + 1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let (_directory, backend) = backend();
            let (_, _, image) = list_with_image(&backend).await;
            let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
            let mut releases = Vec::new();
            let mut active = Vec::new();
            let mut waiting = Vec::new();
            for index in 0..MAX_IMAGE_RESPONSES * 3 {
                let (release, wait) = std::sync::mpsc::channel::<()>();
                releases.push(release);
                let started = started.clone();
                let job = respond_to_image_request(
                    backend.images.clone(),
                    backend.image_response_slots.clone(),
                    image_request(&image.path, "GET"),
                    move |response| {
                        let _ = started.send(());
                        let _ = wait.recv();
                        assert_eq!(response.status(), tauri::http::StatusCode::OK);
                        assert!(!response.body().is_empty());
                    },
                );
                if index < MAX_IMAGE_RESPONSES {
                    active.push(tokio::spawn(job));
                } else {
                    waiting.push(Box::pin(job));
                }
            }
            for _ in 0..MAX_IMAGE_RESPONSES {
                tokio::time::timeout(Duration::from_secs(2), starts.recv())
                    .await
                    .unwrap()
                    .unwrap();
            }
            for job in &mut waiting {
                std::future::poll_fn(|cx| {
                    assert!(job.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
            }
            let (database, unrelated) = tokio::join!(
                tokio::time::timeout(
                    Duration::from_secs(2),
                    backend.database_job(|db| db.create_list("Still usable".into())),
                ),
                tokio::time::timeout(Duration::from_secs(2), tokio::task::spawn_blocking(|| 42),),
            );
            let extra_started = starts.try_recv().is_ok();
            // Unblock every response before asserting, including when admission is broken.
            drop(releases);
            for job in active {
                job.await.unwrap().unwrap();
            }
            for job in waiting {
                job.await.unwrap();
            }
            assert!(database.is_ok_and(|result| result.is_ok()));
            assert_eq!(unrelated.unwrap().unwrap(), 42);
            assert!(!extra_started);
        });
    }

    #[tokio::test]
    async fn cancelled_image_requests_keep_capacity_until_the_response_finishes() {
        use std::future::Future;
        use std::task::Poll;
        use std::time::Duration;

        let (_directory, mut backend) = backend();
        backend.image_response_slots = Arc::new(tokio::sync::Semaphore::new(1));
        let (_, _, image) = list_with_image(&backend).await;
        let (started, starts) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let first = tokio::spawn(respond_to_image_request(
            backend.images.clone(),
            backend.image_response_slots.clone(),
            image_request(&image.path, "GET"),
            move |response| {
                let _ = started.send(());
                let _ = wait.recv();
                assert_eq!(response.status(), tauri::http::StatusCode::OK);
            },
        ));
        tokio::time::timeout(Duration::from_secs(2), starts)
            .await
            .unwrap()
            .unwrap();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());

        let (responded, response) = tokio::sync::oneshot::channel();
        let mut cancelled = Box::pin(respond_to_image_request(
            backend.images.clone(),
            backend.image_response_slots.clone(),
            image_request(&image.path, "GET"),
            move |_| {
                let _ = responded.send(());
            },
        ));
        std::future::poll_fn(|cx| {
            assert!(cancelled.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(cancelled);
        let cancelled_response = tokio::time::timeout(Duration::from_secs(2), response).await;

        let next = respond_to_image_request(
            backend.images.clone(),
            backend.image_response_slots.clone(),
            image_request(&image.path, "HEAD"),
            |response| {
                assert_eq!(response.status(), tauri::http::StatusCode::OK);
                assert!(response.body().is_empty());
            },
        );
        tokio::pin!(next);
        let early = tokio::time::timeout(Duration::from_millis(100), &mut next).await;
        drop(release);
        assert!(cancelled_response.is_ok_and(|result| result.is_err()));
        assert!(early.is_err());
        tokio::time::timeout(Duration::from_secs(2), next)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn image_request_capacity_is_reusable_after_errors_and_panics() {
        use tauri::http::StatusCode;
        use tokio::time::{Duration, timeout};

        let (_directory, mut backend) = backend();
        backend.image_response_slots = Arc::new(tokio::sync::Semaphore::new(1));
        let (_, _, image) = list_with_image(&backend).await;
        let missing = format!("{}.png", uuid::Uuid::new_v4());
        for (images, path, status) in [
            (
                backend.images.clone(),
                missing.as_str(),
                StatusCode::NOT_FOUND,
            ),
            (
                backend.images.clone(),
                "external.png",
                StatusCode::FORBIDDEN,
            ),
            (
                Err("unavailable".into()),
                &image.path,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
        ] {
            timeout(
                Duration::from_secs(2),
                respond_to_image_request(
                    images,
                    backend.image_response_slots.clone(),
                    image_request(path, "GET"),
                    move |response| {
                        assert_eq!(response.status(), status);
                        assert!(response.body().is_empty());
                    },
                ),
            )
            .await
            .unwrap()
            .unwrap();
        }
        let panic = timeout(
            Duration::from_secs(2),
            respond_to_image_request(
                backend.images.clone(),
                backend.image_response_slots.clone(),
                image_request(&image.path, "GET"),
                |_| panic!("injected response failure"),
            ),
        )
        .await
        .unwrap();
        assert!(panic.unwrap_err().is_panic());
        for method in ["GET", "HEAD"] {
            timeout(
                Duration::from_secs(2),
                respond_to_image_request(
                    backend.images.clone(),
                    backend.image_response_slots.clone(),
                    image_request(&image.path, method),
                    move |response| {
                        assert_eq!(response.status(), StatusCode::OK);
                        assert_eq!(response.headers()["content-type"], "image/png");
                        assert_eq!(response.body().is_empty(), method == "HEAD");
                    },
                ),
            )
            .await
            .unwrap()
            .unwrap();
        }
        backend.image_response_slots.close();
        timeout(
            Duration::from_secs(2),
            respond_to_image_request(
                backend.images.clone(),
                backend.image_response_slots.clone(),
                image_request(&image.path, "GET"),
                |response| {
                    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
                    assert!(response.body().is_empty());
                },
            ),
        )
        .await
        .unwrap()
        .unwrap();
    }

    #[test]
    fn cancelled_database_requests_do_not_occupy_workers_waiting_for_the_mutex() {
        use std::future::Future;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::task::Poll;
        use std::time::Duration;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(2)
            .build()
            .unwrap();
        runtime.block_on(async {
            let (_directory, backend) = backend();
            let backend = Arc::new(backend);
            let (started, starts) = tokio::sync::oneshot::channel();
            let (release, wait) = std::sync::mpsc::channel::<()>();
            let first = tokio::spawn({
                let backend = backend.clone();
                async move {
                    backend
                        .database_job(move |_| {
                            started.send(()).unwrap();
                            let _ = wait.recv();
                            Ok(())
                        })
                        .await
                }
            });
            tokio::time::timeout(Duration::from_secs(2), starts)
                .await
                .unwrap()
                .unwrap();
            first.abort();
            assert!(first.await.unwrap_err().is_cancelled());
            let ran = Arc::new(AtomicBool::new(false));
            let mut second = Box::pin(backend.database_job({
                let ran = ran.clone();
                move |_| {
                    ran.store(true, Ordering::SeqCst);
                    Ok(())
                }
            }));
            std::future::poll_fn(|cx| {
                assert!(second.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            drop(second);
            let unrelated = tokio::time::timeout(
                Duration::from_millis(200),
                tokio::task::spawn_blocking(|| 42),
            )
            .await;
            drop(release);
            assert_eq!(unrelated.unwrap().unwrap(), 42);
            backend
                .database_job(|db| db.create_list("Still usable".into()))
                .await
                .unwrap();
            assert!(!ran.load(Ordering::SeqCst));
        });
    }

    #[tokio::test]
    async fn cancelled_local_picker_keeps_capacity_until_its_worker_exits() {
        use std::time::Duration;
        let (_directory, backend) = backend();
        let service = backend.images.as_ref().unwrap().clone();
        let (started, starts) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let first = tokio::spawn({
            let service = service.clone();
            async move {
                service
                    .pick_local(move || {
                        started.send(()).unwrap();
                        let _ = wait.recv();
                        Ok(None)
                    })
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), starts)
            .await
            .unwrap()
            .unwrap();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        let extra = service.pick_local(|| Ok(None)).await;
        drop(release);
        assert!(extra.is_err_and(|error| error.contains("画像の登録が実行中")));
        let image = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(image) = service
                    .pick_local(|| {
                        Ok(Some(
                            Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/32x32.png"),
                        ))
                    })
                    .await
                {
                    break image;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
        .unwrap();
        assert!(service.validate_import(&image).is_ok());
        assert!(service.pick_local(|| Ok(None)).await.unwrap().is_none());
    }

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

    #[tokio::test]
    async fn an_image_only_startup_failure_keeps_the_database_usable() {
        let directory = tempfile::tempdir().unwrap();
        let app_data = directory.path().join("app-data");
        std::fs::create_dir(&app_data).unwrap();
        std::fs::write(app_data.join("images"), b"ordinary file").unwrap();
        let backend = Backend::open_with(Ok(app_data.clone()), |_| {});
        assert!(backend.images.is_err());
        let list = backend
            .database_job(|database| database.create_list("Still usable".to_owned()))
            .await
            .unwrap();
        assert_eq!(list.name, "Still usable");
        assert_eq!(
            backend
                .database_job(|database| database.list_summaries())
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            std::fs::read(app_data.join("images")).unwrap(),
            b"ordinary file"
        );
    }

    fn backend() -> (tempfile::TempDir, Backend) {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("test.sqlite3")).unwrap();
        let images = ImageService::new(directory.path().to_owned()).unwrap();
        (
            directory,
            Backend {
                database: Arc::new(Mutex::new(Ok(database))),
                database_slots: Arc::new(tokio::sync::Semaphore::new(1)),
                images: Ok(Arc::new(images)),
                image_response_slots: Arc::new(tokio::sync::Semaphore::new(MAX_IMAGE_RESPONSES)),
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

    #[cfg(unix)]
    fn image_directory_alias(directory: &Path) -> (tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let alias = root.path().join("app-alias");
        std::os::unix::fs::symlink(directory, &alias).unwrap();
        (root, alias.join("images"))
    }

    fn upgrade_to_future_schema(path: &Path) -> (rusqlite::Connection, String) {
        let mut connection = database::test_connection(path).unwrap();
        let supported: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let future = supported + 1;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        transaction
            .execute_batch("CREATE TABLE future_metadata (value TEXT)")
            .unwrap();
        transaction
            .pragma_update(None, "user_version", future)
            .unwrap();
        transaction.commit().unwrap();
        (
            connection,
            format!(
                "データベースのバージョン {future} はこのアプリの対応版 {supported} より新しいため開けません。アプリを更新してください。"
            ),
        )
    }

    #[tokio::test]
    async fn startup_preserves_images_when_schema_is_upgraded_after_database_initialization() {
        let directory = tempfile::tempdir().unwrap();
        let first = Backend::open_with(Ok(directory.path().to_owned()), |_| {});
        let (_, _, image) = list_with_image(&first).await;
        let orphan = first
            .images
            .as_ref()
            .unwrap()
            .import_local(Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/32x32.png"))
            .unwrap();
        let paths =
            [image.path, orphan.path].map(|name| directory.path().join("images").join(name));
        let contents = paths.each_ref().map(|path| std::fs::read(path).unwrap());
        drop(first);

        let mut expected_error = String::new();
        let backend = Backend::open_with(Ok(directory.path().to_owned()), |stage| {
            if stage == 1 {
                (_, expected_error) =
                    upgrade_to_future_schema(&directory.path().join("pairrank.sqlite3"));
            }
        });

        assert_eq!(backend.images.as_ref().err(), Some(&expected_error));
        assert_eq!(
            backend
                .database_job(|db| db.list_summaries())
                .await
                .unwrap_err(),
            expected_error
        );
        for (path, contents) in paths.iter().zip(contents) {
            assert_eq!(std::fs::read(path).unwrap(), contents);
        }
    }

    #[tokio::test]
    async fn future_schema_rejects_image_changes_and_deletions_without_changing_references() {
        let (directory, backend) = backend();
        let (list_id, item_id, previous) = list_with_image(&backend).await;
        let imported = backend
            .images
            .as_ref()
            .unwrap()
            .import_local(Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/32x32.png"))
            .unwrap();
        let previous_path = directory.path().join("images").join(&previous.path);
        let previous_contents = std::fs::read(&previous_path).unwrap();
        let (connection, expected_error) =
            upgrade_to_future_schema(&directory.path().join("test.sqlite3"));
        let image_reference = || {
            connection
                .query_row(
                    "SELECT image_path, image_source_url, deleted FROM items
                     WHERE list_id = ?1 AND id = ?2",
                    rusqlite::params![list_id, item_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, bool>(2)?,
                        ))
                    },
                )
                .unwrap()
        };
        let previous_reference = image_reference();

        for operation in ["associate", "remove", "delete item", "delete list"] {
            let result = match operation {
                "associate" => backend
                    .change_images(Some(imported.clone()), move |db, image| {
                        db.set_image(list_id, item_id, image)
                    })
                    .await
                    .map(|_| ()),
                "remove" => backend
                    .change_images(None, move |db, image| db.set_image(list_id, item_id, image))
                    .await
                    .map(|_| ()),
                "delete item" => backend
                    .change_images(None, move |db, _| db.delete_item(list_id, item_id))
                    .await
                    .map(|_| ()),
                "delete list" => {
                    backend
                        .change_images(None, move |db, _| db.delete_list(list_id))
                        .await
                }
                _ => unreachable!(),
            };
            assert_eq!(result.unwrap_err(), expected_error, "{operation}");
            assert_eq!(image_reference(), previous_reference, "{operation}");
            assert_eq!(std::fs::read(&previous_path).unwrap(), previous_contents);
        }
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

    #[cfg(unix)]
    #[tokio::test]
    async fn image_changes_clean_legacy_references_through_a_parent_alias() {
        for operation in ["remove", "replace", "delete item", "delete list"] {
            let (directory, backend) = backend();
            let (list_id, item_id, mut previous) = list_with_image(&backend).await;
            let previous_path = directory.path().join("images").join(&previous.path);
            let (_alias_root, alias) = image_directory_alias(directory.path());
            previous.path = alias.join(&previous.path).to_str().unwrap().to_owned();
            backend
                .database_job(move |db| db.set_image(list_id, item_id, Some(previous)))
                .await
                .unwrap();
            let replacement = (operation == "replace").then(|| {
                backend
                    .images
                    .as_ref()
                    .unwrap()
                    .import_local(Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/32x32.png"))
                    .unwrap()
            });
            let replacement_path = replacement
                .as_ref()
                .map(|image| directory.path().join("images").join(&image.path));

            let result = match operation {
                "remove" | "replace" => backend
                    .change_images(replacement, move |db, image| {
                        db.set_image(list_id, item_id, image)
                    })
                    .await
                    .map(|_| ()),
                "delete item" => backend
                    .change_images(None, move |db, _| db.delete_item(list_id, item_id))
                    .await
                    .map(|_| ()),
                "delete list" => {
                    backend
                        .change_images(None, move |db, _| db.delete_list(list_id))
                        .await
                }
                _ => unreachable!(),
            };

            result.unwrap();
            assert!(!previous_path.exists(), "{operation}");
            if let Some(path) = replacement_path {
                assert!(path.is_file());
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn image_cleanup_preserves_shared_references_with_different_path_forms() {
        for (removed_form, retained_form) in [(0, 1), (0, 2), (1, 0), (1, 2), (2, 0), (2, 1)] {
            let (directory, backend) = backend();
            let (list_id, item_id, image) = list_with_image(&backend).await;
            let (_alias_root, alias) = image_directory_alias(directory.path());
            let managed = directory.path().canonicalize().unwrap().join("images");
            let image_path = managed.join(&image.path);
            let references = [
                image.path.clone(),
                image_path.to_str().unwrap().to_owned(),
                alias.join(&image.path).to_str().unwrap().to_owned(),
            ];
            let removed = ImageAsset {
                path: references[removed_form].clone(),
                source_url: None,
            };
            let retained = ImageAsset {
                path: references[retained_form].clone(),
                source_url: None,
            };
            let second_id = backend
                .database_job(move |db| {
                    db.set_image(list_id, item_id, Some(removed))?;
                    let list = db.create_list("Second list".into())?;
                    let list = db.add_items(list.id, vec!["Shared image".into()])?;
                    db.set_image(list.id, list.items[0].id, Some(retained))?;
                    Ok(list.id)
                })
                .await
                .unwrap();

            backend
                .change_images(None, move |db, _| db.delete_list(list_id))
                .await
                .unwrap();
            assert!(image_path.is_file(), "{removed_form} -> {retained_form}");
            backend
                .change_images(None, move |db, _| db.delete_list(second_id))
                .await
                .unwrap();
            assert!(!image_path.exists(), "{removed_form} -> {retained_form}");
        }
    }

    #[tokio::test]
    async fn live_cleanup_preserves_another_services_pending_image_and_same_named_external_file() {
        let (directory, backend) = backend();
        let (list_id, _, previous) = list_with_image(&backend).await;
        let second = ImageService::new(directory.path().to_owned()).unwrap();
        let pending = second
            .import_local(Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/32x32.png"))
            .unwrap();
        let pending_path = directory.path().join("images").join(&pending.path);
        let pending_contents = std::fs::read(&pending_path).unwrap();
        let external_root = tempfile::tempdir().unwrap();
        let external_path = external_root.path().join(&pending.path);
        std::fs::write(&external_path, b"external image").unwrap();
        let external = ImageAsset {
            path: external_path.to_str().unwrap().to_owned(),
            source_url: None,
        };
        backend
            .database_job(move |db| {
                let list = db.get_list(list_id)?;
                let empty = list.items.iter().find(|item| item.image.is_none()).unwrap();
                db.set_image(list_id, empty.id, Some(external))
            })
            .await
            .unwrap();

        backend
            .change_images(None, move |db, _| db.delete_list(list_id))
            .await
            .unwrap();

        assert!(!directory.path().join("images").join(previous.path).exists());
        assert_eq!(std::fs::read(pending_path).unwrap(), pending_contents);
        assert_eq!(std::fs::read(external_path).unwrap(), b"external image");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_association_preserves_an_image_referenced_by_an_absolute_or_alias_path() {
        for use_alias in [false, true] {
            let (directory, backend) = backend();
            let (list_id, item_id, image) = list_with_image(&backend).await;
            let (_alias_root, alias) = image_directory_alias(directory.path());
            let managed = directory.path().canonicalize().unwrap().join("images");
            let image_path = managed.join(&image.path);
            let contents = std::fs::read(&image_path).unwrap();
            let mut retained = image.clone();
            retained.path = (if use_alias { alias } else { managed })
                .join(&image.path)
                .to_str()
                .unwrap()
                .to_owned();
            let retained_path = retained.path.clone();
            backend
                .database_job(move |db| db.set_image(list_id, item_id, Some(retained)))
                .await
                .unwrap();

            assert!(
                backend
                    .change_images(Some(image), move |db, image| {
                        db.set_image(list_id, -1, image)
                    })
                    .await
                    .is_err()
            );

            assert_eq!(std::fs::read(image_path).unwrap(), contents);
            let state = backend
                .database_job(move |db| db.get_list(list_id))
                .await
                .unwrap();
            let item = state.items.iter().find(|item| item.id == item_id).unwrap();
            assert_eq!(item.image.as_ref().unwrap().path, retained_path);
        }
    }

    #[tokio::test]
    async fn cleanup_preserves_images_when_database_references_fail_after_a_committed_removal() {
        let (directory, backend) = backend();
        let (list_id, item_id, image) = list_with_image(&backend).await;
        let image_path = directory.path().join("images").join(&image.path);
        let contents = std::fs::read(&image_path).unwrap();
        let database_path = directory.path().join("test.sqlite3");

        let state = backend
            .change_images(None, move |db, image| {
                let change = db.set_image(list_id, item_id, image)?;
                upgrade_to_future_schema(&database_path);
                Ok(change)
            })
            .await
            .unwrap();

        let item = state.items.iter().find(|item| item.id == item_id).unwrap();
        assert!(item.image.is_none());
        assert!(backend.database_job(|db| db.image_paths()).await.is_err());
        assert_eq!(std::fs::read(image_path).unwrap(), contents);
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
        database::test_connection(directory.path().join("test.sqlite3")).unwrap()
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
