mod credentials;
mod database;
mod images;
mod models;
mod rating;

use database::Database;
use images::{ImageCandidate, ImageService, SearchProvider, SearchSettings};
use models::{ListState, ListSummary};
use rating::Preference;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

struct Backend {
    database: Arc<Mutex<Result<Database, String>>>,
    images: Result<Arc<ImageService>, String>,
}

async fn database_job<T: Send + 'static>(
    app: AppHandle,
    operation: impl FnOnce(&mut Database) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let database = app.state::<Backend>().database.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = database
            .lock()
            .map_err(|_| "データ操作を続けられません。アプリを再起動してください。".to_string())?;
        let database = guard.as_mut().map_err(|error| error.clone())?;
        operation(database)
    })
    .await
    .map_err(|error| format!("データ処理に失敗しました: {error}"))?
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
    database_job(app, move |db| db.delete_list(list_id)).await
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
    database_job(app, move |db| db.delete_item(list_id, item_id)).await
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
        let list = db.get_list(list_id)?;
        let ratings = list
            .items
            .iter()
            .map(|item| (item.id, item.rating))
            .collect::<Vec<_>>();
        let Some((mut a_id, mut b_id)) = rating::select_pair(&ratings, &db.pair_counts(list_id)?)?
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
    let service = image_service(&app)?;
    tauri::async_runtime::spawn_blocking(move || service.settings())
        .await
        .map_err(|error| error.to_string())?
}
#[tauri::command]
async fn set_api_key(
    app: AppHandle,
    provider: SearchProvider,
    key: String,
) -> Result<SearchSettings, String> {
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
async fn set_local_image(app: AppHandle, list_id: i64, item_id: i64) -> Result<ListState, String> {
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
    database_job(app, move |db| match image {
        Some(image) => db.set_image(list_id, item_id, Some(image)),
        None => db.get_list(list_id),
    })
    .await
}
#[tauri::command]
async fn set_remote_image(
    app: AppHandle,
    list_id: i64,
    item_id: i64,
    candidate: ImageCandidate,
) -> Result<ListState, String> {
    let image = image_service(&app)?.import_remote(candidate).await?;
    database_job(app, move |db| db.set_image(list_id, item_id, Some(image))).await
}
#[tauri::command]
async fn remove_image(app: AppHandle, list_id: i64, item_id: i64) -> Result<ListState, String> {
    database_job(app, move |db| db.set_image(list_id, item_id, None)).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let directory = app.path().app_data_dir().map_err(|error| error.to_string());
            let database = directory.clone().and_then(|path| {
                std::fs::create_dir_all(&path)
                    .map_err(|error| format!("保存先を作成できません: {error}"))?;
                Database::open(&path.join("pairrank.sqlite3"))
            });
            let images = directory.and_then(ImageService::new).map(Arc::new);
            app.manage(Backend {
                database: Arc::new(Mutex::new(database)),
                images,
            });
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
