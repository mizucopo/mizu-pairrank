use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io::{Cursor, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use image::{ImageDecoder, ImageFormat, ImageReader};
use reqwest::{Client, Response, StatusCode, redirect::Policy};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use url::{Host, Url};
use uuid::Uuid;

use crate::credentials::{CredentialStore, SystemCredentials};
use crate::models::ImageAsset;

const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PIXELS: u64 = 32_000_000;
const MAX_REDIRECTS: usize = 4;
const SEARCH_PARALLELISM: usize = 4;
const MAX_BUFFERED_IMAGES: usize = 4;
const MAX_ACTIVE_SEARCHES: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchProvider {
    Brave,
    Ollama,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSettings {
    pub brave_configured: bool,
    pub ollama_configured: bool,
    pub default_provider: SearchProvider,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub errors: HashMap<SearchProvider, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageCandidate {
    /// Original image URL. Imports validate it again before making a request.
    pub id: String,
    pub title: String,
    /// A decoded, bounded PNG data URL; the webview makes no remote image requests.
    pub preview_url: String,
    pub source_url: String,
}

#[derive(Clone)]
struct Resource {
    bytes: Vec<u8>,
    final_url: String,
}

type NetworkFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

trait ImageTransport: Send + Sync {
    fn search<'a>(
        &'a self,
        provider: SearchProvider,
        key: &'a str,
        query: &'a str,
    ) -> NetworkFuture<'a, Vec<u8>>;
    fn fetch<'a>(&'a self, url: &'a str, limit: usize) -> NetworkFuture<'a, Resource>;
}

struct HttpTransport;

impl ImageTransport for HttpTransport {
    fn search<'a>(
        &'a self,
        provider: SearchProvider,
        key: &'a str,
        query: &'a str,
    ) -> NetworkFuture<'a, Vec<u8>> {
        Box::pin(async move {
            // Dedicated requests with fixed hosts and no redirects: credentials are never
            // attached to image/page requests or forwarded to a different origin.
            let client = client_builder()
                .build()
                .map_err(|_| "検索接続を準備できませんでした。".to_owned())?;
            let request = search_request(&client, provider, key, query)?;
            let response = client.execute(request).await.map_err(|_| {
                "検索サービスに接続できませんでした。通信状態を確認してください。".to_owned()
            })?;
            check_search_status(response.status())?;
            read_response(response, MAX_PAGE_BYTES).await
        })
    }

    fn fetch<'a>(&'a self, url: &'a str, limit: usize) -> NetworkFuture<'a, Resource> {
        Box::pin(async move {
            let mut current = validate_public_url(url)?;
            for redirect_count in 0..=MAX_REDIRECTS {
                let host = current
                    .host_str()
                    .ok_or_else(|| "画像URLにホスト名がありません。".to_owned())?;
                let port = current.port_or_known_default().unwrap_or(443);
                let addresses = resolve_public_addresses(&current, port).await?;
                // Pin the already validated resolution, avoiding a second DNS lookup and
                // DNS rebinding. Proxies are disabled because they would resolve separately.
                let client = client_builder()
                    .resolve_to_addrs(host, &addresses)
                    .build()
                    .map_err(|_| "画像接続を準備できませんでした。".to_owned())?;
                let response = client
                    .get(current.clone())
                    .send()
                    .await
                    .map_err(|_| "画像または検索元ページを取得できませんでした。".to_owned())?;
                if response.status().is_redirection() {
                    if redirect_count == MAX_REDIRECTS {
                        return Err("画像URLのリダイレクト回数が上限を超えました。".to_owned());
                    }
                    let location = response
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .ok_or_else(|| "リダイレクト先のURLを読み取れませんでした。".to_owned())?;
                    current = redirected_url(&current, location)?;
                    continue;
                }
                if !response.status().is_success() {
                    return Err("画像または検索元ページのサーバーが取得を拒否しました。".to_owned());
                }
                return Ok(Resource {
                    bytes: read_response(response, limit).await?,
                    final_url: current.to_string(),
                });
            }
            unreachable!("redirect loop returns after its bounded final iteration")
        })
    }
}

fn client_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
}

fn search_request(
    client: &Client,
    provider: SearchProvider,
    key: &str,
    query: &str,
) -> Result<reqwest::Request, String> {
    let request = match provider {
        SearchProvider::Brave => client
            .get("https://api.search.brave.com/res/v1/images/search")
            .header("X-Subscription-Token", key)
            .query(&[
                ("q", query),
                ("country", "JP"),
                ("search_lang", "ja"),
                ("count", "30"),
                ("safesearch", "strict"),
            ]),
        SearchProvider::Ollama => client
            .post("https://ollama.com/api/web_search")
            .bearer_auth(key)
            .json(&serde_json::json!({"query": query, "max_results": 10})),
    };
    request
        .header("Accept", "application/json")
        .build()
        .map_err(|_| "検索要求を作成できませんでした。APIキーを確認してください。".to_owned())
}

fn check_search_status(status: StatusCode) -> Result<(), String> {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            Err("検索サービスの認証に失敗しました。APIキーを確認してください。".to_owned())
        }
        StatusCode::TOO_MANY_REQUESTS => {
            Err("検索サービスの利用上限に達しました。時間をおいて再試行してください。".to_owned())
        }
        _ if status.is_success() => Ok(()),
        _ => Err("検索サービスでエラーが発生しました。時間をおいて再試行してください。".to_owned()),
    }
}

async fn read_response(mut response: Response, limit: usize) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err("取得データがサイズ上限を超えています。".to_owned());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "取得データを最後まで読み取れませんでした。".to_owned())?
    {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err("取得データがサイズ上限を超えています。".to_owned());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn validate_public_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|_| "画像URLが正しくありません。".to_owned())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host().is_none()
        || !matches!(url.port_or_known_default(), Some(80 | 443))
    {
        return Err("画像は公開されたHTTP/HTTPSのURLから取得してください。".to_owned());
    }
    match url.host() {
        Some(Host::Ipv4(ip)) if !is_public_ip(ip.into()) => return Err(private_network_error()),
        Some(Host::Ipv6(ip)) if !is_public_ip(ip.into()) => return Err(private_network_error()),
        Some(Host::Domain(host)) => {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            if host == "localhost"
                || host.ends_with(".localhost")
                || host.ends_with(".local")
                || !host.contains('.')
            {
                return Err(private_network_error());
            }
        }
        _ => {}
    }
    Ok(url)
}

fn redirected_url(base: &Url, location: &str) -> Result<Url, String> {
    let target = base
        .join(location)
        .map_err(|_| "リダイレクト先のURLが正しくありません。".to_owned())?;
    validate_public_url(target.as_str())
}

fn private_network_error() -> String {
    "ローカル・プライベートネットワークのURLは取得できません。".to_owned()
}

async fn resolve_public_addresses(url: &Url, port: u16) -> Result<Vec<SocketAddr>, String> {
    let addresses: Vec<_> = match url.host() {
        Some(Host::Ipv4(ip)) => vec![SocketAddr::new(ip.into(), port)],
        Some(Host::Ipv6(ip)) => vec![SocketAddr::new(ip.into(), port)],
        Some(Host::Domain(host)) => tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::lookup_host((host, port)),
        )
        .await
        .map_err(|_| "画像URLの名前解決がタイムアウトしました。".to_owned())?
        .map_err(|_| "画像URLの名前解決に失敗しました。".to_owned())?
        .collect(),
        None => return Err("画像URLにホスト名がありません。".to_owned()),
    };
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(private_network_error());
    }
    Ok(addresses)
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let segments = ip.segments();
            // Only globally routed unicast; exclude transition/documentation prefixes
            // which can encapsulate an otherwise blocked IPv4 destination.
            (segments[0] & 0xe000) == 0x2000
                && segments[0] != 0x2002
                && !(segments[0] == 0x2001 && (segments[1] < 0x0200 || segments[1] == 0x0db8))
                && !(segments[0] == 0x3fff && (segments[1] & 0xf000) == 0)
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !matches!(a, 0 | 10 | 127 | 224..=255)
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 169 && b == 254)
        && !(a == 172 && (16..=31).contains(&b))
        && !(a == 192 && (b == 168 || (b == 0 && matches!(c, 0 | 2)) || (b == 88 && c == 99)))
        && !(a == 198 && (matches!(b, 18 | 19) || (b == 51 && c == 100)))
        && !(a == 203 && b == 0 && c == 113)
}

pub struct ImageService {
    directory: PathBuf,
    storage_lease: Arc<std::fs::File>,
    credentials: Arc<dyn CredentialStore>,
    transport: Arc<dyn ImageTransport>,
    decode_gate: Arc<Mutex<()>>,
    buffer_slots: Arc<Semaphore>,
    search_slots: Semaphore,
    settings_slots: Arc<Semaphore>,
}

impl ImageService {
    pub fn open(app_data: PathBuf, database: &crate::database::Database) -> Result<Self, String> {
        Self::open_storage(app_data, Some(database))
    }

    #[cfg(test)]
    pub fn new(app_data: PathBuf) -> Result<Self, String> {
        Self::open_storage(app_data, None)
    }

    fn open_storage(
        app_data: PathBuf,
        database: Option<&crate::database::Database>,
    ) -> Result<Self, String> {
        let directory = app_data.join("images");
        crate::storage::create_private_directory(&directory)
            .map_err(|_| "画像の保存フォルダーを作成できませんでした。".to_owned())?;
        sync_directory(&app_data)
            .map_err(|_| "画像の保存フォルダーを同期できませんでした。".to_owned())?;
        let directory = directory
            .canonicalize()
            .map_err(|_| "画像の保存フォルダーを開けませんでした。".to_owned())?;
        #[cfg(unix)]
        restrict_image_permissions(&directory)
            .map_err(|_| "保存済み画像の権限を設定できませんでした。".to_owned())?;
        let storage_lease = image_storage_lock(&app_data)
            .map_err(|_| "画像の保存先ロックを開けませんでした。".to_owned())?;
        match storage_lease.try_lock() {
            Ok(()) => {
                if let Some(database) = database {
                    // Read the complete reference set before deleting anything. No other
                    // ImageService can import or associate files while this lock is exclusive.
                    let references = database.image_paths()?;
                    reconcile_images(&directory, &references)
                        .map_err(|_| "未使用画像を確認できませんでした。".to_owned())?;
                }
                storage_lease
                    .unlock()
                    .map_err(|_| "画像の保存先ロックを解除できませんでした。".to_owned())?;
            }
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(_) => return Err("画像の保存先をロックできませんでした。".to_owned()),
        }
        // Keep every live instance protected, including the interval between saving a
        // PNG and committing its reference. Reconcile on the next standalone startup.
        storage_lease
            .lock_shared()
            .map_err(|_| "画像の保存先をロックできませんでした。".to_owned())?;
        Ok(Self {
            directory,
            storage_lease: Arc::new(storage_lease),
            credentials: Arc::new(SystemCredentials),
            transport: Arc::new(HttpTransport),
            decode_gate: Arc::new(Mutex::new(())),
            buffer_slots: Arc::new(Semaphore::new(MAX_BUFFERED_IMAGES)),
            search_slots: Semaphore::new(MAX_ACTIVE_SEARCHES),
            settings_slots: Arc::new(Semaphore::new(1)),
        })
    }

    pub fn remove_managed_file(&self, path: &str) {
        self.remove_managed_file_with(path, || {});
    }

    fn remove_managed_file_with(&self, path: &str, checkpoint: impl FnOnce()) {
        let Some(path) = self.managed_path(path) else {
            return;
        };
        let remove = || -> std::io::Result<()> {
            let pinned = verified_image_directory(&self.directory)?;
            let filename = path
                .file_name()
                .ok_or_else(|| std::io::Error::other("missing image filename"))?;
            if !pinned.symlink_metadata(filename)?.is_file() {
                return Ok(());
            }
            checkpoint();
            pinned.remove_file(filename)?;
            #[cfg(unix)]
            pinned.into_std_file().sync_all()?;
            Ok(())
        };
        // Cleanup must never turn a committed database operation into a failure.
        let _ = remove();
    }

    fn managed_path(&self, reference: &str) -> Option<PathBuf> {
        // Joining an old absolute reference preserves its original location.
        let path = self.directory.join(reference);
        (path.parent() == Some(self.directory.as_path()) && is_managed_image_name(&path))
            .then_some(path)
    }

    pub fn image_response(
        &self,
        request: &tauri::http::Request<Vec<u8>>,
    ) -> tauri::http::Response<Vec<u8>> {
        use tauri::http::{Method, Response, StatusCode};
        let read = || -> Result<(Vec<u8>, u64), StatusCode> {
            if request.method() != Method::GET && request.method() != Method::HEAD {
                return Err(StatusCode::METHOD_NOT_ALLOWED);
            }
            let reference = request.uri().path().strip_prefix('/').unwrap_or_default();
            if reference.contains(['/', '\\']) {
                return Err(StatusCode::FORBIDDEN);
            }
            let path = self.managed_path(reference).ok_or(StatusCode::FORBIDDEN)?;
            let metadata = std::fs::symlink_metadata(&path).map_err(|_| StatusCode::NOT_FOUND)?;
            if !metadata.is_file()
                || !path
                    .canonicalize()
                    .is_ok_and(|resolved| resolved.parent() == Some(self.directory.as_path()))
            {
                return Err(StatusCode::FORBIDDEN);
            }
            if metadata.len() > MAX_IMAGE_BYTES as u64 {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            if request.method() == Method::HEAD {
                return Ok((Vec::new(), metadata.len()));
            }
            let file = std::fs::File::open(path).map_err(|_| StatusCode::NOT_FOUND)?;
            let mut bytes = Vec::new();
            file.take(MAX_IMAGE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            if bytes.len() > MAX_IMAGE_BYTES {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            let length = bytes.len() as u64;
            Ok((bytes, length))
        };
        let response = Response::builder()
            .header("Cache-Control", "no-store")
            .header("X-Content-Type-Options", "nosniff");
        match read() {
            Ok((bytes, length)) => response
                .header("Content-Type", "image/png")
                .header("Content-Length", length)
                .body(bytes)
                .unwrap(),
            Err(status) => response.status(status).body(Vec::new()).unwrap(),
        }
    }

    pub async fn settings(&self) -> Result<SearchSettings, String> {
        let permit = self
            .settings_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                "検索設定の読み込みが実行中です。少し待ってから再試行してください。".to_owned()
            })?;
        let credentials = self.credentials.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut errors = HashMap::new();
            let mut configured = |provider| match credentials.get(provider) {
                Ok(key) => key.is_some(),
                Err(error) => {
                    errors.insert(provider, error);
                    false
                }
            };
            let brave_configured = configured(SearchProvider::Brave);
            let ollama_configured = configured(SearchProvider::Ollama);
            Ok(SearchSettings {
                brave_configured,
                ollama_configured,
                default_provider: if ollama_configured && !brave_configured {
                    SearchProvider::Ollama
                } else {
                    SearchProvider::Brave
                },
                errors,
            })
        })
        .await
        .map_err(|_| "検索設定を取得できませんでした。".to_owned())?
    }

    pub fn set_api_key(&self, provider: SearchProvider, key: String) -> Result<(), String> {
        let key = key.trim();
        if key.len() > 4096 || key.chars().any(char::is_control) {
            return Err("APIキーの形式が正しくありません。".to_owned());
        }
        self.credentials.set(provider, key)
    }

    pub async fn search(
        &self,
        provider: SearchProvider,
        query: String,
    ) -> Result<Vec<ImageCandidate>, String> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 400 || query.split_whitespace().count() > 50
        {
            return Err("検索語は1〜400文字、50語以内で入力してください。".to_owned());
        }
        // Keep completed previews bounded too: a dismissed invoke still runs natively.
        // Reject excess searches instead of retaining an unbounded queue of commands.
        let _search = self
            .search_slots
            .try_acquire()
            .map_err(|_| "画像検索が実行中です。少し待ってから再試行してください。".to_owned())?;
        let credentials = self.credentials.clone();
        let key = tokio::task::spawn_blocking(move || credentials.get(provider))
            .await
            .map_err(|_| "APIキーを取得できませんでした。".to_owned())??
            .ok_or_else(|| "選択した検索サービスのAPIキーを設定してください。".to_owned())?;
        let payload = self.transport.search(provider, &key, query).await?;
        let seeds = parse_search_response(provider, &payload)?;
        tokio::time::timeout(Duration::from_secs(90), self.prepare_candidates(seeds))
            .await
            .map_err(|_| {
                "画像候補の取得がタイムアウトしました。検索語を変えて再試行してください。"
                    .to_owned()
            })?
    }

    async fn prepare_candidates(
        &self,
        seeds: Vec<CandidateSeed>,
    ) -> Result<Vec<ImageCandidate>, String> {
        let mut pending = seeds.into_iter().enumerate();
        let mut active = JoinSet::new();
        let mut completed = Vec::new();
        loop {
            while active.len() < SEARCH_PARALLELISM {
                let Some((position, seed)) = pending.next() else {
                    break;
                };
                let transport = self.transport.clone();
                let decode_gate = self.decode_gate.clone();
                let buffer_slots = self.buffer_slots.clone();
                active.spawn(async move {
                    (
                        position,
                        prepare_candidate(transport, decode_gate, buffer_slots, seed).await,
                    )
                });
            }
            let Some(result) = active.join_next().await else {
                break;
            };
            if let Ok((position, Ok(candidate))) = result {
                completed.push((position, candidate));
            }
        }
        completed.sort_by_key(|(position, _)| *position);
        let mut seen = HashSet::new();
        Ok(completed
            .into_iter()
            .map(|(_, candidate)| candidate)
            .filter(|candidate| seen.insert(candidate.id.clone()))
            .collect())
    }

    pub async fn import_remote(&self, candidate: ImageCandidate) -> Result<ImageAsset, String> {
        validate_public_url(&candidate.id)?;
        validate_public_url(&candidate.source_url)?;
        let buffer_permit = self
            .buffer_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "画像処理を続けられません。アプリを再起動してください。".to_owned())?;
        let resource = self.transport.fetch(&candidate.id, MAX_IMAGE_BYTES).await?;
        let directory = self.directory.clone();
        let decode_gate = self.decode_gate.clone();
        let storage_lease = self.storage_lease.clone();
        tokio::task::spawn_blocking(move || {
            // Keep the encoded buffer charged even if the invoking future is cancelled.
            let _buffer_permit = buffer_permit;
            let _storage_lease = storage_lease;
            let bytes = resource.bytes;
            let _permit = decode_gate
                .lock()
                .map_err(|_| "画像処理を続けられません。アプリを再起動してください。".to_owned())?;
            save_image(&directory, &bytes, Some(candidate.source_url))
        })
        .await
        .map_err(|_| "画像保存処理を完了できませんでした。".to_owned())?
    }

    /// The path comes only from the native file chooser, not a webview capability.
    pub fn import_local(&self, path: PathBuf) -> Result<ImageAsset, String> {
        let file = std::fs::File::open(path)
            .map_err(|_| "選択した画像ファイルを開けませんでした。".to_owned())?;
        let metadata = file
            .metadata()
            .map_err(|_| "選択した画像ファイルを確認できませんでした。".to_owned())?;
        if !metadata.is_file() || metadata.len() > MAX_IMAGE_BYTES as u64 {
            return Err("20MiB以下の画像ファイルを選択してください。".to_owned());
        }
        let _permit = self
            .decode_gate
            .lock()
            .map_err(|_| "画像処理を続けられません。アプリを再起動してください。".to_owned())?;
        let mut bytes = Vec::new();
        file.take(MAX_IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "選択した画像ファイルを読み取れませんでした。".to_owned())?;
        save_image(&self.directory, &bytes, None)
    }
}

fn image_storage_lock(app_data: &Path) -> std::io::Result<std::fs::File> {
    let path = app_data.join(".images.lock");
    match crate::storage::create_private_file(&path) {
        Ok(file) => {
            file.sync_all()?;
            sync_directory(app_data)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if !std::fs::symlink_metadata(&path)?.is_file() {
                return Err(std::io::Error::other(
                    "image storage lock is not a regular file",
                ));
            }
            #[cfg(unix)]
            crate::storage::restrict_existing_file(&path)?;
        }
        Err(error) => return Err(error),
    }
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
}

fn reconcile_images(directory: &Path, references: &HashSet<String>) -> std::io::Result<()> {
    let pinned = verified_image_directory(directory)?;
    let mut removed = false;
    for entry in pinned.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        let path = directory.join(&name);
        if !entry.file_type()?.is_file() || !is_managed_image_name(&path) {
            continue;
        }
        if name.to_str().is_some_and(|name| references.contains(name))
            || path.to_str().is_some_and(|path| references.contains(path))
        {
            continue;
        }
        match pinned.remove_file(name) {
            Ok(()) => removed = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    if removed {
        #[cfg(unix)]
        pinned.into_std_file().sync_all()?;
    }
    Ok(())
}

fn open_image_directory(directory: &Path) -> std::io::Result<cap_std::fs::Dir> {
    // A readable Unix handle supports fsync; Windows denies delete sharing.
    #[cfg(unix)]
    return std::fs::File::open(directory).map(cap_std::fs::Dir::from_std_file);
    #[cfg(not(unix))]
    cap_std::fs::Dir::open_ambient_dir(directory, cap_std::ambient_authority())
}

fn verified_image_directory(directory: &Path) -> std::io::Result<cap_std::fs::Dir> {
    let pinned = open_image_directory(directory)?;
    let identity = same_file::Handle::from_file(pinned.try_clone()?.into_std_file())?;
    if !std::fs::symlink_metadata(directory)?.is_dir()
        || directory.canonicalize()? != directory
        || same_file::Handle::from_path(directory)? != identity
    {
        return Err(std::io::Error::other(
            "image storage directory was replaced",
        ));
    }
    Ok(pinned)
}

fn is_managed_image_name(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("png")
        && path
            .file_stem()
            .and_then(|value| value.to_str())
            .and_then(|value| Uuid::parse_str(value).ok())
            .is_some()
}

#[cfg(unix)]
fn restrict_image_permissions(directory: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if file_type.is_file() && is_managed_image_name(&entry.path()) {
            match crate::storage::restrict_existing_file(&entry.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

struct CandidateSeed {
    title: String,
    source_url: String,
    image_url: Option<String>,
    thumbnail_url: Option<String>,
}

fn parse_search_response(
    provider: SearchProvider,
    bytes: &[u8],
) -> Result<Vec<CandidateSeed>, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| "検索サービスの応答を読み取れませんでした。".to_owned())?;
    let results = value
        .get("results")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "検索サービスの応答形式が正しくありません。".to_owned())?;
    let limit = if provider == SearchProvider::Brave {
        30
    } else {
        10
    };
    Ok(results
        .iter()
        .take(limit)
        .filter_map(|item| {
            let source_url = item.get("url")?.as_str()?.to_owned();
            validate_public_url(&source_url).ok()?;
            let image_url = if provider == SearchProvider::Brave {
                let image = item.pointer("/properties/url")?.as_str()?;
                validate_public_url(image).ok()?;
                Some(image.to_owned())
            } else {
                None
            };
            Some(CandidateSeed {
                title: item
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("画像")
                    .to_owned(),
                source_url,
                image_url,
                thumbnail_url: item
                    .pointer("/thumbnail/src")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect())
}

async fn prepare_candidate(
    transport: Arc<dyn ImageTransport>,
    decode_gate: Arc<Mutex<()>>,
    buffer_slots: Arc<Semaphore>,
    seed: CandidateSeed,
) -> Result<ImageCandidate, String> {
    let buffer_permit = buffer_slots
        .acquire_owned()
        .await
        .map_err(|_| "画像処理を続けられません。アプリを再起動してください。".to_owned())?;
    let (image_url, source_url) = if let Some(image_url) = seed.image_url {
        (image_url, seed.source_url)
    } else {
        let page = transport.fetch(&seed.source_url, MAX_PAGE_BYTES).await?;
        let image_url = representative_image(&page.bytes, &page.final_url)
            .ok_or_else(|| "ページに代表画像がありません。".to_owned())?;
        (image_url, page.final_url)
    };
    let thumbnail_url = seed
        .thumbnail_url
        .filter(|url| validate_public_url(url).is_ok())
        .unwrap_or_else(|| image_url.clone());
    let resource = transport.fetch(&thumbnail_url, MAX_IMAGE_BYTES).await?;
    let thumbnail = tokio::task::spawn_blocking(move || {
        // A cancelled search cannot release capacity while its blocking decoder owns bytes.
        let _buffer_permit = buffer_permit;
        let bytes = resource.bytes;
        let _permit = decode_gate
            .lock()
            .map_err(|_| "画像処理を続けられません。アプリを再起動してください。".to_owned())?;
        normalize_image(&bytes, 320)
    })
    .await
    .map_err(|_| "画像のプレビューを作成できませんでした。".to_owned())??;
    Ok(ImageCandidate {
        id: image_url,
        title: seed.title,
        preview_url: format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(thumbnail)
        ),
        source_url,
    })
}

fn representative_image(bytes: &[u8], page_url: &str) -> Option<String> {
    let base = validate_public_url(page_url).ok()?;
    let html = Html::parse_document(&String::from_utf8_lossy(bytes));
    let selector = Selector::parse("meta").ok()?;
    for property in ["og:image", "twitter:image"] {
        for element in html.select(&selector) {
            let attributes = element.value();
            if attributes
                .attr("property")
                .or_else(|| attributes.attr("name"))
                != Some(property)
            {
                continue;
            }
            let Some(content) = attributes.attr("content") else {
                continue;
            };
            let Ok(url) = base.join(content.trim()) else {
                continue;
            };
            if validate_public_url(url.as_str()).is_ok() {
                return Some(url.to_string());
            }
        }
    }
    None
}

fn normalize_image(bytes: &[u8], max_edge: u32) -> Result<Vec<u8>, String> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("20MiB以下の画像ファイルを選択してください。".to_owned());
    }
    let format = image::guess_format(bytes)
        .map_err(|_| "画像を読み取れませんでした。PNG・JPEG・WebPに対応しています。".to_owned())?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return Err("PNG・JPEG・WebPの画像を選択してください。".to_owned());
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(12_000);
    limits.max_image_height = Some(12_000);
    limits.max_alloc = Some(128 * 1024 * 1024);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits.clone());
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| "画像サイズを読み取れませんでした。".to_owned())?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err("画像の画素数が上限（3,200万画素）を超えています。".to_owned());
    }
    let orientation = decoder.orientation().map_err(|_| {
        "画像を読み取れませんでした。画像が破損していないか確認してください。".to_owned()
    })?;
    drop(decoder);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    let mut decoded = reader.decode().map_err(|_| {
        "画像を読み取れませんでした。画像が破損していないか確認してください。".to_owned()
    })?;
    decoded.apply_orientation(orientation);
    let normalized = if decoded.width() > max_edge || decoded.height() > max_edge {
        decoded.thumbnail(max_edge, max_edge)
    } else {
        decoded
    };
    let mut encoded = Cursor::new(Vec::new());
    normalized
        .write_to(&mut encoded, ImageFormat::Png)
        .map_err(|_| "画像を保存形式に変換できませんでした。".to_owned())?;
    Ok(encoded.into_inner())
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> std::io::Result<()> {
    std::fs::File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> std::io::Result<()> {
    // Windows does not support Unix directory fsync; the image file is still flushed.
    Ok(())
}

fn save_image(
    directory: &Path,
    bytes: &[u8],
    source_url: Option<String>,
) -> Result<ImageAsset, String> {
    save_image_with(directory, bytes, source_url, |_| {})
}

fn save_image_with(
    directory: &Path,
    bytes: &[u8],
    source_url: Option<String>,
    mut checkpoint: impl FnMut(bool),
) -> Result<ImageAsset, String> {
    let normalized = normalize_image(bytes, 1600)?;
    // All creation, synchronization and rollback stay relative to this open directory.
    let pinned = open_image_directory(directory)
        .map_err(|_| "画像の保存先を開けませんでした。".to_owned())?;
    let directory_handle = pinned
        .try_clone()
        .map_err(|_| "画像の保存先を開けませんでした。".to_owned())?
        .into_std_file();
    let identity = same_file::Handle::from_file(
        directory_handle
            .try_clone()
            .map_err(|_| "画像の保存先を開けませんでした。".to_owned())?,
    )
    .map_err(|_| "画像の保存先を開けませんでした。".to_owned())?;
    let unchanged = || -> std::io::Result<bool> {
        Ok(std::fs::symlink_metadata(directory)?.is_dir()
            && directory.canonicalize()? == directory
            && same_file::Handle::from_path(directory)? == identity)
    };
    if !unchanged().unwrap_or(false) {
        return Err("画像の保存先が変更されています。アプリを再起動してください。".to_owned());
    }
    checkpoint(false);
    let filename = format!("{}.png", Uuid::new_v4());
    let path = directory.join(&filename);
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = pinned
        .open_with(&filename, &options)
        .map(|file| file.into_std())
        .map_err(|_| "画像の保存先を作成できませんでした。".to_owned())?;
    checkpoint(true);
    let sync = || -> std::io::Result<()> {
        #[cfg(unix)]
        directory_handle.sync_all()?;
        Ok(())
    };
    let mut write = || -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(&normalized)?;
        file.sync_all()?;
        sync()?;
        let saved = same_file::Handle::from_file(file.try_clone()?)?;
        if !unchanged()? || same_file::Handle::from_path(&path)? != saved {
            return Err(std::io::Error::other(
                "image destination changed during save",
            ));
        }
        Ok(())
    };
    if write().is_err() {
        drop(file);
        let _ = pinned.remove_file(&filename);
        let _ = sync();
        return Err("画像ファイルを保存できませんでした。空き容量を確認してください。".to_owned());
    }
    Ok(ImageAsset {
        path: filename,
        source_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[cfg(unix)]
    #[test]
    fn cleanup_uses_the_validated_directory_when_its_path_is_replaced_before_unlink() {
        let directory = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let service = ImageService::new(directory.path().to_owned()).unwrap();
        let source = directory.path().join("source.png");
        std::fs::write(&source, png(2, 2)).unwrap();
        let image = service.import_local(source.clone()).unwrap();
        let external_image = external.path().join(&image.path);
        std::fs::write(&external_image, b"external image").unwrap();
        let previous = directory.path().join("previous-images");
        service.remove_managed_file_with(&image.path, || {
            std::fs::rename(&service.directory, &previous).unwrap();
            std::os::unix::fs::symlink(external.path(), &service.directory).unwrap();
        });
        assert_eq!(std::fs::read(external_image).unwrap(), b"external image");
        assert!(!previous.join(image.path).exists());
        assert_eq!(std::fs::read(source).unwrap(), png(2, 2));
    }

    #[cfg(unix)]
    #[test]
    fn startup_reconciliation_rejects_a_directory_replaced_before_opening_it() {
        let directory = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let service = ImageService::new(directory.path().to_owned()).unwrap();
        let canonical = service.directory.clone();
        let external_file = external.path().join(format!("{}.png", Uuid::new_v4()));
        std::fs::write(&external_file, b"external image").unwrap();
        std::fs::rename(&canonical, directory.path().join("previous-images")).unwrap();
        std::os::unix::fs::symlink(external.path(), &canonical).unwrap();
        assert!(reconcile_images(&canonical, &HashSet::new()).is_err());
        assert_eq!(std::fs::read(external_file).unwrap(), b"external image");
    }

    #[test]
    fn startup_reconciles_interrupted_imports_and_preserves_referenced_and_unmanaged_files() {
        let directory = tempfile::tempdir().unwrap();
        let mut database =
            crate::database::Database::open(&directory.path().join("test.sqlite3")).unwrap();
        let source = directory.path().join("source.png");
        std::fs::write(&source, png(2, 2)).unwrap();
        let service = ImageService::open(directory.path().to_owned(), &database).unwrap();
        let current = service.import_local(source.clone()).unwrap();
        let mut legacy = service.import_local(source.clone()).unwrap();
        let orphan = service.import_local(source.clone()).unwrap();
        let current_path = service.directory.join(&current.path);
        let legacy_path = service.directory.join(&legacy.path);
        let orphan_path = service.directory.join(&orphan.path);
        legacy.path = legacy_path.to_str().unwrap().to_owned();
        let unrelated = service.directory.join("user-picture.png");
        std::fs::write(&unrelated, b"unmanaged").unwrap();
        let subdirectory = service.directory.join(format!("{}.png", Uuid::new_v4()));
        std::fs::create_dir(&subdirectory).unwrap();
        #[cfg(unix)]
        let link = {
            let link = service.directory.join(format!("{}.png", Uuid::new_v4()));
            std::os::unix::fs::symlink(&source, &link).unwrap();
            link
        };
        let list = database.create_list("Images".into()).unwrap();
        let list = database
            .add_items(list.id, vec!["Current".into(), "Legacy".into()])
            .unwrap();
        database
            .set_image(list.id, list.items[0].id, Some(current))
            .unwrap();
        database
            .set_image(list.id, list.items[1].id, Some(legacy))
            .unwrap();
        drop(service); // The PNG was flushed but its association never committed.

        let _reopened = ImageService::open(directory.path().to_owned(), &database).unwrap();
        assert!(!orphan_path.exists());
        assert!(current_path.exists());
        assert!(legacy_path.exists());
        assert!(subdirectory.is_dir());
        #[cfg(unix)]
        assert!(std::fs::symlink_metadata(link).unwrap().is_symlink());
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"unmanaged");
        assert_eq!(std::fs::read(source).unwrap(), png(2, 2));
    }

    #[test]
    fn startup_defers_reconciliation_while_another_instance_can_associate_images() {
        let directory = tempfile::tempdir().unwrap();
        let mut database =
            crate::database::Database::open(&directory.path().join("test.sqlite3")).unwrap();
        let source = directory.path().join("source.png");
        std::fs::write(&source, png(2, 2)).unwrap();
        let first = ImageService::open(directory.path().to_owned(), &database).unwrap();
        let pending = first.import_local(source.clone()).unwrap();
        let orphan = first.import_local(source).unwrap();
        let pending_path = first.directory.join(&pending.path);
        let orphan_path = first.directory.join(&orphan.path);
        let second = ImageService::open(directory.path().to_owned(), &database).unwrap();
        assert!(pending_path.exists());
        assert!(orphan_path.exists());
        let list = database.create_list("Images".into()).unwrap();
        let list = database.add_items(list.id, vec!["Item".into()]).unwrap();
        database
            .set_image(list.id, list.items[0].id, Some(pending))
            .unwrap();
        drop(first);
        drop(second);

        let _reopened = ImageService::open(directory.path().to_owned(), &database).unwrap();
        assert!(pending_path.exists());
        assert!(!orphan_path.exists());
    }

    #[test]
    fn startup_preserves_all_images_when_database_references_cannot_be_read() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("test.sqlite3");
        let database = crate::database::Database::open(&database_path).unwrap();
        let service = ImageService::open(directory.path().to_owned(), &database).unwrap();
        let source = directory.path().join("source.png");
        std::fs::write(&source, png(2, 2)).unwrap();
        let orphan = service.import_local(source).unwrap();
        let orphan_path = service.directory.join(orphan.path);
        drop(service);
        rusqlite::Connection::open(database_path)
            .unwrap()
            .execute_batch("DROP TABLE items")
            .unwrap();
        assert!(ImageService::open(directory.path().to_owned(), &database).is_err());
        assert!(orphan_path.exists());
    }

    #[test]
    fn saved_image_reference_is_independent_of_the_storage_directory() {
        let (directory, service, _) = service(Ok(Vec::new()), vec![]);
        let source = directory.path().join("source.png");
        std::fs::write(&source, png(4, 2)).unwrap();
        let image = service.import_local(source).unwrap();
        assert_eq!(Path::new(&image.path).components().count(), 1);
        assert!(service.directory.join(&image.path).is_file());
        service.remove_managed_file(&image.path);
        assert!(!service.directory.join(&image.path).exists());
    }

    #[test]
    fn managed_image_protocol_serves_imported_pngs_and_rejects_other_paths() {
        use tauri::http::{Request, StatusCode};
        let (directory, service, _) = service(Ok(Vec::new()), vec![]);
        let source = directory.path().join("source.png");
        std::fs::write(&source, png(4, 2)).unwrap();
        let image = service.import_local(source).unwrap();
        for origin in [
            "pairrank-image://localhost",
            "http://pairrank-image.localhost",
        ] {
            let request = Request::builder()
                .uri(format!("{origin}/{}", image.path))
                .body(Vec::new())
                .unwrap();
            let response = service.image_response(&request);
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["content-type"], "image/png");
            assert_eq!(image::load_from_memory(response.body()).unwrap().width(), 4);
        }
        for reference in [
            "source.png",
            "../source.png",
            "%2e%2e%2fsource.png",
            "/source.png",
        ] {
            let request = Request::builder()
                .uri(format!("pairrank-image://localhost/{reference}"))
                .body(Vec::new())
                .unwrap();
            assert_eq!(
                service.image_response(&request).status(),
                StatusCode::FORBIDDEN
            );
        }
        let uri = format!("pairrank-image://localhost/{}", image.path);
        let head = Request::builder()
            .method("HEAD")
            .uri(&uri)
            .body(Vec::new())
            .unwrap();
        let response = service.image_response(&head);
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.body().is_empty());
        assert!(
            response.headers()["content-length"]
                .to_str()
                .unwrap()
                .parse::<usize>()
                .unwrap()
                > 0
        );
        let post = Request::builder()
            .method("POST")
            .uri(&uri)
            .body(Vec::new())
            .unwrap();
        assert_eq!(
            service.image_response(&post).status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        std::fs::OpenOptions::new()
            .write(true)
            .open(service.directory.join(&image.path))
            .unwrap()
            .set_len(MAX_IMAGE_BYTES as u64 + 1)
            .unwrap();
        let too_large = Request::builder().uri(&uri).body(Vec::new()).unwrap();
        assert_eq!(
            service.image_response(&too_large).status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        service.remove_managed_file(&image.path);
        let missing = Request::builder().uri(uri).body(Vec::new()).unwrap();
        assert_eq!(
            service.image_response(&missing).status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn cleanup_still_accepts_legacy_absolute_managed_references() {
        let (directory, service, _) = service(Ok(Vec::new()), vec![]);
        let source = directory.path().join("source.png");
        std::fs::write(&source, png(4, 2)).unwrap();
        let image = service.import_local(source.clone()).unwrap();
        let legacy = service.directory.join(&image.path);
        service.remove_managed_file(legacy.to_str().unwrap());
        assert!(!legacy.exists());
        assert!(source.exists());
    }

    #[cfg(unix)]
    #[test]
    fn imports_reject_directory_replacement_between_validation_creation_and_write() {
        for replace_after_creation in [false, true] {
            let app_data = tempfile::tempdir().unwrap();
            let external = tempfile::tempdir().unwrap();
            let service = ImageService::new(app_data.path().to_owned()).unwrap();
            let source = app_data.path().join("source.png");
            let original = png(4, 2);
            std::fs::write(&source, &original).unwrap();
            let previous = service.import_local(source.clone()).unwrap();
            let previous_directory = app_data.path().join("old-images");
            let result = save_image_with(&service.directory, &original, None, |created| {
                if created == replace_after_creation {
                    std::fs::rename(&service.directory, &previous_directory).unwrap();
                    std::os::unix::fs::symlink(external.path(), &service.directory).unwrap();
                }
            });
            assert!(
                result.is_err(),
                "a changed save directory must not produce a committed reference"
            );
            assert_eq!(std::fs::read_dir(external.path()).unwrap().count(), 0);
            assert_eq!(std::fs::read_dir(&previous_directory).unwrap().count(), 1);
            assert!(previous_directory.join(previous.path).is_file());
            assert_eq!(std::fs::read(source).unwrap(), original);
        }
    }

    #[cfg(unix)]
    #[test]
    fn import_rejects_a_replaced_storage_directory_without_writing_outside_it() {
        let app_data = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let service = ImageService::new(app_data.path().to_owned()).unwrap();
        let source = app_data.path().join("source.png");
        let original = png(4, 2);
        std::fs::write(&source, &original).unwrap();
        let previous = service.import_local(source.clone()).unwrap();
        let previous_directory = app_data.path().join("old-images");
        std::fs::rename(&service.directory, &previous_directory).unwrap();
        std::os::unix::fs::symlink(external.path(), &service.directory).unwrap();

        assert!(service.import_local(source.clone()).is_err());
        assert_eq!(std::fs::read_dir(external.path()).unwrap().count(), 0);
        assert!(previous_directory.join(previous.path).is_file());
        assert_eq!(std::fs::read(source).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn managed_image_protocol_does_not_serve_symlinks_or_a_replaced_storage_directory() {
        use tauri::http::{Request, StatusCode};
        let app_data = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let service = ImageService::new(app_data.path().to_owned()).unwrap();
        let filename = format!("{}.png", Uuid::new_v4());
        let source = external.path().join(&filename);
        std::fs::write(&source, png(4, 2)).unwrap();
        let request = Request::builder()
            .uri(format!("pairrank-image://localhost/{filename}"))
            .body(Vec::new())
            .unwrap();
        std::os::unix::fs::symlink(&source, service.directory.join(&filename)).unwrap();
        assert_eq!(
            service.image_response(&request).status(),
            StatusCode::FORBIDDEN
        );
        std::fs::rename(&service.directory, app_data.path().join("old-images")).unwrap();
        std::os::unix::fs::symlink(external.path(), &service.directory).unwrap();
        assert_eq!(
            service.image_response(&request).status(),
            StatusCode::FORBIDDEN
        );
        assert!(source.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn image_references_round_trip_and_delete_inside_non_utf8_storage() {
        use std::os::unix::ffi::OsStringExt;

        let root = tempfile::tempdir().unwrap();
        let app_data = root
            .path()
            .join(std::ffi::OsString::from_vec(b"app-\xff".to_vec()));
        let source = root.path().join("source.png");
        let original = png(4, 2);
        std::fs::write(&source, &original).unwrap();
        let service = ImageService::new(app_data).unwrap();
        let imported = service.import_local(source.clone()).unwrap();
        let saved: ImageAsset =
            serde_json::from_str(&serde_json::to_string(&imported).unwrap()).unwrap();
        assert!(
            saved.path.is_ascii(),
            "the stored reference must not encode the native directory"
        );
        assert!(service.directory.join(&saved.path).is_file());
        let request = tauri::http::Request::builder()
            .uri(format!("pairrank-image://localhost/{}", saved.path))
            .body(Vec::new())
            .unwrap();
        let response = service.image_response(&request);
        assert_eq!(response.status(), tauri::http::StatusCode::OK);
        assert_eq!(image::load_from_memory(response.body()).unwrap().width(), 4);
        service.remove_managed_file(&saved.path);
        assert_eq!(std::fs::read_dir(&service.directory).unwrap().count(), 0);
        assert_eq!(std::fs::read(source).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn newly_imported_images_are_private_and_source_permissions_are_unchanged() {
        use std::os::unix::fs::PermissionsExt;

        let app_data = tempfile::tempdir().unwrap();
        let source = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(source.path(), png(4, 2)).unwrap();
        std::fs::set_permissions(source.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        let service = ImageService::new(app_data.path().to_owned()).unwrap();
        let image = service.import_local(source.path().to_owned()).unwrap();
        assert_eq!(
            std::fs::metadata(&service.directory)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(service.directory.join(&image.path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(source.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }

    #[cfg(unix)]
    #[test]
    fn reopening_image_storage_restricts_only_existing_managed_files() {
        use std::os::unix::fs::PermissionsExt;

        let app_data = tempfile::tempdir().unwrap();
        let images = app_data.path().join("images");
        std::fs::create_dir(&images).unwrap();
        std::fs::set_permissions(&images, std::fs::Permissions::from_mode(0o755)).unwrap();
        let managed = images.join(format!("{}.png", Uuid::new_v4()));
        let source = app_data.path().join("source.png");
        let unrelated = images.join("unmanaged.png");
        for path in [&managed, &source, &unrelated] {
            std::fs::write(path, b"unchanged image").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        std::os::unix::fs::symlink(&source, images.join(format!("{}.png", Uuid::new_v4())))
            .unwrap();
        ImageService::new(app_data.path().to_owned()).unwrap();
        assert_eq!(
            std::fs::metadata(&managed).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&images).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for path in [&source, &unrelated] {
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o644
            );
        }
        assert_eq!(std::fs::read(managed).unwrap(), b"unchanged image");
    }

    #[derive(Default)]
    struct MemoryCredentials(Mutex<HashMap<SearchProvider, String>>);

    impl CredentialStore for MemoryCredentials {
        fn get(&self, provider: SearchProvider) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().get(&provider).cloned())
        }

        fn set(&self, provider: SearchProvider, key: &str) -> Result<(), String> {
            let mut keys = self.0.lock().unwrap();
            if key.is_empty() {
                keys.remove(&provider);
            } else {
                keys.insert(provider, key.to_owned());
            }
            Ok(())
        }
    }

    struct MockTransport {
        response: Result<Vec<u8>, String>,
        resources: HashMap<String, Resource>,
        searches: Mutex<Vec<(SearchProvider, String)>>,
        fetches: Mutex<Vec<String>>,
    }

    impl ImageTransport for MockTransport {
        fn search<'a>(
            &'a self,
            provider: SearchProvider,
            _key: &'a str,
            query: &'a str,
        ) -> NetworkFuture<'a, Vec<u8>> {
            self.searches
                .lock()
                .unwrap()
                .push((provider, query.to_owned()));
            Box::pin(async { self.response.clone() })
        }

        fn fetch<'a>(&'a self, url: &'a str, limit: usize) -> NetworkFuture<'a, Resource> {
            Box::pin(async move {
                self.fetches.lock().unwrap().push(url.to_owned());
                validate_public_url(url)?;
                match self.resources.get(url) {
                    Some(resource) if resource.bytes.len() <= limit => Ok(resource.clone()),
                    _ => Err("mock image unavailable".to_owned()),
                }
            })
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::DynamicImage::new_rgb8(width, height);
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    fn resource(url: &str, bytes: Vec<u8>) -> (String, Resource) {
        (
            url.to_owned(),
            Resource {
                bytes,
                final_url: url.to_owned(),
            },
        )
    }

    fn service(
        response: Result<Vec<u8>, String>,
        resources: Vec<(String, Resource)>,
    ) -> (tempfile::TempDir, ImageService, Arc<MockTransport>) {
        let directory = tempfile::tempdir().unwrap();
        let transport = Arc::new(MockTransport {
            response,
            resources: resources.into_iter().collect(),
            searches: Mutex::default(),
            fetches: Mutex::default(),
        });
        let service = ImageService {
            directory: directory.path().canonicalize().unwrap(),
            storage_lease: Arc::new(tempfile::tempfile().unwrap()),
            credentials: Arc::new(MemoryCredentials::default()),
            transport: transport.clone(),
            decode_gate: Arc::new(Mutex::new(())),
            buffer_slots: Arc::new(Semaphore::new(MAX_BUFFERED_IMAGES)),
            search_slots: Semaphore::new(MAX_ACTIVE_SEARCHES),
            settings_slots: Arc::new(Semaphore::new(1)),
        };
        (directory, service, transport)
    }

    #[tokio::test]
    async fn cancelled_settings_reads_keep_capacity_until_the_keyring_worker_finishes() {
        struct BlockingCredentials {
            started: tokio::sync::mpsc::UnboundedSender<()>,
            release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
        }
        impl CredentialStore for BlockingCredentials {
            fn get(&self, _provider: SearchProvider) -> Result<Option<String>, String> {
                let wait = self.release.lock().unwrap().take();
                if let Some(wait) = wait {
                    self.started.send(()).unwrap();
                    let _ = wait.recv();
                }
                Ok(None)
            }
            fn set(&self, _provider: SearchProvider, _key: &str) -> Result<(), String> {
                Ok(())
            }
        }
        let (_directory, mut service, _) = service(Ok(Vec::new()), vec![]);
        let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
        let (release, wait) = std::sync::mpsc::channel();
        service.credentials = Arc::new(BlockingCredentials {
            started,
            release: Mutex::new(Some(wait)),
        });
        let service = Arc::new(service);
        let first = tokio::spawn({
            let service = service.clone();
            async move { service.settings().await }
        });
        tokio::time::timeout(Duration::from_secs(2), starts.recv())
            .await
            .unwrap()
            .unwrap();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        let extra = tokio::time::timeout(Duration::from_millis(100), service.settings()).await;
        drop(release); // Also releases the worker if any later assertion fails.
        assert!(extra.is_ok_and(|result| result.is_err_and(|message| message.contains("実行中"))));
        let settings = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(settings) = service.settings().await {
                    break settings;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!settings.brave_configured && !settings.ollama_configured);
    }

    #[tokio::test]
    async fn settings_preserve_the_healthy_provider_when_the_other_key_cannot_be_read() {
        struct PartlyReadableCredentials(SearchProvider);
        impl CredentialStore for PartlyReadableCredentials {
            fn get(&self, provider: SearchProvider) -> Result<Option<String>, String> {
                if provider == self.0 {
                    Err("keyring read failed".into())
                } else {
                    Ok(Some("healthy-secret".into()))
                }
            }
            fn set(&self, _provider: SearchProvider, _key: &str) -> Result<(), String> {
                Ok(())
            }
        }
        for unreadable in [SearchProvider::Brave, SearchProvider::Ollama] {
            let (_directory, mut service, _) = service(Ok(br#"{"results":[]}"#.to_vec()), vec![]);
            service.credentials = Arc::new(PartlyReadableCredentials(unreadable));
            let settings = service.settings().await.unwrap();
            let healthy = if unreadable == SearchProvider::Brave {
                SearchProvider::Ollama
            } else {
                SearchProvider::Brave
            };
            assert_eq!(settings.brave_configured, healthy == SearchProvider::Brave);
            assert_eq!(
                settings.ollama_configured,
                healthy == SearchProvider::Ollama
            );
            assert_eq!(settings.default_provider, healthy);
            assert_eq!(
                settings.errors,
                HashMap::from([(unreadable, "keyring read failed".to_owned())])
            );
            assert!(
                !serde_json::to_string(&settings)
                    .unwrap()
                    .contains("healthy-secret")
            );
            assert!(service.search(healthy, "image".into()).await.is_ok());
        }
    }

    #[tokio::test]
    async fn successful_credential_mutations_do_not_depend_on_follow_up_keyring_reads() {
        struct UnreadableCredentials(MemoryCredentials);
        impl CredentialStore for UnreadableCredentials {
            fn get(&self, _provider: SearchProvider) -> Result<Option<String>, String> {
                Err("keyring read failed".to_owned())
            }

            fn set(&self, provider: SearchProvider, key: &str) -> Result<(), String> {
                self.0.set(provider, key)
            }
        }
        let (_directory, mut service, _) = service(Ok(Vec::new()), vec![]);
        let credentials = Arc::new(UnreadableCredentials(MemoryCredentials::default()));
        service.credentials = credentials.clone();
        for provider in [SearchProvider::Brave, SearchProvider::Ollama] {
            assert!(
                service
                    .set_api_key(provider, "saved-key".to_owned())
                    .is_ok()
            );
            assert_eq!(
                credentials.0.get(provider).unwrap().as_deref(),
                Some("saved-key")
            );
            assert!(service.set_api_key(provider, String::new()).is_ok());
            assert_eq!(credentials.0.get(provider).unwrap(), None);
        }
        let settings = service.settings().await.unwrap();
        assert!(!settings.brave_configured && !settings.ollama_configured);
        assert_eq!(
            settings.errors,
            HashMap::from([
                (SearchProvider::Brave, "keyring read failed".to_owned()),
                (SearchProvider::Ollama, "keyring read failed".to_owned()),
            ])
        );
    }

    #[tokio::test]
    async fn credentials_can_be_saved_switched_and_removed_without_returning_secrets() {
        let (_directory, service, _) = service(Ok(Vec::new()), vec![]);
        let settings = service.settings().await.unwrap();
        assert!(!settings.brave_configured && !settings.ollama_configured);
        assert_eq!(settings.default_provider, SearchProvider::Brave);
        service
            .set_api_key(SearchProvider::Ollama, "ollama-secret".to_owned())
            .unwrap();
        let settings = service.settings().await.unwrap();
        assert_eq!(settings.default_provider, SearchProvider::Ollama);
        service
            .set_api_key(SearchProvider::Brave, "brave-secret".to_owned())
            .unwrap();
        let settings = service.settings().await.unwrap();
        assert_eq!(settings.default_provider, SearchProvider::Brave);
        let serialized = serde_json::to_string(&settings).unwrap();
        assert!(!serialized.contains("secret"));
        assert!(serialized.contains("braveConfigured"));
        service
            .set_api_key(SearchProvider::Brave, String::new())
            .unwrap();
        let settings = service.settings().await.unwrap();
        assert!(!settings.brave_configured);
        assert_eq!(settings.default_provider, SearchProvider::Ollama);
        assert!(
            service
                .set_api_key(SearchProvider::Brave, "key\ninjected".to_owned())
                .is_err()
        );
    }

    #[test]
    fn provider_requests_follow_the_api_contract_and_keep_credentials_in_headers() {
        let client = client_builder().build().unwrap();
        let brave =
            search_request(&client, SearchProvider::Brave, "test-secret", "猫 & 犬").unwrap();
        assert_eq!(brave.method(), reqwest::Method::GET);
        assert_eq!(brave.url().host_str(), Some("api.search.brave.com"));
        assert_eq!(brave.url().path(), "/res/v1/images/search");
        let query: HashMap<_, _> = brave.url().query_pairs().into_owned().collect();
        assert_eq!(query.get("q").unwrap(), "猫 & 犬");
        assert_eq!(query.get("count").unwrap(), "30");
        assert_eq!(query.get("safesearch").unwrap(), "strict");
        assert_eq!(
            brave.headers().get("x-subscription-token").unwrap(),
            "test-secret"
        );
        assert!(!brave.url().as_str().contains("test-secret"));
        let ollama = search_request(&client, SearchProvider::Ollama, "test-secret", "猫").unwrap();
        assert_eq!(ollama.method(), reqwest::Method::POST);
        assert_eq!(ollama.url().as_str(), "https://ollama.com/api/web_search");
        let body: serde_json::Value =
            serde_json::from_slice(ollama.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body, serde_json::json!({"query": "猫", "max_results": 10}));
        assert_eq!(
            ollama.headers().get("authorization").unwrap(),
            "Bearer test-secret"
        );
        assert!(ollama.headers().get("x-subscription-token").is_none());
        assert!(brave.headers().get("authorization").is_none());
    }

    #[tokio::test]
    async fn brave_results_have_safe_previews_keep_order_and_skip_invalid_images() {
        let response = serde_json::json!({"results": [
            {"title": "First", "url": "https://example.com/first", "properties": {"url": "https://example.com/first.png"}, "thumbnail": {"src": "https://example.com/thumb.png"}},
            {"title": "Missing", "url": "https://example.com/missing", "properties": {"url": "https://example.com/missing.png"}},
            {"title": "Second", "url": "https://example.com/second", "properties": {"url": "https://example.com/second.png"}},
            {"title": "Private", "url": "https://example.com/private", "properties": {"url": "http://127.0.0.1/image"}},
            {"title": "Duplicate", "url": "https://example.com/first", "properties": {"url": "https://example.com/first.png"}, "thumbnail": {"src": "https://example.com/thumb.png"}}
        ]});
        let (_directory, service, transport) = service(
            Ok(serde_json::to_vec(&response).unwrap()),
            vec![
                resource("https://example.com/thumb.png", png(640, 320)),
                resource("https://example.com/second.png", png(8, 4)),
            ],
        );
        service
            .set_api_key(SearchProvider::Brave, "test-key".to_owned())
            .unwrap();
        let results = service
            .search(SearchProvider::Brave, "  猫  ".to_owned())
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "First");
        assert_eq!(results[1].title, "Second");
        assert_eq!(results[0].id, "https://example.com/first.png");
        let preview = results[0]
            .preview_url
            .strip_prefix("data:image/png;base64,")
            .unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(preview)
            .unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (320, 160));
        assert_eq!(
            *transport.searches.lock().unwrap(),
            vec![(SearchProvider::Brave, "猫".to_owned())]
        );
    }

    #[tokio::test]
    async fn ollama_reads_page_metadata_and_skips_inaccessible_pages() {
        let response = serde_json::json!({"results": [
            {"title":"Open Graph", "url":"https://example.com/first"},
            {"title":"Twitter", "url":"https://example.com/path/second"},
            {"title":"No image", "url":"https://example.com/empty"},
            {"title":"Unavailable", "url":"https://example.com/missing"}
        ]});
        let (_directory, service, _) = service(Ok(serde_json::to_vec(&response).unwrap()), vec![
            resource("https://example.com/first", br#"<meta name="twitter:image" content="/fallback.png"><meta property="og:image" content="/first.png">"#.to_vec()),
            resource("https://example.com/path/second", br#"<meta name="twitter:image" content="../second.png">"#.to_vec()),
            resource("https://example.com/empty", b"<p>No image</p>".to_vec()),
            resource("https://example.com/first.png", png(2, 2)),
            resource("https://example.com/second.png", png(2, 2)),
        ]);
        service
            .set_api_key(SearchProvider::Ollama, "test-key".to_owned())
            .unwrap();
        let results = service
            .search(SearchProvider::Ollama, "猫".to_owned())
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "https://example.com/first.png");
        assert_eq!(results[1].id, "https://example.com/second.png");
    }

    #[tokio::test]
    async fn missing_key_invalid_query_and_api_errors_do_not_switch_provider() {
        let (_directory, service, transport) = service(Err("API unavailable".to_owned()), vec![]);
        assert!(
            service
                .search(SearchProvider::Brave, "猫".to_owned())
                .await
                .unwrap_err()
                .contains("APIキー")
        );
        service
            .set_api_key(SearchProvider::Ollama, "test-key".to_owned())
            .unwrap();
        assert!(
            service
                .search(SearchProvider::Ollama, " ".to_owned())
                .await
                .is_err()
        );
        assert_eq!(
            service
                .search(SearchProvider::Ollama, "猫".to_owned())
                .await
                .unwrap_err(),
            "API unavailable"
        );
        assert_eq!(
            *transport.searches.lock().unwrap(),
            vec![(SearchProvider::Ollama, "猫".to_owned())]
        );
        assert!(
            check_search_status(StatusCode::UNAUTHORIZED)
                .unwrap_err()
                .contains("認証")
        );
        assert!(
            check_search_status(StatusCode::TOO_MANY_REQUESTS)
                .unwrap_err()
                .contains("利用上限")
        );
        assert!(check_search_status(StatusCode::FOUND).is_err());
    }

    #[test]
    fn url_and_redirect_checks_reject_private_networks_and_non_http_protocols() {
        for url in [
            "file:///etc/passwd",
            "data:image/png;base64,aA==",
            "http://localhost/image",
            "http://LOCALHOST./image",
            "http://service.local/image",
            "http://127.0.0.1/image",
            "http://2130706433/image",
            "http://10.0.0.1/image",
            "http://100.64.0.1/image",
            "http://169.254.169.254/latest",
            "http://172.16.0.1/image",
            "http://192.168.1.1/image",
            "http://[::1]/image",
            "http://[::ffff:127.0.0.1]/image",
            "http://[fc00::1]/image",
            "https://example.com:8443/image",
            "https://user:secret@example.com/image",
        ] {
            assert!(validate_public_url(url).is_err(), "accepted {url}");
        }
        let base = validate_public_url("https://example.com/images/item").unwrap();
        assert_eq!(
            redirected_url(&base, "../new.png").unwrap().as_str(),
            "https://example.com/new.png"
        );
        assert!(redirected_url(&base, "http://127.0.0.1/image").is_err());
        assert!(is_public_ip("8.8.8.8".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
        for address in [
            "198.18.0.1",
            "192.0.2.1",
            "203.0.113.1",
            "2001:db8::1",
            "2002:7f00:1::",
        ] {
            assert!(!is_public_ip(address.parse().unwrap()));
        }
    }

    #[test]
    fn metadata_extraction_resolves_urls_and_falls_back_from_unsafe_og_image() {
        let page = br#"<meta property="og:image" content="http://127.0.0.1/secret"><meta name="twitter:image" content="//cdn.example.com/a.png?x=1&amp;y=2">"#;
        assert_eq!(
            representative_image(page, "https://example.com/item").unwrap(),
            "https://cdn.example.com/a.png?x=1&y=2"
        );
        assert!(representative_image(b"<p>none</p>", "https://example.com/item").is_none());
    }

    #[test]
    fn local_import_is_normalized_and_never_changes_source_file() {
        let (directory, service, _) = service(Ok(Vec::new()), vec![]);
        let source = directory.path().join("source.png");
        let original = png(1800, 900);
        std::fs::write(&source, &original).unwrap();
        let asset = service.import_local(source.clone()).unwrap();
        assert_ne!(service.directory.join(&asset.path), source);
        assert!(asset.source_url.is_none());
        assert_eq!(std::fs::read(source).unwrap(), original);
        let normalized = image::open(service.directory.join(asset.path)).unwrap();
        assert_eq!((normalized.width(), normalized.height()), (1600, 800));
        let tiny = image::load_from_memory(&normalize_image(&png(2, 1), 320).unwrap()).unwrap();
        assert_eq!((tiny.width(), tiny.height()), (2, 1));
    }

    #[tokio::test]
    async fn remote_import_keeps_attribution_and_failures_preserve_existing_images() {
        let (directory, service, _) = service(
            Ok(Vec::new()),
            vec![resource("https://example.com/image.png", png(3, 2))],
        );
        let candidate = ImageCandidate {
            id: "https://example.com/image.png".to_owned(),
            title: "Image".to_owned(),
            preview_url: String::new(),
            source_url: "https://example.com/item".to_owned(),
        };
        let asset = service.import_remote(candidate.clone()).await.unwrap();
        assert_eq!(
            asset.source_url.as_deref(),
            Some("https://example.com/item")
        );
        assert!(service.directory.join(&asset.path).exists());
        let mut missing = candidate;
        missing.id = "https://example.com/missing.png".to_owned();
        assert!(service.import_remote(missing).await.is_err());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(service.directory.join(&asset.path).exists());
    }

    #[test]
    fn invalid_unsupported_and_oversized_images_are_rejected_without_creating_files() {
        let (directory, service, _) = service(Ok(Vec::new()), vec![]);
        assert!(save_image(&service.directory, b"<svg></svg>", None).is_err());
        assert!(normalize_image(b"GIF89a", 320).is_err());
        assert!(normalize_image(&vec![0; MAX_IMAGE_BYTES + 1], 320).is_err());
        assert!(
            service
                .import_local(directory.path().join("missing.png"))
                .is_err()
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        assert!(
            parse_search_response(SearchProvider::Brave, br#"{"error":"invalid key"}"#).is_err()
        );
        assert!(parse_search_response(SearchProvider::Brave, b"broken json").is_err());
    }
    #[tokio::test]
    async fn concurrent_search_limit_covers_completed_previews_until_the_whole_search_finishes() {
        struct SlowTailTransport {
            started: tokio::sync::mpsc::UnboundedSender<()>,
            release: Arc<Semaphore>,
        }
        impl ImageTransport for SlowTailTransport {
            fn search<'a>(
                &'a self,
                _: SearchProvider,
                _: &'a str,
                _: &'a str,
            ) -> NetworkFuture<'a, Vec<u8>> {
                Box::pin(async {
                    Ok(serde_json::to_vec(&serde_json::json!({"results": [
                        {"url": "https://example.com/fast", "properties": {"url": "https://example.com/fast.png"}},
                        {"url": "https://example.com/slow", "properties": {"url": "https://example.com/slow.png"}}
                    ]})).unwrap())
                })
            }
            fn fetch<'a>(&'a self, url: &'a str, _: usize) -> NetworkFuture<'a, Resource> {
                Box::pin(async move {
                    self.started.send(()).unwrap();
                    if url.ends_with("slow.png") {
                        self.release.acquire().await.unwrap().forget();
                    }
                    Ok(Resource {
                        bytes: png(4, 2),
                        final_url: url.into(),
                    })
                })
            }
        }
        let (_directory, mut service, _) = service(Ok(Vec::new()), vec![]);
        let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
        let release = Arc::new(Semaphore::new(0));
        service.transport = Arc::new(SlowTailTransport {
            started,
            release: release.clone(),
        });
        service
            .set_api_key(SearchProvider::Brave, "test-key".into())
            .unwrap();
        let service = Arc::new(service);
        let mut pending = Vec::new();
        for _ in 0..2 {
            let service = service.clone();
            pending.push(tokio::spawn(async move {
                service.search(SearchProvider::Brave, "image".into()).await
            }));
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            for _ in 0..4 {
                starts.recv().await.unwrap();
            }
        })
        .await
        .unwrap();
        // The fast previews have released their encoded-buffer permits, while
        // the two slow candidates keep their searches (and completed outputs) alive.
        tokio::time::timeout(Duration::from_secs(2), async {
            while service.buffer_slots.available_permits() != MAX_BUFFERED_IMAGES - 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let extra = tokio::time::timeout(
            Duration::from_millis(100),
            service.search(SearchProvider::Brave, "another".into()),
        )
        .await;
        release.add_permits(10);
        for search in pending {
            assert_eq!(search.await.unwrap().unwrap().len(), 2);
        }
        assert!(
            extra.is_ok_and(|result| result.is_err_and(|message| message.contains("画像検索")))
        );
        assert_eq!(
            service
                .search(SearchProvider::Brave, "later".into())
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn shared_budgets_bound_fetch_buffers_and_decodes_across_searches_and_imports() {
        struct ParallelTransport {
            response: Vec<u8>,
            bytes: Vec<u8>,
            fetched: tokio::sync::mpsc::UnboundedSender<()>,
        }
        impl ImageTransport for ParallelTransport {
            fn search<'a>(
                &'a self,
                _: SearchProvider,
                _: &'a str,
                _: &'a str,
            ) -> NetworkFuture<'a, Vec<u8>> {
                Box::pin(async { Ok(self.response.clone()) })
            }
            fn fetch<'a>(&'a self, url: &'a str, _: usize) -> NetworkFuture<'a, Resource> {
                Box::pin(async move {
                    self.fetched.send(()).unwrap();
                    Ok(Resource {
                        bytes: self.bytes.clone(),
                        final_url: url.into(),
                    })
                })
            }
        }
        let (directory, mut service, _) = service(Ok(Vec::new()), vec![]);
        let (fetched, mut fetches) = tokio::sync::mpsc::unbounded_channel();
        service.transport = Arc::new(ParallelTransport {
            response: serde_json::to_vec(
                &serde_json::json!({ "results": (0..4).map(|n| serde_json::json!({
                "url": format!("https://example.com/{n}"),
                "properties": { "url": format!("https://example.com/{n}.png") }
            })).collect::<Vec<_>>() }),
            )
            .unwrap(),
            bytes: png(4, 2),
            fetched,
        });
        service
            .set_api_key(SearchProvider::Brave, "test-key".into())
            .unwrap();
        let source = directory.path().join("source.png");
        std::fs::write(&source, png(4, 2)).unwrap();
        let gate = service.decode_gate.clone();
        let (occupied, occupied_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let decoder = std::thread::spawn(move || {
            let _permit = gate.lock().unwrap();
            occupied.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        occupied_rx.recv().unwrap();
        let service = Arc::new(service);
        let mut searches = Vec::new();
        for _ in 0..2 {
            let service = service.clone();
            searches.push(tokio::spawn(async move {
                service.search(SearchProvider::Brave, "image".into()).await
            }));
        }
        let remote_service = service.clone();
        let remote = tokio::spawn(async move {
            remote_service
                .import_remote(ImageCandidate {
                    id: "https://example.com/import.png".into(),
                    title: "Image".into(),
                    preview_url: String::new(),
                    source_url: "https://example.com/import".into(),
                })
                .await
        });
        let local_service = service.clone();
        let local = tokio::task::spawn_blocking(move || local_service.import_local(source));
        let network_parallel = tokio::time::timeout(Duration::from_secs(2), async {
            for _ in 0..4 {
                fetches.recv().await.unwrap();
            }
        })
        .await
        .is_ok();
        let buffers_bounded = tokio::time::timeout(Duration::from_millis(100), fetches.recv())
            .await
            .is_err();
        let waited_for_budget = searches.iter().all(|search| !search.is_finished())
            && !remote.is_finished()
            && !local.is_finished();
        release.send(()).unwrap();
        decoder.join().unwrap();
        assert!(
            network_parallel,
            "up to four images must fetch in parallel while decoding is occupied"
        );
        assert!(
            buffers_bounded,
            "additional image bodies must not queue ahead of decoding"
        );
        assert!(
            waited_for_budget,
            "search previews and both imports must share the decode budget"
        );
        for search in searches {
            assert_eq!(search.await.unwrap().unwrap().len(), 4);
        }
        assert!(
            service
                .directory
                .join(remote.await.unwrap().unwrap().path)
                .exists()
        );
        assert!(
            service
                .directory
                .join(local.await.unwrap().unwrap().path)
                .exists()
        );
    }
    #[tokio::test]
    async fn cancelling_a_search_keeps_its_buffers_reserved_until_blocking_decodes_finish() {
        let urls: Vec<_> = (0..4)
            .map(|id| format!("https://example.com/{id}.png"))
            .collect();
        let response = serde_json::to_vec(&serde_json::json!({
            "results": urls.iter().map(|url| serde_json::json!({
                "url": url, "properties": { "url": url }
            })).collect::<Vec<_>>()
        }))
        .unwrap();
        let (_directory, service, transport) = service(
            Ok(response),
            urls.iter().map(|url| resource(url, png(4, 2))).collect(),
        );
        service
            .set_api_key(SearchProvider::Brave, "test-key".into())
            .unwrap();
        let gate = service.decode_gate.clone();
        let (occupied, occupied_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let decoder = std::thread::spawn(move || {
            let _permit = gate.lock().unwrap();
            occupied.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        occupied_rx.recv().unwrap();
        let service = Arc::new(service);
        let first_service = service.clone();
        let first = tokio::spawn(async move {
            first_service
                .search(SearchProvider::Brave, "old".into())
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while transport.fetches.lock().unwrap().len() < 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        let next =
            tokio::spawn(async move { service.search(SearchProvider::Brave, "new".into()).await });
        let bounded_after_cancel = tokio::time::timeout(Duration::from_millis(100), async {
            while transport.fetches.lock().unwrap().len() == 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_err();
        release.send(()).unwrap();
        decoder.join().unwrap();
        assert!(
            bounded_after_cancel,
            "cancelling the async caller must not free capacity still used by a blocking worker"
        );
        assert_eq!(next.await.unwrap().unwrap().len(), 4);
        assert_eq!(transport.fetches.lock().unwrap().len(), 8);
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_does_not_follow_a_replaced_images_directory_outside_app_data() {
        let app_data = tempfile::tempdir().unwrap();
        let service = ImageService::new(app_data.path().to_owned()).unwrap();
        let external = tempfile::tempdir().unwrap();
        let filename = format!("{}.png", Uuid::new_v4());
        let original = external.path().join(&filename);
        std::fs::write(&original, b"external image").unwrap();
        std::fs::remove_dir(&service.directory).unwrap();
        std::os::unix::fs::symlink(external.path(), &service.directory).unwrap();
        service.remove_managed_file(service.directory.join(filename).to_str().unwrap());
        assert_eq!(std::fs::read(original).unwrap(), b"external image");
    }

    fn jpeg_with_orientation(orientation: u8) -> Vec<u8> {
        let pixels = image::RgbImage::from_fn(32, 16, |x, y| {
            image::Rgb(match (x < 16, y < 8) {
                (true, true) => [255, 0, 0],
                (false, true) => [0, 255, 0],
                (true, false) => [0, 0, 255],
                (false, false) => [255, 255, 0],
            })
        });
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 100)
            .encode_image(&pixels)
            .unwrap();
        // APP1 Exif: little-endian TIFF with one SHORT Orientation (0x0112) entry.
        let mut oriented = vec![0xff, 0xd8, 0xff, 0xe1, 0, 34];
        oriented.extend_from_slice(b"Exif\0\0II\x2a\0\x08\0\0\0");
        oriented.extend_from_slice(b"\x01\0\x12\x01\x03\0\x01\0\0\0");
        oriented.extend_from_slice(&[orientation, 0, 0, 0, 0, 0, 0, 0]);
        oriented.extend_from_slice(&jpeg[2..]);
        oriented
    }

    #[test]
    fn local_jpeg_import_applies_exif_rotation_and_reflection_before_discarding_metadata() {
        let (directory, service, _) = service(Ok(Vec::new()), vec![]);
        for (orientation, dimensions, expected) in [
            (
                6,
                (16, 32),
                [[0, 0, 255], [255, 0, 0], [255, 255, 0], [0, 255, 0]],
            ),
            (
                2,
                (32, 16),
                [[0, 255, 0], [255, 0, 0], [255, 255, 0], [0, 0, 255]],
            ),
        ] {
            let source = directory.path().join(format!("oriented-{orientation}.jpg"));
            let bytes = jpeg_with_orientation(orientation);
            std::fs::write(&source, &bytes).unwrap();
            let imported = service.import_local(source.clone()).unwrap();
            let decoded = image::open(service.directory.join(imported.path))
                .unwrap()
                .to_rgb8();
            assert_eq!(decoded.dimensions(), dimensions);
            let (width, height) = dimensions;
            for ((x, y), color) in [
                (width / 4, height / 4),
                (3 * width / 4, height / 4),
                (width / 4, 3 * height / 4),
                (3 * width / 4, 3 * height / 4),
            ]
            .into_iter()
            .zip(expected)
            {
                let actual = decoded.get_pixel(x, y).0;
                assert!(
                    actual
                        .into_iter()
                        .zip(color)
                        .all(|(actual, expected)| actual.abs_diff(expected) < 20),
                    "orientation {orientation}: expected {color:?}, got {actual:?}"
                );
            }
            assert_eq!(std::fs::read(source).unwrap(), bytes);
        }
    }

    #[cfg(unix)]
    struct RestorePermissions {
        path: PathBuf,
        original: std::fs::Permissions,
    }

    #[cfg(unix)]
    impl RestorePermissions {
        fn write_only_directory(path: &Path) -> Option<Self> {
            use std::os::unix::fs::PermissionsExt;
            let original = std::fs::metadata(path).unwrap().permissions();
            let restore = Self {
                path: path.to_owned(),
                original,
            };
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o300)).unwrap();
            match std::fs::File::open(path) {
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Some(restore),
                Ok(_) => {
                    eprintln!(
                        "skipping directory permission-failure scenario: this user bypasses Unix mode bits"
                    );
                    None
                }
                Err(error) => panic!("unexpected directory permission error: {error}"),
            }
        }
    }

    #[cfg(unix)]
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.path, self.original.clone());
        }
    }

    #[cfg(unix)]
    #[test]
    fn import_rejects_unsynced_directory_entries_and_removes_only_the_new_file() {
        let (directory, service, _) = service(Ok(Vec::new()), vec![]);
        let source = tempfile::NamedTempFile::new().unwrap();
        let original = png(4, 2);
        std::fs::write(source.path(), &original).unwrap();
        let existing = service.import_local(source.path().to_owned()).unwrap();
        let Some(permissions) = RestorePermissions::write_only_directory(directory.path()) else {
            return;
        };
        let result = service.import_local(source.path().to_owned());
        drop(permissions);
        assert!(
            result.is_err(),
            "an import must fail before association if its directory entry cannot be synced"
        );
        assert!(service.directory.join(&existing.path).exists());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert_eq!(std::fs::read(source.path()).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn image_service_requires_the_initial_images_directory_entry_to_be_synced() {
        let app_data = tempfile::tempdir().unwrap();
        let Some(permissions) = RestorePermissions::write_only_directory(app_data.path()) else {
            return;
        };
        let result = ImageService::new(app_data.path().to_owned());
        drop(permissions);
        assert!(
            result.is_err(),
            "a newly created images directory must be synced in its parent before use"
        );
        assert!(ImageService::new(app_data.path().to_owned()).is_ok());
    }
}
