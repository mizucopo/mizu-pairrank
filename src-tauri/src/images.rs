use std::collections::HashSet;
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
    credentials: Arc<dyn CredentialStore>,
    transport: Arc<dyn ImageTransport>,
    decode_gate: Arc<Mutex<()>>,
}

impl ImageService {
    pub fn new(app_data: PathBuf) -> Result<Self, String> {
        let directory = app_data.join("images");
        std::fs::create_dir_all(&directory)
            .map_err(|_| "画像の保存フォルダーを作成できませんでした。".to_owned())?;
        sync_directory(&app_data)
            .map_err(|_| "画像の保存フォルダーを同期できませんでした。".to_owned())?;
        let directory = directory
            .canonicalize()
            .map_err(|_| "画像の保存フォルダーを開けませんでした。".to_owned())?;
        Ok(Self {
            directory,
            credentials: Arc::new(SystemCredentials),
            transport: Arc::new(HttpTransport),
            decode_gate: Arc::new(Mutex::new(())),
        })
    }

    pub fn remove_managed_file(&self, path: &str) {
        let path = Path::new(path);
        if path.parent() != Some(self.directory.as_path())
            || path.extension().and_then(|value| value.to_str()) != Some("png")
            || path
                .file_stem()
                .and_then(|value| value.to_str())
                .and_then(|value| Uuid::parse_str(value).ok())
                .is_none()
        {
            return;
        }
        if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
            && path
                .canonicalize()
                .is_ok_and(|resolved| resolved.parent() == Some(self.directory.as_path()))
        {
            // Cleanup must never turn a committed database operation into a failure.
            let _ = std::fs::remove_file(path);
        }
    }

    pub fn settings(&self) -> Result<SearchSettings, String> {
        let brave_configured = self.credentials.get(SearchProvider::Brave)?.is_some();
        let ollama_configured = self.credentials.get(SearchProvider::Ollama)?.is_some();
        Ok(SearchSettings {
            brave_configured,
            ollama_configured,
            default_provider: if ollama_configured && !brave_configured {
                SearchProvider::Ollama
            } else {
                SearchProvider::Brave
            },
        })
    }

    pub fn set_api_key(
        &self,
        provider: SearchProvider,
        key: String,
    ) -> Result<SearchSettings, String> {
        let key = key.trim();
        if key.len() > 4096 || key.chars().any(char::is_control) {
            return Err("APIキーの形式が正しくありません。".to_owned());
        }
        self.credentials.set(provider, key)?;
        self.settings()
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
                active.spawn(async move {
                    (
                        position,
                        prepare_candidate(transport, decode_gate, seed).await,
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
        let resource = self.transport.fetch(&candidate.id, MAX_IMAGE_BYTES).await?;
        let directory = self.directory.clone();
        let decode_gate = self.decode_gate.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = decode_gate
                .lock()
                .map_err(|_| "画像処理を続けられません。アプリを再起動してください。".to_owned())?;
            save_image(&directory, &resource.bytes, Some(candidate.source_url))
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
        let mut bytes = Vec::new();
        file.take(MAX_IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "選択した画像ファイルを読み取れませんでした。".to_owned())?;
        let _permit = self
            .decode_gate
            .lock()
            .map_err(|_| "画像処理を続けられません。アプリを再起動してください。".to_owned())?;
        save_image(&self.directory, &bytes, None)
    }
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
    seed: CandidateSeed,
) -> Result<ImageCandidate, String> {
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
        let _permit = decode_gate
            .lock()
            .map_err(|_| "画像処理を続けられません。アプリを再起動してください。".to_owned())?;
        normalize_image(&resource.bytes, 320)
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
    let normalized = normalize_image(bytes, 1600)?;
    let path = directory.join(format!("{}.png", Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|_| "画像の保存先を作成できませんでした。".to_owned())?;
    if file
        .write_all(&normalized)
        .and_then(|()| file.sync_all())
        .and_then(|()| sync_directory(directory))
        .is_err()
    {
        drop(file);
        let _ = std::fs::remove_file(&path);
        let _ = sync_directory(directory);
        return Err("画像ファイルを保存できませんでした。空き容量を確認してください。".to_owned());
    }
    Ok(ImageAsset {
        path: path.to_string_lossy().into_owned(),
        source_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

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
        });
        let service = ImageService {
            directory: directory.path().to_owned(),
            credentials: Arc::new(MemoryCredentials::default()),
            transport: transport.clone(),
            decode_gate: Arc::new(Mutex::new(())),
        };
        (directory, service, transport)
    }

    #[test]
    fn credentials_can_be_saved_switched_and_removed_without_returning_secrets() {
        let (_directory, service, _) = service(Ok(Vec::new()), vec![]);
        let settings = service.settings().unwrap();
        assert!(!settings.brave_configured && !settings.ollama_configured);
        assert_eq!(settings.default_provider, SearchProvider::Brave);
        let settings = service
            .set_api_key(SearchProvider::Ollama, "ollama-secret".to_owned())
            .unwrap();
        assert_eq!(settings.default_provider, SearchProvider::Ollama);
        let settings = service
            .set_api_key(SearchProvider::Brave, "brave-secret".to_owned())
            .unwrap();
        assert_eq!(settings.default_provider, SearchProvider::Brave);
        let serialized = serde_json::to_string(&settings).unwrap();
        assert!(!serialized.contains("secret"));
        assert!(serialized.contains("braveConfigured"));
        let settings = service
            .set_api_key(SearchProvider::Brave, String::new())
            .unwrap();
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
        assert_ne!(Path::new(&asset.path), source);
        assert!(asset.source_url.is_none());
        assert_eq!(std::fs::read(source).unwrap(), original);
        let normalized = image::open(asset.path).unwrap();
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
        assert!(Path::new(&asset.path).exists());
        let mut missing = candidate;
        missing.id = "https://example.com/missing.png".to_owned();
        assert!(service.import_remote(missing).await.is_err());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(Path::new(&asset.path).exists());
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
    async fn shared_decode_budget_bounds_searches_and_imports_without_serializing_network_fetches()
    {
        struct ParallelTransport {
            response: Vec<u8>,
            bytes: Vec<u8>,
            fetches: tokio::sync::Barrier,
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
                    self.fetches.wait().await;
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
            fetches: tokio::sync::Barrier::new(9),
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
            for _ in 0..9 {
                fetches.recv().await.unwrap();
            }
        })
        .await
        .is_ok();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let waited_for_budget = searches.iter().all(|search| !search.is_finished())
            && !remote.is_finished()
            && !local.is_finished();
        release.send(()).unwrap();
        decoder.join().unwrap();
        assert!(
            network_parallel,
            "both searches and import must fetch while decoding is occupied"
        );
        assert!(
            waited_for_budget,
            "search previews and both imports must share the decode budget"
        );
        for search in searches {
            assert_eq!(search.await.unwrap().unwrap().len(), 4);
        }
        assert!(Path::new(&remote.await.unwrap().unwrap().path).exists());
        assert!(Path::new(&local.await.unwrap().unwrap().path).exists());
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
            let decoded = image::open(imported.path).unwrap().to_rgb8();
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
        assert!(Path::new(&existing.path).exists());
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
