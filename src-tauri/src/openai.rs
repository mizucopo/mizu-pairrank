//! OpenAI web image search. Authentication is an explicit saved choice; no fallback.
//! https://developers.openai.com/api/docs/guides/tools-web-search
//! https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference

use crate::chatgpt::ChatGptClient;
use crate::credentials::{CredentialKind, CredentialStore};
use crate::images::{CandidateSeed, SearchProvider, validate_public_url};
use reqwest::{Client, Request, Response, StatusCode, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, future::Future, pin::Pin, sync::Arc, time::Duration};

const RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
const MODELS_URL: &str = "https://api.openai.com/v1/models";
const MAX_SETTINGS_BYTES: usize = 4096;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_MODELS_BYTES: usize = 2 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 64 * 1024;
const MAX_RESULTS: usize = 30;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OpenAiAuthMode {
    #[default]
    Api,
    Chatgpt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenAiSettings {
    pub(crate) model: String,
    pub(crate) auth_mode: OpenAiAuthMode,
}

impl Default for OpenAiSettings {
    fn default() -> Self {
        Self {
            model: "gpt-6-luna".to_owned(),
            auth_mode: OpenAiAuthMode::Api,
        }
    }
}

fn validate_model(model: &str) -> Result<(), String> {
    if model.is_empty()
        || model.len() > 128
        || model.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(
            "OpenAIのモデルIDは空白・制御文字を含まない128バイト以内の文字列で指定してください。"
                .to_owned(),
        );
    }
    Ok(())
}

/// Reads settings only. Missing settings use the default; corrupt settings are never replaced.
pub(crate) fn read_settings(credentials: &dyn CredentialStore) -> Result<OpenAiSettings, String> {
    let Some(record) = credentials.get(CredentialKind::OpenAiSettings)? else {
        return Ok(OpenAiSettings::default());
    };
    if record.len() > MAX_SETTINGS_BYTES {
        return Err(
            "保存されたOpenAI設定の形式が不正です。設定を確認して保存し直してください。".to_owned(),
        );
    }
    let settings: OpenAiSettings = serde_json::from_str(&record).map_err(|_| {
        "保存されたOpenAI設定を読み取れませんでした。設定を確認して保存し直してください。"
            .to_owned()
    })?;
    validate_model(&settings.model)?;
    Ok(settings)
}

pub(crate) fn write_settings(
    credentials: &dyn CredentialStore,
    model: String,
    auth_mode: OpenAiAuthMode,
) -> Result<(), String> {
    validate_model(&model)?;
    let record = serde_json::to_string(&OpenAiSettings { model, auth_mode })
        .map_err(|_| "OpenAI設定を保存できませんでした。".to_owned())?;
    credentials.set(CredentialKind::OpenAiSettings, &record)
}

type TransportFuture<'a> = Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + 'a>>;

trait OpenAiTransport: Send + Sync {
    fn send(&self, request: Request, limit: usize) -> TransportFuture<'_>;
}

struct HttpTransport {
    http: Result<Client, String>,
}

struct HttpResponse {
    status: StatusCode,
    request_id: Option<String>,
    chunks: Vec<Vec<u8>>,
}

fn http_client() -> Result<Client, String> {
    Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|_| "OpenAIへの接続を準備できませんでした。".to_owned())
}

fn network_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "OpenAIへの通信がタイムアウトしました。再試行してください。".to_owned()
    } else {
        "OpenAIに接続できませんでした。通信状態を確認して再試行してください。".to_owned()
    }
}

impl OpenAiTransport for HttpTransport {
    fn send(&self, request: Request, limit: usize) -> TransportFuture<'_> {
        Box::pin(async move {
            let response = self
                .http
                .as_ref()
                .map_err(Clone::clone)?
                .execute(request)
                .await
                .map_err(network_error)?;
            read_http_response(response, limit).await
        })
    }
}

async fn read_http_response(mut response: Response, limit: usize) -> Result<HttpResponse, String> {
    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let limit = if status.is_success() {
        limit
    } else {
        MAX_ERROR_BYTES
    };
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(format!(
            "OpenAIの応答がサイズ上限を超えました (HTTP {})。",
            status.as_u16()
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        body.len()
            .checked_add(chunk.len())
            .filter(|bytes| *bytes <= limit)
            .ok_or_else(|| {
                format!(
                    "OpenAIの応答がサイズ上限を超えました (HTTP {})。",
                    status.as_u16()
                )
            })?;
        body.extend_from_slice(&chunk);
    }
    Ok(HttpResponse {
        status,
        request_id,
        chunks: vec![body],
    })
}

pub(crate) struct OpenAiClient {
    credentials: Arc<dyn CredentialStore>,
    transport: Arc<dyn OpenAiTransport>,
}

impl OpenAiClient {
    pub(crate) fn new(credentials: Arc<dyn CredentialStore>) -> Self {
        Self {
            credentials,
            transport: Arc::new(HttpTransport {
                http: http_client(),
            }),
        }
    }

    async fn settings(&self) -> Result<OpenAiSettings, String> {
        let credentials = self.credentials.clone();
        tokio::task::spawn_blocking(move || read_settings(credentials.as_ref()))
            .await
            .map_err(|_| "OpenAI設定を取得できませんでした。".to_owned())?
    }

    async fn authorization(&self, mode: OpenAiAuthMode) -> Result<String, String> {
        match mode {
            OpenAiAuthMode::Api => {
                let credentials = self.credentials.clone();
                tokio::task::spawn_blocking(move || {
                    credentials
                        .get(CredentialKind::ApiKey(SearchProvider::Openai))?
                        .filter(|key| !key.trim().is_empty())
                        .ok_or_else(|| "OpenAIのAPIキーを設定してください。".to_owned())
                })
                .await
                .map_err(|_| "OpenAIのAPIキーを取得できませんでした。".to_owned())?
            }
            OpenAiAuthMode::Chatgpt => {
                ChatGptClient::new(self.credentials.clone())
                    .access_token()
                    .await
            }
        }
    }

    pub(crate) async fn search(&self, query: &str) -> Result<Vec<CandidateSeed>, String> {
        if query.trim().is_empty()
            || query.chars().count() > 400
            || query.split_whitespace().count() > 50
        {
            return Err("画像の検索語は400文字・50語以内で入力してください。".to_owned());
        }
        let settings = self.settings().await?;
        let secret = self.authorization(settings.auth_mode).await?;
        let request = search_request(&settings.model, &secret, query)?;
        let response = self.transport.send(request, MAX_RESPONSE_BYTES).await?;
        let context = ErrorContext {
            settings: &settings,
            secret: &secret,
            status: response.status,
            request_id: response.request_id.as_deref(),
        };
        check_http_response(&response, MAX_RESPONSE_BYTES, &context)?;
        if !response.status.is_success() {
            return Err(context.failure(json_error(&response.chunks).as_ref()));
        }
        let completed = completed_response(&response.chunks, &context)?;
        image_results(&completed, &settings.model)
    }

    /// Optional catalog assistance. Saving and searching never depend on this list.
    pub(crate) async fn models(&self) -> Result<Vec<String>, String> {
        let settings = self.settings().await?;
        let secret = self.authorization(settings.auth_mode).await?;
        let request = Client::new()
            .get(MODELS_URL)
            .bearer_auth(&secret)
            .header("Accept", "application/json")
            .build()
            .map_err(|_| "OpenAIのモデル一覧の要求を作成できませんでした。".to_owned())?;
        let response = self.transport.send(request, MAX_MODELS_BYTES).await?;
        let context = ErrorContext {
            settings: &settings,
            secret: &secret,
            status: response.status,
            request_id: response.request_id.as_deref(),
        };
        check_http_response(&response, MAX_MODELS_BYTES, &context)?;
        if !response.status.is_success() {
            return Err(context.failure(json_error(&response.chunks).as_ref()));
        }
        let value: Value = serde_json::from_slice(&response.chunks.concat())
            .map_err(|_| "OpenAIのモデル一覧を読み取れませんでした。".to_owned())?;
        model_ids(&value, settings.auth_mode)
    }
}

fn search_request(model: &str, secret: &str, query: &str) -> Result<Request, String> {
    Client::new()
        .post(RESPONSES_URL)
        .bearer_auth(secret)
        .header("Accept", "text/event-stream")
        .json(&json!({
            "model": model,
            "reasoning": { "effort": "low" },
            "tools": [{
                "type": "web_search",
                "search_content_types": ["image", "text"],
                "image_settings": { "max_results": MAX_RESULTS, "caption": true }
            }],
            "include": ["web_search_call.results"],
            "input": [{ "role": "user", "content": query }],
            "instructions": "Search web images matching query. Use web_search for image results.",
            "store": false,
            "stream": true
        }))
        .build()
        .map_err(|_| "OpenAIの画像検索の要求を作成できませんでした。".to_owned())
}

fn check_http_response(
    response: &HttpResponse,
    limit: usize,
    context: &ErrorContext<'_>,
) -> Result<(), String> {
    let limit = if response.status.is_success() {
        limit
    } else {
        MAX_ERROR_BYTES
    };
    let bytes = response
        .chunks
        .iter()
        .try_fold(0usize, |total, chunk| total.checked_add(chunk.len()));
    if bytes.is_none_or(|bytes| bytes > limit) {
        return Err(
            context.with_metadata("OpenAIの応答がサイズ上限を超えました。".to_owned(), None)
        );
    }
    Ok(())
}

fn model_ids(value: &Value, mode: OpenAiAuthMode) -> Result<Vec<String>, String> {
    let (field, id_field) = match mode {
        OpenAiAuthMode::Api => ("data", "id"),
        OpenAiAuthMode::Chatgpt => ("models", "slug"),
    };
    let entries = value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| "OpenAIのモデル一覧の形式が不正です。".to_owned())?;
    Ok(entries
        .iter()
        .filter(|entry| {
            mode != OpenAiAuthMode::Chatgpt
                || entry.get("visibility").and_then(Value::as_str) == Some("list")
        })
        .filter_map(|entry| entry.get(id_field).and_then(Value::as_str))
        .filter(|id| validate_model(id).is_ok())
        .map(str::to_owned)
        .collect())
}

struct ErrorContext<'a> {
    settings: &'a OpenAiSettings,
    secret: &'a str,
    status: StatusCode,
    request_id: Option<&'a str>,
}

impl ErrorContext<'_> {
    fn failure(&self, error: Option<&Value>) -> String {
        let code = error
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str);
        let param = error
            .and_then(|error| error.get("param"))
            .and_then(Value::as_str);
        let message = match code {
            Some("subscription_sharing_usage_limit_exceeded") if self.settings.auth_mode == OpenAiAuthMode::Chatgpt =>
                "ChatGPTプランの利用上限に達しました。「利用状況を管理」で確認してください。".to_owned(),
            Some("subscription_sharing_usage_unavailable") if self.settings.auth_mode == OpenAiAuthMode::Chatgpt =>
                "ChatGPTプランの使用を現在利用できません。「利用状況を管理」で確認して再試行してください。".to_owned(),
            _ if self.status == StatusCode::UNAUTHORIZED => match self.settings.auth_mode {
                OpenAiAuthMode::Api => "OpenAIのAPIキーを認証できませんでした。APIキーを確認して保存し直してください。".to_owned(),
                OpenAiAuthMode::Chatgpt => "ChatGPTの接続を認証できませんでした。接続し直してください。".to_owned(),
            },
            _ if self.status == StatusCode::TOO_MANY_REQUESTS || code == Some("rate_limit_exceeded") =>
                "OpenAIのリクエスト上限に達しました。時間をおいて再試行してください。".to_owned(),
            _ if self.status == StatusCode::FORBIDDEN || self.status == StatusCode::NOT_FOUND || model_failure(error, code, param) =>
                format!("OpenAIのモデル「{}」で画像検索を利用できません。モデルへのアクセス権を確認するか、別のモデルを指定してください。", self.settings.model),
            _ => "OpenAIの画像検索を完了できませんでした。再試行してください。".to_owned(),
        };
        self.with_metadata(message, error)
    }

    fn with_metadata(&self, message: String, error: Option<&Value>) -> String {
        let mut metadata = vec![format!("HTTP {}", self.status.as_u16())];
        for (name, value) in [
            ("request_id", self.request_id),
            (
                "code",
                error
                    .and_then(|error| error.get("code"))
                    .and_then(Value::as_str),
            ),
            (
                "param",
                error
                    .and_then(|error| error.get("param"))
                    .and_then(Value::as_str),
            ),
        ] {
            if let Some(value) = value.and_then(|value| diagnostic_value(value, self.secret)) {
                metadata.push(format!("{name}={value}"));
            }
        }
        format!("{message} ({})", metadata.join(", "))
    }
}

fn diagnostic_value<'a>(value: &'a str, secret: &str) -> Option<&'a str> {
    (!value.is_empty()
        && value.len() <= 128
        && !value.starts_with("sk-")
        && (secret.is_empty() || !value.contains(secret))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.[]".contains(&byte)))
    .then_some(value)
}

fn model_failure(error: Option<&Value>, code: Option<&str>, param: Option<&str>) -> bool {
    if code.is_some_and(|code| {
        [
            "model_not_found",
            "invalid_model",
            "unsupported_model",
            "model_not_supported",
            "unsupported_tool",
        ]
        .contains(&code)
    }) || param.is_some_and(|param| param == "model" || param.starts_with("tools"))
    {
        return true;
    }
    // Use the untrusted message only to classify an error; never return or log it.
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .filter(|message| message.len() <= MAX_ERROR_BYTES)
        .unwrap_or_default()
        .to_ascii_lowercase();
    (message.contains("model")
        || message.contains("web_search")
        || message.contains("image_settings")
        || message.contains("search_content_types"))
        && (message.contains("not support")
            || message.contains("unsupported")
            || message.contains("not found")
            || message.contains("does not exist"))
}

fn json_error(chunks: &[Vec<u8>]) -> Option<Value> {
    let value: Value = serde_json::from_slice(&chunks.concat()).ok()?;
    value.get("error").cloned().or_else(|| {
        value
            .get("response")
            .and_then(|response| response.get("error"))
            .cloned()
    })
}

/// SSE framing is byte based: UTF-8, CRLF and JSON may all span network chunks.
#[derive(Default)]
struct SseDecoder {
    line: Vec<u8>,
    data: Vec<u8>,
    event: Option<String>,
    after_cr: bool,
    total: usize,
}

impl SseDecoder {
    fn push(
        &mut self,
        bytes: &[u8],
        mut receive: impl FnMut(&str, &[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        self.total = self
            .total
            .checked_add(bytes.len())
            .filter(|total| *total <= MAX_RESPONSE_BYTES)
            .ok_or_else(|| "OpenAIの応答がサイズ上限を超えました。".to_owned())?;
        for &byte in bytes {
            if self.after_cr {
                self.after_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if byte == b'\r' || byte == b'\n' {
                self.finish_line(&mut receive)?;
                self.after_cr = byte == b'\r';
            } else {
                self.line.push(byte);
            }
        }
        Ok(())
    }

    fn finish_line(
        &mut self,
        receive: &mut impl FnMut(&str, &[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        let line = std::mem::take(&mut self.line);
        if line.is_empty() {
            if !self.data.is_empty() {
                receive(self.event.as_deref().unwrap_or_default(), &self.data)?;
            }
            self.data.clear();
            self.event = None;
            return Ok(());
        }
        if line[0] == b':' {
            return Ok(());
        }
        let colon = line
            .iter()
            .position(|byte| *byte == b':')
            .unwrap_or(line.len());
        let (field, rest) = line.split_at(colon);
        let value = rest.strip_prefix(b":").unwrap_or_default();
        let value = value.strip_prefix(b" ").unwrap_or(value);
        match field {
            b"data" => {
                if !self.data.is_empty() {
                    self.data.push(b'\n');
                }
                self.data.extend_from_slice(value);
            }
            b"event" => {
                if value.len() > 128 {
                    return Err("OpenAIのストリームの形式が不正です。".to_owned());
                }
                self.event = Some(
                    std::str::from_utf8(value)
                        .map_err(|_| "OpenAIのストリームを読み取れませんでした。".to_owned())?
                        .to_owned(),
                );
            }
            _ => {}
        }
        Ok(())
    }
}

fn completed_response(chunks: &[Vec<u8>], context: &ErrorContext<'_>) -> Result<Value, String> {
    // Some services send a JSON error even with a successful HTTP status.
    if let Some(error) = json_error(chunks) {
        return Err(context.failure(Some(&error)));
    }
    let mut decoder = SseDecoder::default();
    let mut completed = None;
    for chunk in chunks {
        decoder.push(chunk, |event, data| {
            if data == b"[DONE]" {
                return Ok(());
            }
            let value: Value = serde_json::from_slice(data)
                .map_err(|_| "OpenAIのストリームを読み取れませんでした。".to_owned())?;
            let kind = value.get("type").and_then(Value::as_str).unwrap_or(event);
            if !event.is_empty() && event != kind {
                return Err("OpenAIのストリームのイベント種別が一致しません。".to_owned());
            }
            match kind {
                "response.completed" => {
                    if completed.is_some() {
                        return Err("OpenAIのストリームの完了イベントが重複しています。".to_owned());
                    }
                    let response = value
                        .get("response")
                        .ok_or_else(|| "OpenAIの検索結果の形式が不正です。".to_owned())?;
                    if response.get("status").and_then(Value::as_str) != Some("completed")
                        || response.get("error").is_some_and(|error| !error.is_null())
                    {
                        return Err(context.failure(response.get("error")));
                    }
                    completed = Some(response.clone());
                }
                "response.failed" | "error" => {
                    let error = value
                        .get("response")
                        .and_then(|response| response.get("error"))
                        .or_else(|| value.get("error"))
                        .unwrap_or(&value);
                    return Err(context.failure(Some(error)));
                }
                "response.incomplete" => {
                    if let Some(error) = value
                        .get("response")
                        .and_then(|response| response.get("error"))
                        .filter(|error| !error.is_null())
                    {
                        return Err(context.failure(Some(error)));
                    }
                    return Err(context.with_metadata(
                        "OpenAIの画像検索が未完了で終了しました。再試行してください。".to_owned(),
                        None,
                    ));
                }
                _ => {}
            }
            Ok(())
        })?;
    }
    // A partial final event (without a blank line) is not a dispatched SSE event.
    if !decoder.line.is_empty() || !decoder.data.is_empty() {
        return Err(context.with_metadata(
            "OpenAIのストリームが途中で切断されました。再試行してください。".to_owned(),
            None,
        ));
    }
    completed.ok_or_else(|| {
        context.with_metadata(
            "OpenAIの画像検索が完了する前にストリームが終了しました。再試行してください。"
                .to_owned(),
            None,
        )
    })
}

pub(crate) fn image_results(response: &Value, model: &str) -> Result<Vec<CandidateSeed>, String> {
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| "OpenAIの検索結果の形式が不正です。".to_owned())?;
    let mut found_search = false;
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for item in output
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("web_search_call"))
    {
        if item.get("status").and_then(Value::as_str) != Some("completed") {
            return Err(
                "OpenAIの画像検索ツールが完了していません。再試行してください。".to_owned(),
            );
        }
        let results = item.get("results").and_then(Value::as_array)
            .ok_or_else(|| format!("OpenAIのモデル「{model}」から画像検索の結果を取得できませんでした。画像検索に対応する別のモデルを指定してください。"))?;
        found_search = true;
        for result in results
            .iter()
            .filter(|result| result.get("type").and_then(Value::as_str) == Some("image_result"))
        {
            let Some(image_url) = result
                .get("image_url")
                .and_then(Value::as_str)
                .filter(|url| validate_public_url(url).is_ok())
            else {
                continue;
            };
            let Some(source_url) = result
                .get("source_website_url")
                .and_then(Value::as_str)
                .filter(|url| validate_public_url(url).is_ok())
            else {
                continue;
            };
            if !seen.insert(image_url.to_owned()) || candidates.len() == MAX_RESULTS {
                continue;
            }
            let thumbnail_url = result
                .get("thumbnail_url")
                .and_then(Value::as_str)
                .filter(|url| validate_public_url(url).is_ok())
                .map(str::to_owned);
            let title = result
                .get("caption")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|caption| !caption.is_empty())
                .unwrap_or("画像検索の結果")
                .chars()
                .take(300)
                .collect();
            candidates.push(CandidateSeed {
                title,
                source_url: source_url.to_owned(),
                image_url: Some(image_url.to_owned()),
                thumbnail_url,
            });
        }
    }
    if !found_search {
        return Err(format!(
            "OpenAIのモデル「{model}」が画像検索の結果を返しませんでした。モデルへのアクセス権を確認するか、別のモデルを指定してください。"
        ));
    }
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::{HashMap, VecDeque},
        sync::Mutex,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    const API_SECRET: &str = "sk-test-private-api-key";
    const CHATGPT_SECRET: &str = "private-chatgpt-access-token";

    #[derive(Default)]
    struct MemoryCredentials {
        records: Mutex<HashMap<CredentialKind, String>>,
        reads: Mutex<Vec<CredentialKind>>,
        writes: Mutex<Vec<CredentialKind>>,
    }

    impl CredentialStore for MemoryCredentials {
        fn get(&self, kind: CredentialKind) -> Result<Option<String>, String> {
            self.reads.lock().unwrap().push(kind);
            Ok(self.records.lock().unwrap().get(&kind).cloned())
        }

        fn set(&self, kind: CredentialKind, value: &str) -> Result<(), String> {
            self.writes.lock().unwrap().push(kind);
            self.records.lock().unwrap().insert(kind, value.to_owned());
            Ok(())
        }
    }

    struct RecordedRequest {
        method: reqwest::Method,
        url: String,
        authorization: String,
        accept: String,
        body: Value,
        limit: usize,
    }

    #[derive(Default)]
    struct MockTransport {
        requests: Mutex<Vec<RecordedRequest>>,
        responses: Mutex<VecDeque<Result<HttpResponse, String>>>,
    }

    impl OpenAiTransport for MockTransport {
        fn send(&self, request: Request, limit: usize) -> TransportFuture<'_> {
            Box::pin(async move {
                self.requests.lock().unwrap().push(RecordedRequest {
                    method: request.method().clone(),
                    url: request.url().to_string(),
                    authorization: request
                        .headers()
                        .get("Authorization")
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                    accept: request
                        .headers()
                        .get("Accept")
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                    body: request
                        .body()
                        .and_then(|body| body.as_bytes())
                        .map(|bytes| serde_json::from_slice(bytes).unwrap())
                        .unwrap_or(Value::Null),
                    limit,
                });
                self.responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unexpected HTTP request")
            })
        }
    }

    fn credentials(mode: OpenAiAuthMode) -> Arc<MemoryCredentials> {
        let credentials = Arc::new(MemoryCredentials::default());
        write_settings(
            credentials.as_ref(),
            "future-unlisted-model".to_owned(),
            mode,
        )
        .unwrap();
        credentials
            .set(CredentialKind::ApiKey(SearchProvider::Openai), API_SECRET)
            .unwrap();
        credentials.set(CredentialKind::ChatGptSession, &json!({
            "issuer": "https://auth.openai.com", "subject": "test-subject", "client_id": "issued-test-client",
            "email": "test@example.com", "id_token": "test-id-token", "access_token": CHATGPT_SECRET,
            "refresh_token": "test-refresh-token", "scopes": ["chatgpt.tokens.use.direct"], "expires_at": u64::MAX
        }).to_string()).unwrap();
        credentials.reads.lock().unwrap().clear();
        credentials.writes.lock().unwrap().clear();
        credentials
    }

    fn client(
        credentials: Arc<MemoryCredentials>,
        responses: Vec<HttpResponse>,
    ) -> (OpenAiClient, Arc<MockTransport>) {
        let transport = Arc::new(MockTransport {
            requests: Mutex::default(),
            responses: Mutex::new(responses.into_iter().map(Ok).collect()),
        });
        (
            OpenAiClient {
                credentials,
                transport: transport.clone(),
            },
            transport,
        )
    }

    fn response(status: u16, body: Vec<u8>) -> HttpResponse {
        HttpResponse {
            status: StatusCode::from_u16(status).unwrap(),
            request_id: Some("req_mock_123".to_owned()),
            chunks: vec![body],
        }
    }

    fn image_result() -> Value {
        json!({"type": "image_result", "image_url": "https://cdn.example.com/photo.png", "source_website_url": "https://example.com/page", "thumbnail_url": "https://cdn.example.com/thumb.png", "caption": "猫 at sunset"})
    }

    fn completed_images(results: Vec<Value>) -> Value {
        json!({"status": "completed", "output": [{"type": "web_search_call", "status": "completed", "results": results}]})
    }

    fn event(kind: &str, response: Value) -> Vec<u8> {
        format!(
            "event: {kind}\ndata: {}\n\n",
            json!({"type": kind, "response": response})
        )
        .into_bytes()
    }

    fn success() -> HttpResponse {
        response(
            200,
            event("response.completed", completed_images(vec![image_result()])),
        )
    }

    #[test]
    fn settings_are_one_explicit_record_and_never_read_authentication() {
        let credentials = MemoryCredentials::default();
        assert_eq!(
            read_settings(&credentials).unwrap(),
            OpenAiSettings::default()
        );
        assert!(credentials.writes.lock().unwrap().is_empty());
        write_settings(
            &credentials,
            "unlisted-model:next".to_owned(),
            OpenAiAuthMode::Chatgpt,
        )
        .unwrap();
        assert_eq!(
            read_settings(&credentials).unwrap(),
            OpenAiSettings {
                model: "unlisted-model:next".to_owned(),
                auth_mode: OpenAiAuthMode::Chatgpt
            }
        );
        assert!(
            credentials
                .reads
                .lock()
                .unwrap()
                .iter()
                .all(|kind| *kind == CredentialKind::OpenAiSettings)
        );
        assert_eq!(
            *credentials.writes.lock().unwrap(),
            vec![CredentialKind::OpenAiSettings]
        );
        let record = credentials
            .records
            .lock()
            .unwrap()
            .get(&CredentialKind::OpenAiSettings)
            .cloned()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&record).unwrap(),
            json!({"model": "unlisted-model:next", "auth_mode": "chatgpt"})
        );
    }

    #[test]
    fn corrupt_or_partial_settings_are_rejected_without_overwriting_them() {
        for record in [
            "",
            "not-json",
            r#"{"model":"gpt-6-luna"}"#,
            r#"{"auth_mode":"api"}"#,
            r#"{"model":"x","auth_mode":"unknown"}"#,
            r#"{"model":"x","auth_mode":"api","legacy":"value"}"#,
            r#"{"model":"white space","auth_mode":"api"}"#,
        ] {
            let credentials = MemoryCredentials::default();
            credentials
                .records
                .lock()
                .unwrap()
                .insert(CredentialKind::OpenAiSettings, record.to_owned());
            assert!(read_settings(&credentials).is_err(), "{record}");
            assert_eq!(
                credentials.records.lock().unwrap()[&CredentialKind::OpenAiSettings],
                record
            );
            assert!(credentials.writes.lock().unwrap().is_empty());
        }
        let credentials = MemoryCredentials::default();
        credentials.records.lock().unwrap().insert(
            CredentialKind::OpenAiSettings,
            "x".repeat(MAX_SETTINGS_BYTES + 1),
        );
        assert!(read_settings(&credentials).unwrap_err().contains("形式"));
    }

    #[test]
    fn model_validation_accepts_unlisted_ids_but_rejects_invalid_input_before_write() {
        let credentials = MemoryCredentials::default();
        for invalid in [
            "",
            " leading",
            "line\nfeed",
            "tab\tmodel",
            "\0model",
            &"x".repeat(129),
        ] {
            assert!(write_settings(&credentials, invalid.to_owned(), OpenAiAuthMode::Api).is_err());
        }
        assert!(credentials.writes.lock().unwrap().is_empty());
        write_settings(&credentials, "x".repeat(128), OpenAiAuthMode::Api).unwrap();
        assert_eq!(read_settings(&credentials).unwrap().model.len(), 128);
    }

    #[tokio::test]
    async fn both_auth_modes_send_the_saved_unlisted_model_and_required_image_search_body() {
        for (mode, secret, unused_kind) in [
            (
                OpenAiAuthMode::Api,
                API_SECRET,
                CredentialKind::ChatGptSession,
            ),
            (
                OpenAiAuthMode::Chatgpt,
                CHATGPT_SECRET,
                CredentialKind::ApiKey(SearchProvider::Openai),
            ),
        ] {
            let credentials = credentials(mode);
            let (client, transport) = client(credentials.clone(), vec![success()]);
            let candidates = client.search("cats at sunset").await.unwrap();
            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].title, "猫 at sunset");
            let requests = transport.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let request = &requests[0];
            assert_eq!(request.method, reqwest::Method::POST);
            assert_eq!(request.url, RESPONSES_URL);
            assert_eq!(request.authorization, format!("Bearer {secret}"));
            assert_eq!(request.accept, "text/event-stream");
            assert_eq!(request.limit, MAX_RESPONSE_BYTES);
            assert_eq!(
                request.body,
                json!({
                    "model": "future-unlisted-model", "reasoning": {"effort": "low"},
                    "tools": [{"type": "web_search", "search_content_types": ["image", "text"], "image_settings": {"max_results": 30, "caption": true}}],
                    "include": ["web_search_call.results"], "input": [{"role": "user", "content": "cats at sunset"}],
                    "instructions": "Search web images matching query. Use web_search for image results.", "store": false, "stream": true
                })
            );
            assert!(!credentials.reads.lock().unwrap().contains(&unused_kind));
            assert!(credentials.writes.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn selected_authentication_never_falls_back_to_the_other_credential() {
        for (mode, missing_kind, unused_kind) in [
            (
                OpenAiAuthMode::Api,
                CredentialKind::ApiKey(SearchProvider::Openai),
                CredentialKind::ChatGptSession,
            ),
            (
                OpenAiAuthMode::Chatgpt,
                CredentialKind::ChatGptSession,
                CredentialKind::ApiKey(SearchProvider::Openai),
            ),
        ] {
            let credentials = credentials(mode);
            credentials.records.lock().unwrap().remove(&missing_kind);
            let (client, transport) = client(credentials.clone(), vec![]);
            assert!(client.search("cats").await.is_err());
            assert!(transport.requests.lock().unwrap().is_empty());
            assert!(!credentials.reads.lock().unwrap().contains(&unused_kind));
            assert!(credentials.writes.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn optional_catalog_preserves_server_order_and_does_not_block_saved_model_search() {
        for (mode, catalog, expected, secret) in [
            (
                OpenAiAuthMode::Api,
                json!({"data": [{"id": "model-z"}, {"id": "model-a"}]}),
                vec!["model-z", "model-a"],
                API_SECRET,
            ),
            (
                OpenAiAuthMode::Chatgpt,
                json!({"models": [
                    {"slug": "model-z", "visibility": "list"}, {"slug": "hidden", "visibility": "hidden"},
                    {"slug": "model-a", "visibility": "list"}, {"slug": "not-visible"}
                ]}),
                vec!["model-z", "model-a"],
                CHATGPT_SECRET,
            ),
        ] {
            let credentials = credentials(mode);
            let (client, transport) = client(
                credentials.clone(),
                vec![response(200, catalog.to_string().into_bytes()), success()],
            );
            assert_eq!(client.models().await.unwrap(), expected);
            assert_eq!(client.search("cats").await.unwrap().len(), 1);
            let requests = transport.requests.lock().unwrap();
            assert_eq!(requests[0].method, reqwest::Method::GET);
            assert_eq!(requests[0].url, MODELS_URL);
            assert_eq!(requests[0].authorization, format!("Bearer {secret}"));
            assert!(requests[0].body.is_null());
            assert_eq!(requests[0].limit, MAX_MODELS_BYTES);
            assert_eq!(requests[1].body["model"], "future-unlisted-model");
            assert_eq!(
                read_settings(credentials.as_ref()).unwrap().model,
                "future-unlisted-model"
            );
            assert!(credentials.writes.lock().unwrap().is_empty());
        }
        assert!(model_ids(&json!({"object": "list"}), OpenAiAuthMode::Api).is_err());
        assert_eq!(
            model_ids(
                &json!({"data": [{"id": "bad id"}, {"id": 42}, {"id": "unlisted-valid"}]}),
                OpenAiAuthMode::Api
            )
            .unwrap(),
            vec!["unlisted-valid"]
        );
    }

    #[tokio::test]
    async fn catalog_failure_does_not_mutate_the_model_or_prevent_the_next_search() {
        let credentials = credentials(OpenAiAuthMode::Api);
        let (client, transport) = client(
            credentials.clone(),
            vec![
                response(
                    403,
                    json!({"error": {"code": "permission_denied"}})
                        .to_string()
                        .into_bytes(),
                ),
                success(),
            ],
        );
        assert!(client.models().await.is_err());
        assert_eq!(client.search("cats").await.unwrap().len(), 1);
        assert_eq!(
            transport.requests.lock().unwrap()[1].body["model"],
            "future-unlisted-model"
        );
        assert!(credentials.writes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn errors_keep_safe_diagnostics_and_distinguish_authentication_model_access_and_rate_limits()
     {
        for (status, code, param, expected) in [
            (401, "invalid_api_key", Value::Null, "APIキーを確認"),
            (403, "permission_denied", Value::Null, "アクセス権"),
            (
                404,
                "model_not_found",
                json!("model"),
                "future-unlisted-model",
            ),
            (
                400,
                "invalid_request_error",
                json!("tools[0].image_settings"),
                "別のモデル",
            ),
            (
                400,
                "unsupported_model",
                Value::Null,
                "future-unlisted-model",
            ),
            (429, "rate_limit_exceeded", Value::Null, "時間をおいて"),
            (429, "rate_limit_exceeded", json!("model"), "時間をおいて"),
        ] {
            let body = json!({"error": {"code": code, "param": param, "message": format!("Never return this raw server message: {API_SECRET}")}});
            let (client, transport) = client(
                credentials(OpenAiAuthMode::Api),
                vec![response(status, body.to_string().into_bytes())],
            );
            let error = client.search("cats").await.err().unwrap();
            assert!(error.contains(expected), "{error}");
            assert!(error.contains(&format!("HTTP {status}")));
            assert!(error.contains("request_id=req_mock_123"));
            assert!(error.contains(&format!("code={code}")));
            assert!(!error.contains(API_SECRET));
            assert!(!error.contains("Never return"));
            assert!(!error.contains("廃止"));
            assert_eq!(transport.requests.lock().unwrap().len(), 1);
        }
        let (client, _) = client(
            credentials(OpenAiAuthMode::Chatgpt),
            vec![response(
                401,
                br#"{"error":{"code":"invalid_token"}}"#.to_vec(),
            )],
        );
        let error = client.search("cats").await.err().unwrap();
        assert!(error.contains("接続し直して"));
        assert!(!error.contains("APIキー"));
    }

    #[test]
    fn diagnostic_metadata_rejects_secrets_control_characters_and_unbounded_values() {
        let settings = OpenAiSettings::default();
        let context = ErrorContext {
            settings: &settings,
            secret: API_SECRET,
            status: StatusCode::BAD_REQUEST,
            request_id: Some(API_SECRET),
        };
        let error = context.failure(Some(&json!({"code": API_SECRET, "param": "model\nInjected text", "message": CHATGPT_SECRET})));
        assert!(!error.contains(API_SECRET));
        assert!(!error.contains(CHATGPT_SECRET));
        assert!(!error.contains("Injected"));
        assert_eq!(diagnostic_value(&"x".repeat(129), API_SECRET), None);
        assert_eq!(
            diagnostic_value("tools[0].image_settings", API_SECRET),
            Some("tools[0].image_settings")
        );
    }

    #[test]
    fn sse_handles_multiline_data_crlf_utf8_and_every_fragment_boundary() {
        let settings = OpenAiSettings::default();
        let context = ErrorContext {
            settings: &settings,
            secret: API_SECRET,
            status: StatusCode::OK,
            request_id: None,
        };
        let payload = json!({"type": "response.completed", "response": completed_images(vec![image_result()])});
        let pretty = serde_json::to_string_pretty(&payload).unwrap();
        let multiline = pretty
            .lines()
            .map(|line| format!("data: {line}\r\n"))
            .collect::<String>();
        let stream = format!(
            ": keepalive\r\nevent: response.completed\r\n{multiline}\r\ndata: [DONE]\r\n\r\n"
        )
        .into_bytes();
        for offset in 0..=stream.len() {
            let chunks = vec![stream[..offset].to_vec(), stream[offset..].to_vec()];
            let completed = completed_response(&chunks, &context).unwrap();
            assert_eq!(
                image_results(&completed, &settings.model).unwrap()[0].title,
                "猫 at sunset"
            );
        }
        let bytes = stream.iter().map(|byte| vec![*byte]).collect::<Vec<_>>();
        assert_eq!(
            image_results(
                &completed_response(&bytes, &context).unwrap(),
                &settings.model
            )
            .unwrap()
            .len(),
            1
        );
    }

    #[test]
    fn only_completed_final_output_supplies_images_and_terminal_failures_reject_partial_results() {
        let settings = OpenAiSettings::default();
        let context = ErrorContext {
            settings: &settings,
            secret: API_SECRET,
            status: StatusCode::OK,
            request_id: Some("req_stream"),
        };
        let progress = event(
            "response.output_item.done",
            json!({"output": [image_result()]}),
        );
        for ending in [
            event(
                "response.failed",
                json!({"error": {"code": "server_error", "message": API_SECRET}}),
            ),
            event(
                "response.incomplete",
                json!({"incomplete_details": {"reason": "max_output_tokens"}}),
            ),
            b"event: error\ndata: {\"type\":\"error\",\"code\":\"server_error\"}\n\n".to_vec(),
            b"data: [DONE]\n\n".to_vec(),
            Vec::new(),
            event("response.completed", completed_images(vec![image_result()]))[..30].to_vec(),
        ] {
            assert!(completed_response(&[progress.clone(), ending], &context).is_err());
        }
        let final_event = event("response.completed", completed_images(vec![image_result()]));
        assert!(
            completed_response(
                &[
                    final_event.clone(),
                    event(
                        "response.failed",
                        json!({"error": {"code": "server_error"}})
                    )
                ],
                &context
            )
            .is_err()
        );
        assert!(completed_response(&[final_event.clone(), final_event], &context).is_err());
        assert!(
            completed_response(
                &[b"event: response.completed\ndata: {\"type\":\"response.failed\"}\n\n".to_vec()],
                &context
            )
            .is_err()
        );
        assert!(
            completed_response(
                &[completed_images(vec![image_result()])
                    .to_string()
                    .into_bytes()],
                &context
            )
            .is_err()
        );
        assert!(
            completed_response(
                &[event(
                    "response.completed",
                    json!({"status": "incomplete", "output": []})
                )],
                &context
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn chatgpt_usage_failures_are_distinct_in_json_and_after_streaming_begins() {
        for (code, expected) in [
            (
                "subscription_sharing_usage_limit_exceeded",
                "利用上限に達しました",
            ),
            (
                "subscription_sharing_usage_unavailable",
                "現在利用できません",
            ),
        ] {
            let error = json!({"code": code, "message": CHATGPT_SECRET});
            for response in [
                response(429, json!({"error": error}).to_string().into_bytes()),
                response(200, json!({"error": error}).to_string().into_bytes()),
                HttpResponse {
                    status: StatusCode::OK,
                    request_id: Some("req_stream".to_owned()),
                    chunks: vec![
                        event("response.created", json!({"status": "in_progress"})),
                        event("response.failed", json!({"error": error})),
                    ],
                },
            ] {
                let (client, transport) =
                    client(credentials(OpenAiAuthMode::Chatgpt), vec![response]);
                let failure = client.search("cats").await.err().unwrap();
                assert!(failure.contains(expected), "{failure}");
                assert!(failure.contains("利用状況を管理"));
                assert!(failure.contains(code));
                assert!(!failure.contains(CHATGPT_SECRET));
                assert!(!failure.contains("APIキー"));
                assert_eq!(transport.requests.lock().unwrap().len(), 1);
            }
        }
    }

    #[test]
    fn structured_images_exclude_text_generated_images_unsafe_urls_and_duplicates() {
        let mut unsafe_image = image_result();
        unsafe_image["image_url"] = json!("http://127.0.0.1/private.png");
        let mut unsafe_source = image_result();
        unsafe_source["source_website_url"] = json!("http://localhost/private");
        let mut no_source = image_result();
        no_source
            .as_object_mut()
            .unwrap()
            .remove("source_website_url");
        let mut valid = image_result();
        valid["thumbnail_url"] = json!("data:image/png;base64,forbidden");
        let output = completed_images(vec![
            json!({"type": "text_result", "image_url": "https://cdn.example.com/text.png", "source_website_url": "https://example.com/page"}),
            unsafe_image,
            unsafe_source,
            no_source,
            valid.clone(),
            valid,
        ]);
        let candidates = image_results(&output, "selected").unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].image_url.as_deref(),
            Some("https://cdn.example.com/photo.png")
        );
        assert_eq!(candidates[0].source_url, "https://example.com/page");
        assert!(candidates[0].thumbnail_url.is_none());
        let generated = json!({"output": [{"type": "image_generation_call", "result": "base64-image"}, {"type": "message", "content": [{"text": "https://cdn.example.com/photo.png"}]}]});
        let error = image_results(&generated, "chosen-model").err().unwrap();
        assert!(error.contains("chosen-model"));
        assert!(
            image_results(&completed_images(vec![]), "selected")
                .unwrap()
                .is_empty()
        );
        assert!(
            image_results(
                &json!({"output": [{"type": "web_search_call", "status": "completed"}]}),
                "selected"
            )
            .is_err()
        );
        assert!(image_results(&json!({"output": [{"type": "web_search_call", "status": "in_progress", "results": [image_result()]}]}), "selected").is_err());
    }

    #[test]
    fn results_and_captions_are_bounded() {
        let results = (0..50)
            .map(|index| {
                let mut result = image_result();
                result["image_url"] = json!(format!("https://cdn.example.com/{index}.png"));
                result["caption"] = json!("猫".repeat(1000));
                result
            })
            .collect();
        let candidates = image_results(&completed_images(results), "selected").unwrap();
        assert_eq!(candidates.len(), MAX_RESULTS);
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.title.chars().count() == 300)
        );
    }

    #[tokio::test]
    async fn transport_network_failures_are_distinct_and_never_trigger_a_retry_or_model_list() {
        let transport = Arc::new(MockTransport::default());
        transport.responses.lock().unwrap().push_back(Err(
            "OpenAIに接続できませんでした。通信状態を確認して再試行してください。".to_owned(),
        ));
        let credentials = credentials(OpenAiAuthMode::Api);
        let client = OpenAiClient {
            credentials: credentials.clone(),
            transport: transport.clone(),
        };
        let error = client.search("cats").await.err().unwrap();
        assert!(error.contains("通信状態"));
        assert!(!error.contains("APIキー"));
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
        assert!(credentials.writes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn response_and_model_catalog_size_limits_are_enforced_for_injected_transport() {
        for (status, size) in [(200, MAX_RESPONSE_BYTES + 1), (400, MAX_ERROR_BYTES + 1)] {
            let (client, _) = client(
                credentials(OpenAiAuthMode::Api),
                vec![response(status, vec![b'x'; size])],
            );
            assert!(
                client
                    .search("cats")
                    .await
                    .err()
                    .unwrap()
                    .contains("サイズ上限")
            );
        }
        let (client, _) = client(
            credentials(OpenAiAuthMode::Api),
            vec![response(200, vec![b'x'; MAX_MODELS_BYTES + 1])],
        );
        assert!(client.models().await.unwrap_err().contains("サイズ上限"));
        let mut decoder = SseDecoder::default();
        assert!(
            decoder
                .push(&vec![b'x'; MAX_RESPONSE_BYTES + 1], |_, _| Ok(()))
                .unwrap_err()
                .contains("サイズ上限")
        );
    }

    #[tokio::test]
    async fn invalid_query_and_corrupt_settings_do_not_read_keys_or_make_requests() {
        let credentials = credentials(OpenAiAuthMode::Api);
        let (client, transport) = client(credentials.clone(), vec![]);
        for query in ["", "  ", &"x".repeat(401), &"word ".repeat(51)] {
            assert!(client.search(query).await.is_err());
        }
        assert!(credentials.reads.lock().unwrap().is_empty());
        credentials
            .records
            .lock()
            .unwrap()
            .insert(CredentialKind::OpenAiSettings, "corrupt".to_owned());
        assert!(client.search("cats").await.is_err());
        assert_eq!(
            *credentials.reads.lock().unwrap(),
            vec![CredentialKind::OpenAiSettings]
        );
        assert!(transport.requests.lock().unwrap().is_empty());
        assert!(credentials.writes.lock().unwrap().is_empty());
    }

    async fn local_response(raw: Vec<u8>) -> Response {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let mut received = Vec::new();
            while !received.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = stream.read(&mut request).await.unwrap();
                assert_ne!(count, 0);
                received.extend_from_slice(&request[..count]);
                assert!(received.len() <= 16 * 1024);
            }
            stream.write_all(&raw).await.unwrap();
        });
        http_client()
            .unwrap()
            .get(format!("http://{address}/fixture"))
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn production_http_client_does_not_follow_redirects_and_reader_bounds_known_or_chunked_bodies()
     {
        let redirect = local_response(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/credential-leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()).await;
        assert_eq!(redirect.status(), StatusCode::FOUND);
        let oversized = local_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 101\r\nConnection: close\r\n\r\n".to_vec(),
        )
        .await;
        assert!(
            read_http_response(oversized, 100)
                .await
                .err()
                .unwrap()
                .contains("サイズ上限")
        );
        let chunked = local_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nabcde\r\n5\r\nfghij\r\n0\r\n\r\n".to_vec()).await;
        assert!(
            read_http_response(chunked, 9)
                .await
                .err()
                .unwrap()
                .contains("サイズ上限")
        );
        let bounded = local_response(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX-Request-Id: req_local\r\nConnection: close\r\n\r\nabcde".to_vec()).await;
        let bounded = read_http_response(bounded, 5).await.unwrap();
        assert_eq!(bounded.chunks.concat(), b"abcde");
        assert_eq!(bounded.request_id.as_deref(), Some("req_local"));
    }
}
