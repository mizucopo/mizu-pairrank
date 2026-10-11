//! Native public-client SIWC. Protocol contracts:
//! https://developers.openai.com/siwc/token-sharing-open-source/sign-in
//! https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions
//! Tokens stay in the native credential store; this module never reads another app's credentials.

use crate::credentials::{CredentialKind, CredentialStore};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, Response, StatusCode, redirect::Policy};
use ring::{digest, rand as secure_rand, signature};
use secure_rand::SecureRandom;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::HashSet,
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, OwnedMutexGuard},
    time::{sleep, timeout},
};
use url::Url;

const AUTH_ORIGIN: &str = "https://auth.openai.com";
const DISCOVERY_URL: &str = "https://auth.openai.com/.well-known/openid-configuration";
const RESOURCE: &str = "https://api.openai.com/v1";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const CALLBACK_PATH: &str = "/auth/callback";
const MAX_HTTP_BODY: usize = 128 * 1024;
const MAX_CALLBACK_HEADER: usize = 16 * 1024;
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(180);
// Commands create clients using their current credential store. The gate is shared across them.
static SESSION_GATE: LazyLock<Arc<Mutex<()>>> = LazyLock::new(|| Arc::new(Mutex::new(())));

#[derive(Clone, Default, Serialize)]
pub(crate) struct ChatGptStatus {
    pub connected: bool,
    pub sharing: bool,
    pub account: Option<String>,
}

pub(crate) fn read_status(credentials: &dyn CredentialStore) -> Result<ChatGptStatus, String> {
    let session = load_session_record(credentials)?;
    Ok(ChatGptStatus {
        connected: session.as_ref().is_some_and(Session::connected),
        sharing: session.as_ref().is_some_and(Session::sharing),
        account: session.and_then(|session| session.email),
    })
}

#[derive(Clone, Serialize, Deserialize)]
struct Session {
    issuer: String,
    subject: String,
    client_id: String,
    email: Option<String>,
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    scopes: Vec<String>,
    expires_at: u64,
}

impl Session {
    fn connected(&self) -> bool {
        self.id_token
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    }

    fn sharing(&self) -> bool {
        self.connected()
            && self
                .access_token
                .as_ref()
                .is_some_and(|value| !value.is_empty())
            && self.scopes.iter().any(|scope| scope == PLAN_SCOPE)
    }

    fn clear_tokens(&mut self) {
        self.id_token = None;
        self.access_token = None;
        self.refresh_token = None;
        self.scopes.clear();
        self.expires_at = 0;
    }
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    revocation_endpoint: Option<String>,
}

#[derive(Deserialize)]
struct Tokens {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
}

#[derive(Deserialize)]
struct JwtHeader {
    alg: String,
    kid: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn contains(&self, expected: &str) -> bool {
        match self {
            Self::One(value) => value == expected,
            Self::Many(values) => values.iter().any(|value| value == expected),
        }
    }
}

#[derive(Deserialize)]
struct Identity {
    iss: String,
    aud: Audience,
    sub: String,
    exp: u64,
    iat: u64,
    nbf: Option<u64>,
    nonce: String,
    email: Option<String>,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    alg: Option<String>,
    #[serde(rename = "use")]
    usage: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

struct TokenFailure {
    code: Option<String>,
    message: String,
}

struct Callback {
    code: String,
    client_id: String,
}

pub(crate) struct ChatGptClient {
    credentials: Arc<dyn CredentialStore>,
    http: Result<Client, String>,
    discovery_url: String,
    trusted_origin: String,
    endpoint_origin: String,
}

impl ChatGptClient {
    pub(crate) fn new(credentials: Arc<dyn CredentialStore>) -> Self {
        Self {
            credentials,
            http: Client::builder()
                .redirect(Policy::none())
                .timeout(Duration::from_secs(20))
                .build()
                .map_err(|_| "ChatGPT接続のHTTPクライアントを作成できませんでした。".to_owned()),
            discovery_url: DISCOVERY_URL.to_owned(),
            trusted_origin: AUTH_ORIGIN.to_owned(),
            endpoint_origin: AUTH_ORIGIN.to_owned(),
        }
    }

    pub(crate) async fn connect(
        &self,
        new_account: bool,
        open_browser: impl FnOnce(String) -> Result<(), String> + Send + 'static,
    ) -> Result<(), String> {
        let guard = Arc::clone(&SESSION_GATE).try_lock_owned().map_err(|_| {
            "ChatGPTの接続処理が進行中です。完了してから再試行してください。".to_owned()
        })?;
        let (guard, previous) = if new_account {
            // A new registration can recover from an unreadable remembered record.
            // The existing record remains untouched until the new identity is verified.
            (guard, None)
        } else {
            self.load_session_async(guard).await?
        };
        let returning = previous.as_ref().filter(|_| !new_account);
        let (guard, host_id) = self.host_id_async(guard).await?;
        let discovery = self.discovery().await?;
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|_| "ChatGPTサインインのローカル受信口を開けませんでした。".to_owned())?;
        let port = listener
            .local_addr()
            .map_err(|_| "ChatGPTサインインのローカル受信口を確認できませんでした。".to_owned())?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
        let state = random_value(32)?;
        let nonce = random_value(32)?;
        let verifier = random_value(64)?;
        let challenge =
            URL_SAFE_NO_PAD.encode(digest::digest(&digest::SHA256, verifier.as_bytes()));
        let mut authorization = Url::parse(&discovery.authorization_endpoint)
            .map_err(|_| "ChatGPTの認証先が不正です。".to_owned())?;
        {
            let mut query = authorization.query_pairs_mut();
            query.extend_pairs([
                (
                    "client_id",
                    returning.map_or("dynamic_agent_client", |s| s.client_id.as_str()),
                ),
                ("ext_agent_host_id", host_id.as_str()),
                ("response_type", "code"),
                ("redirect_uri", redirect_uri.as_str()),
                ("scope", SCOPES),
                ("resource", RESOURCE),
                ("state", state.as_str()),
                ("nonce", nonce.as_str()),
                ("code_challenge_method", "S256"),
                ("code_challenge", challenge.as_str()),
            ]);
            if let Some(session) = returning {
                if let Some(id_token) = &session.id_token {
                    query.append_pair("id_token_hint", id_token);
                }
                if let Some(email) = &session.email {
                    query.append_pair("login_hint", email);
                }
                if session.connected() && !session.scopes.iter().any(|scope| scope == PLAN_SCOPE) {
                    // An explicit reconnect can enable a previously declined plan permission.
                    query.append_pair("prompt", "consent");
                }
            } else {
                query.append_pair("agent_name_hint", "mizu-pairrank");
            }
        }
        // Authorization URLs contain an ID-token hint on reconnect; never log them.
        let authorization_url = authorization.to_string();
        let (guard, ()) =
            serialized_blocking(guard, move || open_browser(authorization_url)).await?;
        let callback = timeout(
            SIGN_IN_TIMEOUT,
            await_callback(
                &listener,
                &state,
                returning.map(|session| session.client_id.as_str()),
            ),
        )
        .await
        .map_err(|_| {
            "ChatGPTサインインが時間切れになりました。再試行してください。".to_owned()
        })??;
        let tokens = self
            .token_request(
                &discovery.token_endpoint,
                &[
                    ("grant_type", "authorization_code"),
                    ("client_id", &callback.client_id),
                    ("code", &callback.code),
                    ("code_verifier", &verifier),
                    ("redirect_uri", &redirect_uri),
                    ("resource", RESOURCE),
                ],
            )
            .await
            .map_err(|failure| failure.message)?;
        let id_token = nonempty(tokens.id_token.as_deref()).ok_or_else(|| {
            "ChatGPTの本人確認トークンがありません。再試行してください。".to_owned()
        })?;
        let jwks: Jwks = self.get_json(&discovery.jwks_uri).await?;
        let identity = verify_identity(
            id_token,
            &jwks,
            &discovery.issuer,
            &callback.client_id,
            &nonce,
            now()?,
        )?;
        if returning.is_some_and(|session| {
            session.subject != identity.sub || session.issuer != identity.iss
        }) {
            return Err("選択したChatGPTアカウントとサインインしたアカウントが一致しません。新しいアカウントとして接続してください。".to_owned());
        }
        let scopes = parse_scopes(tokens.scope.as_deref().unwrap_or_default());
        validate_access_fields(&tokens, scopes.iter().any(|scope| scope == PLAN_SCOPE))?;
        let session = Session {
            issuer: identity.iss,
            subject: identity.sub,
            client_id: callback.client_id,
            email: identity.email,
            id_token: tokens.id_token,
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            scopes,
            expires_at: expiry(tokens.expires_in.unwrap_or_default())?,
        };
        // Replace the remembered registration only after successful identity validation.
        let _guard = self.save_session_async(guard, &session).await?;
        Ok(())
    }

    pub(crate) async fn disconnect(&self) -> Result<(), String> {
        let guard = Arc::clone(&SESSION_GATE).lock_owned().await;
        let (guard, session) = self.load_session_async(guard).await?;
        let Some(mut session) = session else {
            return Ok(());
        };
        let revocation = if let Some(token) = &session.refresh_token {
            self.revoke(&session.client_id, token).await
        } else {
            Ok(())
        };
        session.clear_tokens();
        let _guard = self.save_session_async(guard, &session).await?;
        revocation.map_err(|_| "このアプリからサインアウトしましたが、ChatGPT側のセッション失効を確認できませんでした。ChatGPTの設定からアプリの接続を解除してください。".to_owned())
    }

    pub(crate) async fn access_token(&self) -> Result<String, String> {
        let guard = Arc::clone(&SESSION_GATE).lock_owned().await;
        let (guard, session) = self.load_session_async(guard).await?;
        let mut session = session.ok_or_else(|| "先にChatGPTに接続してください。".to_owned())?;
        if !session.sharing() {
            return Err("ChatGPTプランの使用が有効になっていません。プランの使用を許可して接続し直してください。".to_owned());
        }
        if session.expires_at > now()?.saturating_add(60) {
            return session
                .access_token
                .ok_or_else(|| "ChatGPTに接続し直してください。".to_owned());
        }
        let Some(refresh_token) = session
            .refresh_token
            .as_deref()
            .filter(|token| !token.is_empty())
        else {
            session.clear_tokens();
            let _guard = self.save_session_async(guard, &session).await?;
            return Err("ChatGPTの接続を更新できません。サインインし直してください。".to_owned());
        };
        let discovery = self.discovery().await?;
        let tokens = match self
            .token_request(
                &discovery.token_endpoint,
                &[
                    ("grant_type", "refresh_token"),
                    ("client_id", &session.client_id),
                    ("refresh_token", refresh_token),
                    ("resource", RESOURCE),
                ],
            )
            .await
        {
            Ok(tokens) => tokens,
            Err(failure) => {
                if failure.code.as_deref().is_some_and(terminal_refresh_error) {
                    session.clear_tokens();
                    let _guard = self.save_session_async(guard, &session).await?;
                }
                return Err(failure.message);
            }
        };
        validate_access_fields(&tokens, true)?;
        let scopes = tokens
            .scope
            .as_deref()
            .map_or_else(|| session.scopes.clone(), parse_scopes);
        session.access_token = tokens.access_token;
        session.refresh_token = tokens.refresh_token;
        session.expires_at = expiry(tokens.expires_in.unwrap_or_default())?;
        session.scopes = scopes;
        // Keep the previously verified ID token for the account hint. It may be expired.
        let _guard = self.save_session_async(guard, &session).await?;
        if !session.sharing() {
            return Err(
                "ChatGPTプランの使用許可がなくなりました。接続し直してください。".to_owned(),
            );
        }
        session
            .access_token
            .ok_or_else(|| "ChatGPTに接続し直してください。".to_owned())
    }

    fn http(&self) -> Result<&Client, String> {
        self.http.as_ref().map_err(Clone::clone)
    }

    async fn load_session_async(
        &self,
        guard: OwnedMutexGuard<()>,
    ) -> Result<(OwnedMutexGuard<()>, Option<Session>), String> {
        let credentials = Arc::clone(&self.credentials);
        serialized_blocking(guard, move || load_session_record(credentials.as_ref())).await
    }

    async fn save_session_async(
        &self,
        guard: OwnedMutexGuard<()>,
        session: &Session,
    ) -> Result<OwnedMutexGuard<()>, String> {
        let credentials = Arc::clone(&self.credentials);
        let session = session.clone();
        let (guard, ()) = serialized_blocking(guard, move || {
            save_session_record(credentials.as_ref(), &session)
        })
        .await?;
        Ok(guard)
    }

    async fn host_id_async(
        &self,
        guard: OwnedMutexGuard<()>,
    ) -> Result<(OwnedMutexGuard<()>, String), String> {
        let credentials = Arc::clone(&self.credentials);
        serialized_blocking(guard, move || host_id_record(credentials.as_ref())).await
    }

    #[cfg(test)]
    fn load_session(&self) -> Result<Option<Session>, String> {
        load_session_record(self.credentials.as_ref())
    }

    #[cfg(test)]
    fn save_session(&self, session: &Session) -> Result<(), String> {
        save_session_record(self.credentials.as_ref(), session)
    }

    #[cfg(test)]
    fn host_id(&self) -> Result<String, String> {
        host_id_record(self.credentials.as_ref())
    }

    async fn discovery(&self) -> Result<Discovery, String> {
        let discovery: Discovery = self.get_json(&self.discovery_url).await?;
        if discovery.issuer != self.trusted_origin {
            return Err("ChatGPT認証の発行元が一致しません。".to_owned());
        }
        for endpoint in [
            &discovery.authorization_endpoint,
            &discovery.token_endpoint,
            &discovery.jwks_uri,
        ] {
            self.validate_endpoint(endpoint)?;
        }
        if let Some(endpoint) = &discovery.revocation_endpoint {
            self.validate_endpoint(endpoint)?;
        }
        Ok(discovery)
    }

    fn validate_endpoint(&self, endpoint: &str) -> Result<(), String> {
        let url = Url::parse(endpoint).map_err(|_| "ChatGPT認証の接続先が不正です。".to_owned())?;
        if url.origin().ascii_serialization() != self.endpoint_origin
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("ChatGPT認証の接続先が許可された発行元ではありません。".to_owned());
        }
        Ok(())
    }

    async fn get_json<T: DeserializeOwned>(&self, endpoint: &str) -> Result<T, String> {
        self.validate_endpoint(endpoint)?;
        let response = self
            .http()?
            .get(endpoint)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|_| {
                "ChatGPT認証サービスに接続できませんでした。再試行してください。".to_owned()
            })?;
        if !response.status().is_success() {
            return Err(format!(
                "ChatGPT認証サービスの応答を確認できませんでした (HTTP {})。",
                response.status().as_u16()
            ));
        }
        read_json(response).await
    }

    async fn token_request(
        &self,
        endpoint: &str,
        form: &[(&str, &str)],
    ) -> Result<Tokens, TokenFailure> {
        let request = async {
            self.validate_endpoint(endpoint)?;
            self.http()?
                .post(endpoint)
                .header("Accept", "application/json")
                .form(form)
                .send()
                .await
                .map_err(|_| {
                    "ChatGPTの接続を更新できませんでした。通信状態を確認して再試行してください。"
                        .to_owned()
                })
        }
        .await;
        let response = request.map_err(|message| TokenFailure {
            code: None,
            message,
        })?;
        let status = response.status();
        if status.is_success() {
            return read_json(response).await.map_err(|message| TokenFailure {
                code: None,
                message,
            });
        }
        let body: serde_json::Value = read_json(response).await.unwrap_or(serde_json::Value::Null);
        let code = body
            .get("error")
            .and_then(|error| {
                error
                    .as_str()
                    .or_else(|| error.get("code").and_then(serde_json::Value::as_str))
            })
            .filter(|code| {
                code.len() <= 80
                    && code
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
            .map(str::to_owned);
        let message = if code.as_deref().is_some_and(terminal_refresh_error) {
            "ChatGPTの接続が失効しました。サインインし直してください。".to_owned()
        } else if code.as_deref() == Some("invalid_client") {
            "ChatGPTのクライアント登録が無効です。新しいアカウントとして接続してください。"
                .to_owned()
        } else {
            format!(
                "ChatGPT認証に失敗しました (HTTP {}{})。再試行してください。",
                status.as_u16(),
                code.as_ref()
                    .map_or_else(String::new, |code| format!(", {code}"))
            )
        };
        Err(TokenFailure { code, message })
    }

    async fn revoke(&self, client_id: &str, token: &str) -> Result<(), String> {
        let discovery = self.discovery().await?;
        let endpoint = discovery
            .revocation_endpoint
            .ok_or_else(|| "失効処理の接続先がありません。".to_owned())?;
        for attempt in 0..3 {
            let result = self
                .http()?
                .post(&endpoint)
                .form(&[
                    ("token", token),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", client_id),
                ])
                .send()
                .await;
            match result {
                Ok(response) if response.status() == StatusCode::OK => return Ok(()),
                Ok(response) if !response.status().is_server_error() => break,
                _ if attempt < 2 => sleep(Duration::from_millis(150 * (1 << attempt))).await,
                _ => break,
            }
        }
        Err("ChatGPT側の失効を確認できませんでした。".to_owned())
    }
}

async fn serialized_blocking<T: Send + 'static>(
    guard: OwnedMutexGuard<()>,
    operation: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<(OwnedMutexGuard<()>, T), String> {
    // The worker owns the guard until it returns. Dropping an awaiting command cannot
    // release serialization while an OS credential prompt/write is still in progress.
    let (guard, result) = tokio::task::spawn_blocking(move || {
        let result = operation();
        (guard, result)
    })
    .await
    .map_err(|_| "ChatGPT接続のローカル処理を完了できませんでした。".to_owned())?;
    Ok((guard, result?))
}

fn save_session_record(credentials: &dyn CredentialStore, session: &Session) -> Result<(), String> {
    let record = serde_json::to_string(session)
        .map_err(|_| "ChatGPT接続を保存できませんでした。".to_owned())?;
    credentials.set(CredentialKind::ChatGptSession, &record)
}

fn host_id_record(credentials: &dyn CredentialStore) -> Result<String, String> {
    if let Some(host) = credentials.get(CredentialKind::ChatGptHost)? {
        if host
            .strip_prefix("urn:uuid:")
            .and_then(|value| uuid::Uuid::parse_str(value).ok())
            .is_none()
        {
            return Err("保存されたChatGPTホストIDが不正です。".to_owned());
        }
        return Ok(host);
    }
    let host = format!("urn:uuid:{}", uuid::Uuid::new_v4());
    credentials.set(CredentialKind::ChatGptHost, &host)?;
    Ok(host)
}

fn load_session_record(credentials: &dyn CredentialStore) -> Result<Option<Session>, String> {
    credentials
        .get(CredentialKind::ChatGptSession)?
        .map(|record| {
            if record.len() > MAX_HTTP_BODY {
                return Err("保存されたChatGPT接続の形式が不正です。".to_owned());
            }
            let session: Session = serde_json::from_str(&record)
                .map_err(|_| "保存されたChatGPT接続を読み取れませんでした。".to_owned())?;
            if session.issuer != AUTH_ORIGIN
                || session.subject.is_empty()
                || !issued_client_id(&session.client_id)
            {
                return Err("保存されたChatGPT接続の形式が不正です。".to_owned());
            }
            Ok(session)
        })
        .transpose()
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

fn parse_scopes(value: &str) -> Vec<String> {
    value.split_whitespace().map(str::to_owned).collect()
}

fn validate_access_fields(tokens: &Tokens, required: bool) -> Result<(), String> {
    if required
        && (nonempty(tokens.access_token.as_deref()).is_none()
            || nonempty(tokens.refresh_token.as_deref()).is_none()
            || !tokens
                .token_type
                .as_deref()
                .is_some_and(|kind| kind.eq_ignore_ascii_case("Bearer"))
            || tokens.expires_in.is_none_or(|seconds| seconds == 0))
    {
        return Err("ChatGPTの接続トークンの応答が不完全です。接続し直してください。".to_owned());
    }
    Ok(())
}

fn terminal_refresh_error(code: &str) -> bool {
    matches!(
        code,
        "invalid_grant"
            | "invalid_refresh_token"
            | "token_expired"
            | "refresh_token_expired"
            | "refresh_token_invalidated"
            | "refresh_token_reused"
    )
}

fn now() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| "端末の日時を確認してください。".to_owned())
}

fn expiry(seconds: u64) -> Result<u64, String> {
    now()?
        .checked_add(seconds)
        .ok_or_else(|| "ChatGPTトークンの有効期限が不正です。".to_owned())
}

fn random_value(bytes: usize) -> Result<String, String> {
    let mut value = vec![0_u8; bytes];
    secure_rand::SystemRandom::new()
        .fill(&mut value)
        .map_err(|_| "安全なサインイン情報を生成できませんでした。".to_owned())?;
    Ok(URL_SAFE_NO_PAD.encode(value))
}

fn issued_client_id(value: &str) -> bool {
    !value.is_empty()
        && value != "dynamic_agent_client"
        && value.len() <= 512
        && !value.chars().any(char::is_control)
}

async fn read_json<T: DeserializeOwned>(mut response: Response) -> Result<T, String> {
    if response
        .content_length()
        .is_some_and(|size| size > MAX_HTTP_BODY as u64)
    {
        return Err("ChatGPT認証サービスの応答が大きすぎます。".to_owned());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "ChatGPT認証サービスの応答を受信できませんでした。".to_owned())?
    {
        if body.len().saturating_add(chunk.len()) > MAX_HTTP_BODY {
            return Err("ChatGPT認証サービスの応答が大きすぎます。".to_owned());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|_| "ChatGPT認証サービスの応答形式が不正です。".to_owned())
}

fn verify_identity(
    token: &str,
    jwks: &Jwks,
    issuer: &str,
    client_id: &str,
    nonce: &str,
    time: u64,
) -> Result<Identity, String> {
    let invalid = || "ChatGPTの本人確認を検証できませんでした。接続し直してください。".to_owned();
    if token.len() > 32 * 1024 {
        return Err(invalid());
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(invalid());
    }
    let header: JwtHeader =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).map_err(|_| invalid())?)
            .map_err(|_| invalid())?;
    // Fail closed for algorithms this native client does not support; never accept "none" or HMAC.
    if header.alg != "RS256" || header.kid.is_empty() {
        return Err(invalid());
    }
    let matching: Vec<_> = jwks
        .keys
        .iter()
        .filter(|key| key.kid.as_deref() == Some(&header.kid))
        .collect();
    if matching.len() != 1 {
        return Err(invalid());
    }
    let key = matching[0];
    if key.kty != "RSA"
        || key
            .alg
            .as_deref()
            .is_some_and(|algorithm| algorithm != "RS256")
        || key.usage.as_deref().is_some_and(|usage| usage != "sig")
    {
        return Err(invalid());
    }
    let n = URL_SAFE_NO_PAD
        .decode(key.n.as_deref().ok_or_else(invalid)?)
        .map_err(|_| invalid())?;
    let e = URL_SAFE_NO_PAD
        .decode(key.e.as_deref().ok_or_else(invalid)?)
        .map_err(|_| invalid())?;
    let signature_bytes = URL_SAFE_NO_PAD.decode(parts[2]).map_err(|_| invalid())?;
    let signed = format!("{}.{}", parts[0], parts[1]);
    signature::RsaPublicKeyComponents { n: &n, e: &e }
        .verify(
            &signature::RSA_PKCS1_2048_8192_SHA256,
            signed.as_bytes(),
            &signature_bytes,
        )
        .map_err(|_| invalid())?;
    let identity: Identity =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).map_err(|_| invalid())?)
            .map_err(|_| invalid())?;
    if identity.iss != issuer
        || !identity.aud.contains(client_id)
        || identity.sub.is_empty()
        || identity.exp <= time.saturating_sub(5)
        || identity.iat > time.saturating_add(5)
        || identity
            .nbf
            .is_some_and(|not_before| not_before > time.saturating_add(5))
        || identity.nonce != nonce
    {
        return Err(invalid());
    }
    Ok(identity)
}

fn callback_from_request(
    request: &str,
    expected_state: &str,
    returning_client: Option<&str>,
) -> Result<Callback, String> {
    let mut line = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let (method, target, version) = (line.next(), line.next(), line.next());
    let Some(target) = target else {
        return Err("invalid_callback".to_owned());
    };
    if method != Some("GET")
        || !matches!(version, Some("HTTP/1.0" | "HTTP/1.1"))
        || line.next().is_some()
        || !target.starts_with('/')
        || target.starts_with("//")
        || target.contains('#')
    {
        return Err("invalid_callback".to_owned());
    }
    let url = Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|_| "invalid_callback".to_owned())?;
    if url.path() != CALLBACK_PATH {
        return Err("invalid_callback".to_owned());
    }
    let mut seen = HashSet::new();
    let pairs: Vec<_> = url.query_pairs().collect();
    if pairs.len() > 32 || pairs.iter().any(|(key, _)| !seen.insert(key.as_ref())) {
        return Err("invalid_callback".to_owned());
    }
    let value = |name: &str| {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_ref())
    };
    if value("state") != Some(expected_state) {
        return Err("invalid_callback".to_owned());
    }
    if value("error").is_some() {
        return Err(
            "ChatGPTサインインがキャンセルされました。必要な場合は再試行してください。".to_owned(),
        );
    }
    let code = value("code")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "ChatGPTの認証コードがありません。".to_owned())?;
    let client_id = match (returning_client, value("client_id")) {
        (Some(expected), Some(returned)) if expected != returned => {
            return Err("ChatGPTのクライアント登録が一致しません。".to_owned());
        }
        (Some(expected), _) => expected,
        (None, Some(returned)) if issued_client_id(returned) => returned,
        _ => {
            return Err(
                "ChatGPTのクライアント登録が完了しませんでした。再試行してください。".to_owned(),
            );
        }
    };
    Ok(Callback {
        code: code.to_owned(),
        client_id: client_id.to_owned(),
    })
}

async fn read_callback_header(stream: &mut TcpStream) -> Result<String, String> {
    let mut header = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let received = stream
            .read(&mut buffer)
            .await
            .map_err(|_| "invalid_callback".to_owned())?;
        if received == 0 || header.len() + received > MAX_CALLBACK_HEADER {
            return Err("invalid_callback".to_owned());
        }
        header.extend_from_slice(&buffer[..received]);
        if header.windows(4).any(|window| window == b"\r\n\r\n") {
            return String::from_utf8(header).map_err(|_| "invalid_callback".to_owned());
        }
    }
}

async fn await_callback(
    listener: &TcpListener,
    state: &str,
    returning_client: Option<&str>,
) -> Result<Callback, String> {
    loop {
        let (mut stream, peer) = listener
            .accept()
            .await
            .map_err(|_| "ChatGPTサインインの応答を受信できませんでした。".to_owned())?;
        if !peer.ip().is_loopback() {
            continue;
        }
        let result = timeout(Duration::from_secs(2), read_callback_header(&mut stream)).await;
        let callback = result.ok().and_then(Result::ok).map_or_else(
            || Err("invalid_callback".to_owned()),
            |request| callback_from_request(&request, state, returning_client),
        );
        let invalid = callback
            .as_ref()
            .is_err_and(|error| error == "invalid_callback");
        let (status, body) = if invalid {
            ("400 Bad Request", "Invalid sign-in callback.")
        } else {
            (
                "200 OK",
                "Authorization received. Return to mizu-pairrank to check the connection.",
            )
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
            body.len()
        );
        let _ = timeout(
            Duration::from_secs(2),
            stream.write_all(response.as_bytes()),
        )
        .await;
        if !invalid {
            return callback;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;
    use serde_json::{Value, json};
    use std::{
        collections::HashMap,
        sync::{
            Mutex as StdMutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    static TEST_GATE: Mutex<()> = Mutex::const_new(());

    // Generated exclusively for these local fixtures. This is not an account credential.
    const TEST_RSA: &str = "MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDeITUoxB4kJelP77RZ4cXznBIdJXnG1AAr59QUfKaAALhYnkazlI5DtEYaLv8+zxu9gtcZqHN5jYzzIOH1XbD5X6NFkZm8gtuBpKgnfV8e36IO5KO+9Ik6Tmcbuj1qwTR8ZpIAT41ZKZT0TAhKAZdjQzdWme38YDn2DvLVLp5tjNi19CWXO3vkiHUk+byN84DNAqytv65XwKjNUe4FXubO27VKPr0j1HDDeFu0RER7ZBIQhZ3lI4CLr8KhYUe5XA/bxuRUXQE3ANzAuzeNfK8CqJV5zH1oIul6IFVOmQ/NZHb3aUHL6hqj6d0Xyjo0gLjHXkaUJ+Qgsx9LkZKCIhN1AgMBAAECggEBAJ5DfsKq6ObMdC1WFmOr7wQhAO7vUpKNXgnqRTrLjT3KRY4uRyWQBMTMXt4s+gNgQN2HqYZIdO34IqLfnN7bgEpND+FzWdvsSEVYcc6hDvLKV4bQ9LILPPLkBabFzRiQy7qOwhuiko0iiVR+D7Ha7Gf27iivx9Z3IRTllbK6Ynqd+cKqpgROUk1UJ7xhY3g9gqQazn+BI/thEE8FIA8EXKx1ShlMJyhuWeDuwLCHFH9tErS1rFzrH8b9o++DxgPyC0yR3A4AiT4TkoEopytiufbHs10KJPl3yObO0wqk5asFpNolXYxmcUtE4Gbm97U6mv7limr8ssKsVyOeSqmIGcECgYEA7rImdMT7zJWXXrokwFJBF/92CghHgAfOj6dfnkoKXgihvRLPe7uii4tKuyF5A7dWqlPM8l3AdeRfnet2fLEEKYLgxnqoE7LG9UbWK/avQllyPBeNWXM86dQnsiio+mnA7fuWD5H6aBHKqz1hWWkgEPBqZ8h/zgUYvmNBiv2Q7ZkCgYEA7juc9V6IgASpi8inQxSDeW2QkYc9rKjBquI4ApTJMVLrLAiwe9u2xIPHSvwJj752apbbbFhh+WTeSeXH8BtYa1alC/066dFkuCh78aJ7EeJoWUrAV07Reyen5xy6QS8TixcSY6LLcuumcyxEVBGHqvp3IAuezOXeVGhX2Ity5j0CgYB+TmjJC19Oj63Mo2VFG4F0aGqWvnDPT9I+88tGIFnCDoE/Yo/GIrDNdISz5u95Ru+mK7fcXD22ZIUwNGO7cZQJP0cf39HGsqhTLAuysdNloZ0wLSpPtgn6lZf3z0ozT/iWNbXfQVim7xs2Z8F2pYkhTYaQqmjupIXGYRFSsgCZqQKBgQC7ZHr3a0/V21T+VmxzUIyGlI+WWZpC2U/VujIynZSQztecAI3KL67Zb4rxScW/oyMEfvvEgtU2Voif+jZGgUwQysvy3VNdHs+qKHYuQQ+rprtMGYeoZaBTzoK9QZSkt4IY86IMa766+/AnQlfRBSUBchkIRPu50ssK/DS1aiOMRQKBgFJeaYF4CBvVg+4gHC4pexxvxDLt+58qnQIZk7VPawkmhtfrZTJ09oDQvP50xYwFDqhufhMS17iS823myTxaIjyy5SBkp0L8cSY0DVV8485X0YyP9iCsK0n/KZwM0A6QKS9/UVmowUZL1mFFpW+Gan3pi8vmlH9lPltPJu9bVkx4";
    const TEST_N: &str = "3iE1KMQeJCXpT--0WeHF85wSHSV5xtQAK-fUFHymgAC4WJ5Gs5SOQ7RGGi7_Ps8bvYLXGahzeY2M8yDh9V2w-V-jRZGZvILbgaSoJ31fHt-iDuSjvvSJOk5nG7o9asE0fGaSAE-NWSmU9EwISgGXY0M3Vpnt_GA59g7y1S6ebYzYtfQllzt75Ih1JPm8jfOAzQKsrb-uV8CozVHuBV7mztu1Sj69I9Rww3hbtEREe2QSEIWd5SOAi6_CoWFHuVwP28bkVF0BNwDcwLs3jXyvAqiVecx9aCLpeiBVTpkPzWR292lBy-oao-ndF8o6NIC4x15GlCfkILMfS5GSgiITdQ";

    #[derive(Default)]
    struct MemoryCredentials {
        records: StdMutex<HashMap<CredentialKind, String>>,
        session_writes: AtomicUsize,
    }

    impl CredentialStore for MemoryCredentials {
        fn get(&self, kind: CredentialKind) -> Result<Option<String>, String> {
            Ok(self.records.lock().unwrap().get(&kind).cloned())
        }
        fn set(&self, kind: CredentialKind, value: &str) -> Result<(), String> {
            if kind == CredentialKind::ChatGptSession {
                self.session_writes.fetch_add(1, Ordering::SeqCst);
            }
            self.records.lock().unwrap().insert(kind, value.to_owned());
            Ok(())
        }
    }

    fn saved_session() -> Session {
        Session {
            issuer: AUTH_ORIGIN.to_owned(),
            subject: "test-subject".to_owned(),
            client_id: "oaiapp_test".to_owned(),
            email: Some("test@example.invalid".to_owned()),
            id_token: Some("retained-fixture-id-token".to_owned()),
            access_token: Some("old-access".to_owned()),
            refresh_token: Some("old-refresh".to_owned()),
            scopes: parse_scopes(SCOPES),
            expires_at: 0,
        }
    }

    fn jwks_json() -> Value {
        json!({"keys":[{"kid":"test-key","kty":"RSA","alg":"RS256","use":"sig","n":TEST_N,"e":"AQAB"}]})
    }

    fn signed_token(client: &str, nonce: &str, overrides: Value) -> String {
        let time = now().unwrap();
        let mut claims = json!({"iss":AUTH_ORIGIN,"aud":client,"sub":"test-subject","exp":time + 3600,"iat":time,"nonce":nonce,"email":"test@example.invalid"});
        for (key, value) in overrides.as_object().unwrap() {
            claims[key] = value.clone();
        }
        let header = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&json!({"alg":"RS256","kid":"test-key"})).unwrap());
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let input = format!("{header}.{payload}");
        let key = signature::RsaKeyPair::from_pkcs8(&STANDARD.decode(TEST_RSA).unwrap()).unwrap();
        let mut signature_bytes = vec![0; key.public().modulus_len()];
        key.sign(
            &signature::RSA_PKCS1_SHA256,
            &secure_rand::SystemRandom::new(),
            input.as_bytes(),
            &mut signature_bytes,
        )
        .unwrap();
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature_bytes))
    }

    struct FixtureServer {
        origin: String,
        token_reply: Arc<StdMutex<(u16, Value)>>,
        revoke_status: Arc<StdMutex<u16>>,
        forms: Arc<StdMutex<Vec<String>>>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for FixtureServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    impl FixtureServer {
        async fn start() -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let token_reply = Arc::new(StdMutex::new((
                200,
                json!({"access_token":"new-access","refresh_token":"new-refresh","token_type":"Bearer","expires_in":3600,"scope":SCOPES}),
            )));
            let revoke_status = Arc::new(StdMutex::new(200));
            let forms = Arc::new(StdMutex::new(Vec::new()));
            let (server_origin, reply, revoke, requests) = (
                origin.clone(),
                token_reply.clone(),
                revoke_status.clone(),
                forms.clone(),
            );
            let task = tokio::spawn(async move {
                loop {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let (server_origin, reply, revoke, requests) = (
                        server_origin.clone(),
                        reply.clone(),
                        revoke.clone(),
                        requests.clone(),
                    );
                    tokio::spawn(async move {
                        let mut bytes = Vec::new();
                        let mut buffer = [0_u8; 2048];
                        let header_end = loop {
                            let count = stream.read(&mut buffer).await.unwrap();
                            if count == 0 {
                                return;
                            }
                            bytes.extend_from_slice(&buffer[..count]);
                            if let Some(end) =
                                bytes.windows(4).position(|window| window == b"\r\n\r\n")
                            {
                                break end + 4;
                            }
                        };
                        let header = String::from_utf8_lossy(&bytes[..header_end]).into_owned();
                        let length: usize = header
                            .lines()
                            .filter_map(|line| line.split_once(':'))
                            .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse().unwrap())
                            .unwrap_or_default();
                        while bytes.len() < header_end + length {
                            let count = stream.read(&mut buffer).await.unwrap();
                            if count == 0 {
                                return;
                            }
                            bytes.extend_from_slice(&buffer[..count]);
                        }
                        let path = header
                            .lines()
                            .next()
                            .unwrap()
                            .split_whitespace()
                            .nth(1)
                            .unwrap();
                        let (status, body) = match path {
                            "/.well-known/openid-configuration" => (
                                200,
                                json!({"issuer":AUTH_ORIGIN,"authorization_endpoint":format!("{server_origin}/authorize"),"token_endpoint":format!("{server_origin}/token"),"jwks_uri":format!("{server_origin}/jwks"),"revocation_endpoint":format!("{server_origin}/revoke")}),
                            ),
                            "/jwks" => (200, jwks_json()),
                            "/token" => {
                                requests.lock().unwrap().push(
                                    String::from_utf8_lossy(
                                        &bytes[header_end..header_end + length],
                                    )
                                    .into_owned(),
                                );
                                reply.lock().unwrap().clone()
                            }
                            "/revoke" => (*revoke.lock().unwrap(), Value::Null),
                            _ => (404, Value::Null),
                        };
                        let body = if path == "/revoke" && status == 200 {
                            String::new()
                        } else {
                            body.to_string()
                        };
                        let response = format!(
                            "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                    });
                }
            });
            Self {
                origin,
                token_reply,
                revoke_status,
                forms,
                task,
            }
        }

        fn client(&self, credentials: Arc<MemoryCredentials>) -> ChatGptClient {
            let mut client = ChatGptClient::new(credentials);
            client.discovery_url = format!("{}/.well-known/openid-configuration", self.origin);
            client.endpoint_origin = self.origin.clone();
            client
        }
    }

    #[test]
    fn verifies_signature_and_each_identity_boundary() {
        let jwks: Jwks = serde_json::from_value(jwks_json()).unwrap();
        let token = signed_token("oaiapp_test", "expected-nonce", json!({}));
        assert!(
            verify_identity(
                &token,
                &jwks,
                AUTH_ORIGIN,
                "oaiapp_test",
                "expected-nonce",
                now().unwrap()
            )
            .is_ok()
        );
        for overrides in [
            json!({"iss":"https://evil.invalid"}),
            json!({"aud":"another-client"}),
            json!({"exp":1}),
            json!({"nonce":"wrong"}),
            json!({"sub":""}),
            json!({"iat":now().unwrap() + 3600}),
            json!({"nbf":now().unwrap() + 3600}),
        ] {
            assert!(
                verify_identity(
                    &signed_token("oaiapp_test", "expected-nonce", overrides),
                    &jwks,
                    AUTH_ORIGIN,
                    "oaiapp_test",
                    "expected-nonce",
                    now().unwrap()
                )
                .is_err()
            );
        }
        let mut tampered = token.into_bytes();
        let position = tampered.iter().position(|byte| *byte == b'.').unwrap() + 2;
        tampered[position] = if tampered[position] == b'A' {
            b'B'
        } else {
            b'A'
        };
        assert!(
            verify_identity(
                &String::from_utf8(tampered).unwrap(),
                &jwks,
                AUTH_ORIGIN,
                "oaiapp_test",
                "expected-nonce",
                now().unwrap()
            )
            .is_err()
        );
        assert!(
            verify_identity(
                "eyJhbGciOiJub25lIiwia2lkIjoidGVzdC1rZXkifQ.e30.",
                &jwks,
                AUTH_ORIGIN,
                "oaiapp_test",
                "expected-nonce",
                now().unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn callback_rejects_duplicates_state_paths_and_registration_substitution() {
        let request = |query: &str| {
            format!("GET {CALLBACK_PATH}?{query} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        };
        assert!(
            callback_from_request(
                &request("state=good&code=code&client_id=oaiapp_test"),
                "good",
                None
            )
            .is_ok()
        );
        assert!(
            callback_from_request(
                &request("state=good&code=code"),
                "good",
                Some("oaiapp_test")
            )
            .is_ok()
        );
        for query in [
            "state=bad&code=code&client_id=oaiapp_test",
            "state=good&state=good&code=code&client_id=oaiapp_test",
            "state=good&code=code&client_id=dynamic_agent_client",
            "state=good&code=code",
            "state=good&code=code&%63ode=other&client_id=oaiapp_test",
        ] {
            assert!(callback_from_request(&request(query), "good", None).is_err());
        }
        assert!(
            callback_from_request(
                &request("state=good&code=code&client_id=another"),
                "good",
                Some("oaiapp_test")
            )
            .is_err()
        );
        assert!(
            callback_from_request(
                "GET /callback?state=good&code=code&client_id=oaiapp_test HTTP/1.1\r\n\r\n",
                "good",
                None
            )
            .is_err()
        );
        assert!(
            callback_from_request(
                "POST /auth/callback?state=good&code=code&client_id=oaiapp_test HTTP/1.1\r\n\r\n",
                "good",
                None
            )
            .is_err()
        );
        assert!(
            callback_from_request(&request("state=good&error=access_denied"), "good", None)
                .err()
                .unwrap()
                .contains("キャンセル")
        );
    }

    #[test]
    fn trusts_only_auth_origin_without_credentials_queries_or_fragments() {
        let client = ChatGptClient::new(Arc::new(MemoryCredentials::default()));
        assert!(
            client
                .validate_endpoint("https://auth.openai.com/.well-known/jwks.json")
                .is_ok()
        );
        for endpoint in [
            "http://auth.openai.com/token",
            "https://auth.openai.com.evil.invalid/token",
            "https://user@auth.openai.com/token",
            "https://auth.openai.com/token?secret=x",
            "https://auth.openai.com/token#fragment",
            "https://auth.openai.com:444/token",
        ] {
            assert!(client.validate_endpoint(endpoint).is_err());
        }
    }

    #[tokio::test]
    async fn refresh_is_serialized_across_factories_and_saves_rotating_record_once() {
        let _test = TEST_GATE.lock().await;
        let server = FixtureServer::start().await;
        let credentials = Arc::new(MemoryCredentials::default());
        let first = server.client(credentials.clone());
        first.save_session(&saved_session()).unwrap();
        let second = server.client(credentials.clone());
        let (one, two) = tokio::join!(first.access_token(), second.access_token());
        assert_eq!(one.unwrap(), "new-access");
        assert_eq!(two.unwrap(), "new-access");
        let forms = server.forms.lock().unwrap();
        assert_eq!(forms.len(), 1);
        let fields: HashMap<_, _> = url::form_urlencoded::parse(forms[0].as_bytes()).collect();
        assert_eq!(fields.get("client_id").unwrap(), "oaiapp_test");
        assert_eq!(fields.get("refresh_token").unwrap(), "old-refresh");
        assert_eq!(fields.get("resource").unwrap(), RESOURCE);
        assert!(!fields.contains_key("scope"));
        drop(forms);
        let record = first.load_session().unwrap().unwrap();
        assert_eq!(record.refresh_token.as_deref(), Some("new-refresh"));
        assert_eq!(record.access_token.as_deref(), Some("new-access"));
        assert!(record.expires_at > now().unwrap());
        assert_eq!(credentials.session_writes.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn terminal_refresh_clears_tokens_but_transient_failure_preserves_record() {
        let _test = TEST_GATE.lock().await;
        let server = FixtureServer::start().await;
        let credentials = Arc::new(MemoryCredentials::default());
        let client = server.client(credentials.clone());
        client.save_session(&saved_session()).unwrap();
        let before = credentials.get(CredentialKind::ChatGptSession).unwrap();
        *server.token_reply.lock().unwrap() = (503, json!({"error":"temporarily_unavailable"}));
        assert!(client.access_token().await.is_err());
        assert_eq!(
            credentials.get(CredentialKind::ChatGptSession).unwrap(),
            before
        );
        *server.token_reply.lock().unwrap() = (400, json!({"error":"refresh_token_reused"}));
        assert!(client.access_token().await.is_err());
        let record = client.load_session().unwrap().unwrap();
        assert_eq!(record.client_id, "oaiapp_test");
        assert_eq!(record.subject, "test-subject");
        assert!(
            record.id_token.is_none()
                && record.access_token.is_none()
                && record.refresh_token.is_none()
        );
        assert!(!read_status(credentials.as_ref()).unwrap().connected);
    }

    #[tokio::test]
    async fn logout_preserves_registration_and_reports_unconfirmed_remote_revocation() {
        let _test = TEST_GATE.lock().await;
        let server = FixtureServer::start().await;
        let credentials = Arc::new(MemoryCredentials::default());
        let client = server.client(credentials.clone());
        client.save_session(&saved_session()).unwrap();
        let host = client.host_id().unwrap();
        *server.revoke_status.lock().unwrap() = 503;
        assert!(
            client
                .disconnect()
                .await
                .unwrap_err()
                .contains("サインアウトしました")
        );
        let record = client.load_session().unwrap().unwrap();
        assert_eq!(record.client_id, "oaiapp_test");
        assert!(
            record.access_token.is_none()
                && record.refresh_token.is_none()
                && record.id_token.is_none()
                && record.scopes.is_empty()
        );
        assert_eq!(client.host_id().unwrap(), host);
        client.save_session(&saved_session()).unwrap();
        *server.revoke_status.lock().unwrap() = 200;
        assert!(client.disconnect().await.is_ok());
    }

    fn browser_callback(
        server: &FixtureServer,
        outcome: &'static str,
        expected_new: bool,
    ) -> impl FnOnce(String) -> Result<(), String> + Send + 'static {
        let token_reply = server.token_reply.clone();
        let runtime = tokio::runtime::Handle::current();
        move |authorization| {
            let url = Url::parse(&authorization).unwrap();
            let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
            assert_eq!(query["scope"], SCOPES);
            assert_eq!(query["resource"], RESOURCE);
            assert_eq!(query["code_challenge_method"], "S256");
            assert!(query["ext_agent_host_id"].starts_with("urn:uuid:"));
            assert_eq!(
                query["client_id"],
                if expected_new {
                    "dynamic_agent_client"
                } else {
                    "oaiapp_test"
                }
            );
            assert_eq!(query.contains_key("agent_name_hint"), expected_new);
            assert_eq!(query.contains_key("id_token_hint"), !expected_new);
            let returned_client = if expected_new {
                "oaiapp_new"
            } else {
                "oaiapp_test"
            };
            let overrides = if outcome == "wrong-account" {
                json!({"sub":"another-subject"})
            } else {
                json!({})
            };
            let nonce = if outcome == "wrong-nonce" {
                "wrong-nonce"
            } else {
                &query["nonce"]
            };
            let id_token = signed_token(returned_client, nonce, overrides);
            *token_reply.lock().unwrap() = if outcome == "no-plan-scope" {
                (
                    200,
                    json!({"id_token":id_token,"scope":"openid profile email"}),
                )
            } else {
                (
                    200,
                    json!({"id_token":id_token,"scope":SCOPES,"access_token":"connected-access","refresh_token":"connected-refresh","token_type":"Bearer","expires_in":3600}),
                )
            };
            let mut callback = Url::parse(&query["redirect_uri"]).unwrap();
            assert_eq!(callback.host_str(), Some("127.0.0.1"));
            assert_eq!(callback.path(), CALLBACK_PATH);
            callback
                .query_pairs_mut()
                .append_pair("state", &query["state"]);
            if outcome == "cancel" {
                callback
                    .query_pairs_mut()
                    .append_pair("error", "access_denied");
            } else {
                callback
                    .query_pairs_mut()
                    .append_pair("code", "fixture-code")
                    .append_pair("client_id", returned_client);
            }
            runtime.spawn(async move {
                reqwest::get(callback).await.unwrap();
            });
            Ok(())
        }
    }

    #[tokio::test]
    async fn cancelled_or_unverified_new_sign_in_preserves_the_prior_account() {
        let _test = TEST_GATE.lock().await;
        let server = FixtureServer::start().await;
        let credentials = Arc::new(MemoryCredentials::default());
        let client = server.client(credentials.clone());
        client.save_session(&saved_session()).unwrap();
        let before = credentials.get(CredentialKind::ChatGptSession).unwrap();
        assert!(
            client
                .connect(true, browser_callback(&server, "cancel", true))
                .await
                .is_err()
        );
        assert_eq!(
            credentials.get(CredentialKind::ChatGptSession).unwrap(),
            before
        );
        assert!(
            client
                .connect(true, browser_callback(&server, "wrong-nonce", true))
                .await
                .is_err()
        );
        assert_eq!(
            credentials.get(CredentialKind::ChatGptSession).unwrap(),
            before
        );
        assert!(
            client
                .connect(false, browser_callback(&server, "wrong-account", false))
                .await
                .is_err()
        );
        assert_eq!(
            credentials.get(CredentialKind::ChatGptSession).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn verified_sign_in_without_plan_permission_stays_connected_and_blocks_inference() {
        let _test = TEST_GATE.lock().await;
        let server = FixtureServer::start().await;
        let credentials = Arc::new(MemoryCredentials::default());
        let client = server.client(credentials.clone());
        client.save_session(&saved_session()).unwrap();
        client
            .connect(true, browser_callback(&server, "no-plan-scope", true))
            .await
            .unwrap();
        let status = read_status(credentials.as_ref()).unwrap();
        assert!(status.connected);
        assert!(!status.sharing);
        assert_eq!(status.account.as_deref(), Some("test@example.invalid"));
        assert!(client.access_token().await.is_err());
        assert_eq!(server.forms.lock().unwrap().len(), 1);
        let record = client.load_session().unwrap().unwrap();
        assert_eq!(record.client_id, "oaiapp_new");
        assert!(record.refresh_token.is_none());
    }

    #[tokio::test]
    async fn callback_reader_bounds_headers_and_ignores_unverified_requests() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { await_callback(&listener, "good", None).await });
        let mut oversized = TcpStream::connect(address).await.unwrap();
        oversized
            .write_all(&vec![b'x'; MAX_CALLBACK_HEADER + 1])
            .await
            .unwrap();
        let mut response = Vec::new();
        oversized.read_to_end(&mut response).await.unwrap();
        assert!(
            String::from_utf8(response)
                .unwrap()
                .starts_with("HTTP/1.1 400")
        );
        let mut invalid = TcpStream::connect(address).await.unwrap();
        invalid
            .write_all(
                b"GET /auth/callback?state=wrong&code=code&client_id=oaiapp_test HTTP/1.1\r\n\r\n",
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        invalid.read_to_end(&mut response).await.unwrap();
        assert!(
            String::from_utf8(response)
                .unwrap()
                .starts_with("HTTP/1.1 400")
        );
        let mut valid = TcpStream::connect(address).await.unwrap();
        valid
            .write_all(
                b"GET /auth/callback?state=good&code=code&client_id=oaiapp_test HTTP/1.1\r\n\r\n",
            )
            .await
            .unwrap();
        assert_eq!(task.await.unwrap().unwrap().client_id, "oaiapp_test");
    }

    #[tokio::test]
    async fn cancelling_a_blocking_credential_worker_keeps_the_shared_gate_until_completion() {
        let _test = TEST_GATE.lock().await;
        let guard = Arc::clone(&SESSION_GATE).lock_owned().await;
        let (started, observed) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let command = tokio::spawn(async move {
            serialized_blocking(guard, move || {
                started.send(()).unwrap();
                wait.recv().unwrap();
                Ok(())
            })
            .await
        });
        observed.await.unwrap();
        command.abort();
        assert!(command.await.unwrap_err().is_cancelled());
        assert!(Arc::clone(&SESSION_GATE).try_lock_owned().is_err());
        release.send(()).unwrap();
        let _guard = timeout(
            Duration::from_secs(2),
            Arc::clone(&SESSION_GATE).lock_owned(),
        )
        .await
        .unwrap();
    }
}
