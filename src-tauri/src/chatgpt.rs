//! Native public-client SIWC. Protocol contracts:
//! https://developers.openai.com/siwc/token-sharing-open-source/sign-in
//! https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions
//! Tokens stay in the native credential store; this module never reads another app's credentials.

use crate::credentials::{CredentialKind, CredentialSessionLock, CredentialStore};
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
    sync::{Mutex, OwnedMutexGuard, OwnedSemaphorePermit},
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
const MAX_CLIENT_ID_BYTES: usize = 512;
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(180);
// Commands create clients using their current credential store. The gate is shared across them.
static SESSION_GATE: LazyLock<Arc<Mutex<()>>> = LazyLock::new(|| Arc::new(Mutex::new(())));

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatGptStatus {
    pub connected: bool,
    pub sharing: bool,
    pub account: Option<String>,
    pub profiles: Vec<ChatGptProfile>,
    pub active_profile: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ChatGptProfile {
    pub id: String,
    pub account: Option<String>,
    pub connected: bool,
    pub sharing: bool,
}

pub(crate) fn read_status(credentials: &dyn CredentialStore) -> Result<ChatGptStatus, String> {
    let _process = credentials.lock_session()?;
    let registry = load_registry_record(credentials)?;
    let session = registry.active();
    Ok(ChatGptStatus {
        connected: session.is_some_and(Session::connected),
        sharing: session.is_some_and(Session::sharing),
        account: session.and_then(|session| session.email.clone()),
        profiles: registry
            .profiles
            .iter()
            .map(|session| ChatGptProfile {
                id: session.id.clone(),
                account: session.email.clone(),
                connected: session.connected(),
                sharing: session.sharing(),
            })
            .collect(),
        active_profile: registry.active_profile,
    })
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Session {
    id: String,
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

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    version: u8,
    active_profile: Option<String>,
    profiles: Vec<Session>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_registration: Option<PendingRegistration>,
}

// An issued ID is not an account identity until a new ID token is validated.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingRegistration {
    issuer: String,
    client_id: String,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: 1,
            active_profile: None,
            profiles: Vec::new(),
            pending_registration: None,
        }
    }
}

impl Registry {
    fn active(&self) -> Option<&Session> {
        self.profiles
            .iter()
            .find(|profile| Some(&profile.id) == self.active_profile.as_ref())
    }

    fn active_mut(&mut self) -> Option<&mut Session> {
        self.profiles
            .iter_mut()
            .find(|profile| Some(&profile.id) == self.active_profile.as_ref())
    }

    fn validate(&self) -> Result<(), String> {
        let invalid = || "保存されたChatGPTアカウント登録が不正です。".to_owned();
        let mut ids = HashSet::new();
        let mut clients = HashSet::new();
        if self.version != 1 || self.active_profile.is_none() != self.profiles.is_empty() {
            return Err(invalid());
        }
        for profile in &self.profiles {
            let id = uuid::Uuid::parse_str(&profile.id).map_err(|_| invalid())?;
            if id.get_version_num() != 4
                || id.to_string() != profile.id
                || !ids.insert(&profile.id)
                || !clients.insert(&profile.client_id)
                || profile.issuer != AUTH_ORIGIN
                || profile.subject.is_empty()
                || !issued_client_id(&profile.client_id)
            {
                return Err(invalid());
            }
        }
        if self.pending_registration.as_ref().is_some_and(|pending| {
            pending.issuer != AUTH_ORIGIN
                || !issued_client_id(&pending.client_id)
                || clients.contains(&pending.client_id)
        }) {
            return Err(invalid());
        }
        if self
            .active_profile
            .as_ref()
            .is_some_and(|active| !ids.contains(active))
        {
            return Err(invalid());
        }
        Ok(())
    }
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

struct SessionGuard {
    _local: OwnedMutexGuard<()>,
    _process: CredentialSessionLock,
}

#[derive(Clone)]
pub(crate) struct ChatGptClient {
    credentials: Arc<dyn CredentialStore>,
    http: Result<Client, String>,
    discovery_url: String,
    trusted_origin: String,
    endpoint_origin: String,
    capacity: Option<Arc<OwnedSemaphorePermit>>,
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
            capacity: None,
        }
    }

    pub(crate) fn with_capacity(mut self, capacity: Arc<OwnedSemaphorePermit>) -> Self {
        self.capacity = Some(capacity);
        self
    }

    pub(crate) async fn connect(
        &self,
        new_account: bool,
        open_browser: impl FnOnce(String) -> Result<(), String> + Send + 'static,
    ) -> Result<(), String> {
        let guard = self.acquire_guard(true).await?;
        let (guard, mut registry) = self.load_registry_async(guard).await?;
        let previous = registry.active().cloned();
        let returning = previous.as_ref().filter(|_| !new_account);
        let pending = registry
            .pending_registration
            .clone()
            .filter(|_| returning.is_none());
        let issued_client = returning
            .map(|session| session.client_id.as_str())
            .or_else(|| pending.as_ref().map(|pending| pending.client_id.as_str()));
        if issued_client.is_none() {
            let mut reservation = registry.clone();
            reservation.pending_registration = Some(PendingRegistration {
                issuer: AUTH_ORIGIN.to_owned(),
                // JSON escaping can double an otherwise valid client's byte length.
                client_id: "\\".repeat(MAX_CLIENT_ID_BYTES),
            });
            registry_record(&reservation)?;
        }
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
                ("client_id", issued_client.unwrap_or("dynamic_agent_client")),
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
            } else if issued_client.is_none() {
                query.append_pair("agent_name_hint", "mizu-pairrank");
            }
        }
        // Authorization URLs contain an ID-token hint on reconnect; never log them.
        let authorization_url = authorization.to_string();
        let capacity = self.capacity.clone();
        let (guard, ()) = serialized_blocking(guard, move || {
            let _capacity = capacity;
            open_browser(authorization_url)
        })
        .await?;
        let callback = timeout(
            SIGN_IN_TIMEOUT,
            await_callback(&listener, &state, issued_client),
        )
        .await
        .map_err(|_| {
            "ChatGPTサインインが時間切れになりました。再試行してください。".to_owned()
        })??;
        let guard = if issued_client.is_none() {
            registry.pending_registration = Some(PendingRegistration {
                issuer: discovery.issuer.clone(),
                client_id: callback.client_id.clone(),
            });
            // Publish the issued ID before consuming a one-time authorization code.
            self.save_registry_async(guard, &registry).await?
        } else {
            guard
        };
        let tokens = match self
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
        {
            Ok(tokens) => tokens,
            Err(failure) => {
                // An invalid unverified registration cannot be resumed. Let the
                // user's next explicit attempt create a fresh registration.
                if returning.is_none() && failure.code.as_deref() == Some("invalid_client") {
                    registry.pending_registration = None;
                    let _guard = self.save_registry_async(guard, &registry).await?;
                }
                return Err(failure.message);
            }
        };
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
            id: returning.map_or_else(|| uuid::Uuid::new_v4().to_string(), |s| s.id.clone()),
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
        // Each issued registration retains its own verified identity and credential set.
        if returning.is_some() {
            *registry.active_mut().expect("validated active profile") = session;
        } else {
            registry.pending_registration = None;
            registry.active_profile = Some(session.id.clone());
            registry.profiles.push(session);
        }
        let _guard = self.save_registry_async(guard, &registry).await?;
        Ok(())
    }

    pub(crate) async fn select_profile(&self, id: String) -> Result<(), String> {
        let guard = self.acquire_guard(false).await?;
        let (guard, mut registry) = self.load_registry_async(guard).await?;
        if !registry.profiles.iter().any(|profile| profile.id == id) {
            return Err("選択したChatGPTアカウント登録がありません。".to_owned());
        }
        registry.active_profile = Some(id);
        let _guard = self.save_registry_async(guard, &registry).await?;
        Ok(())
    }

    pub(crate) async fn disconnect(&self) -> Result<(), String> {
        let client = self.clone();
        tokio::spawn(async move { client.disconnect_locked().await })
            .await
            .map_err(|_| "ChatGPTのサインアウトを完了できませんでした。".to_owned())?
    }

    async fn disconnect_locked(&self) -> Result<(), String> {
        let guard = self.acquire_guard(false).await?;
        let (guard, mut registry) = self.load_registry_async(guard).await?;
        let Some(session) = registry.active() else {
            return Ok(());
        };
        let revocation = if let Some(token) = &session.refresh_token {
            self.revoke(&session.client_id, token).await
        } else {
            Ok(())
        };
        registry
            .active_mut()
            .expect("validated active profile")
            .clear_tokens();
        let _guard = self.save_registry_async(guard, &registry).await?;
        revocation.map_err(|_| "このアプリからサインアウトしましたが、ChatGPT側のセッション失効を確認できませんでした。ChatGPTの設定からアプリの接続を解除してください。".to_owned())
    }

    pub(crate) async fn access_token(&self) -> Result<String, String> {
        // Rotating refresh tokens must be saved even when a requesting search is cancelled.
        let client = self.clone();
        tokio::spawn(async move { client.access_token_locked().await })
            .await
            .map_err(|_| "ChatGPT接続の更新を完了できませんでした。".to_owned())?
    }

    async fn access_token_locked(&self) -> Result<String, String> {
        let guard = self.acquire_guard(false).await?;
        let (guard, mut registry) = self.load_registry_async(guard).await?;
        let mut session = registry
            .active()
            .cloned()
            .ok_or_else(|| "先にChatGPTに接続してください。".to_owned())?;
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
            *registry.active_mut().expect("validated active profile") = session;
            let _guard = self.save_registry_async(guard, &registry).await?;
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
                    *registry.active_mut().expect("validated active profile") = session;
                    let _guard = self.save_registry_async(guard, &registry).await?;
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
        *registry.active_mut().expect("validated active profile") = session.clone();
        let _guard = self.save_registry_async(guard, &registry).await?;
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

    async fn acquire_guard(&self, fail_if_busy: bool) -> Result<SessionGuard, String> {
        let local = if fail_if_busy {
            Arc::clone(&SESSION_GATE).try_lock_owned().map_err(|_| {
                "ChatGPTの接続処理が進行中です。完了してから再試行してください。".to_owned()
            })?
        } else {
            Arc::clone(&SESSION_GATE).lock_owned().await
        };
        let credentials = Arc::clone(&self.credentials);
        let capacity = self.capacity.clone();
        let (local, process) = serialized_blocking(local, move || {
            let _capacity = capacity;
            credentials.lock_session()
        })
        .await?;
        Ok(SessionGuard {
            _local: local,
            _process: process,
        })
    }

    async fn load_registry_async(
        &self,
        guard: SessionGuard,
    ) -> Result<(SessionGuard, Registry), String> {
        let credentials = Arc::clone(&self.credentials);
        let capacity = self.capacity.clone();
        serialized_blocking(guard, move || {
            let _capacity = capacity;
            load_registry_record(credentials.as_ref())
        })
        .await
    }

    async fn save_registry_async(
        &self,
        guard: SessionGuard,
        registry: &Registry,
    ) -> Result<SessionGuard, String> {
        let credentials = Arc::clone(&self.credentials);
        let registry = registry.clone();
        let capacity = self.capacity.clone();
        let (guard, ()) = serialized_blocking(guard, move || {
            let _capacity = capacity;
            save_registry_record(credentials.as_ref(), &registry)
        })
        .await?;
        Ok(guard)
    }

    async fn host_id_async(&self, guard: SessionGuard) -> Result<(SessionGuard, String), String> {
        let credentials = Arc::clone(&self.credentials);
        let capacity = self.capacity.clone();
        serialized_blocking(guard, move || {
            let _capacity = capacity;
            host_id_record(credentials.as_ref())
        })
        .await
    }

    #[cfg(test)]
    fn load_session(&self) -> Result<Option<Session>, String> {
        let _guard = self.credentials.lock_session()?;
        Ok(load_registry_record(self.credentials.as_ref())?
            .active()
            .cloned())
    }

    #[cfg(test)]
    fn save_session(&self, session: &Session) -> Result<(), String> {
        let _guard = self.credentials.lock_session()?;
        let mut registry = load_registry_record(self.credentials.as_ref())?;
        if let Some(previous) = registry
            .profiles
            .iter_mut()
            .find(|profile| profile.id == session.id)
        {
            *previous = session.clone();
        } else {
            registry.profiles.push(session.clone());
        }
        registry.active_profile = Some(session.id.clone());
        save_registry_record(self.credentials.as_ref(), &registry)
    }

    #[cfg(test)]
    fn host_id(&self) -> Result<String, String> {
        let _guard = self.credentials.lock_session()?;
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

async fn serialized_blocking<G: Send + 'static, T: Send + 'static>(
    guard: G,
    operation: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<(G, T), String> {
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

fn save_registry_record(
    credentials: &dyn CredentialStore,
    registry: &Registry,
) -> Result<(), String> {
    registry.validate()?;
    credentials.set(CredentialKind::ChatGptSession, &registry_record(registry)?)
}

fn registry_record(registry: &Registry) -> Result<String, String> {
    let record = serde_json::to_string(registry)
        .map_err(|_| "ChatGPT接続を保存できませんでした。".to_owned())?;
    if record.len() > MAX_HTTP_BODY {
        return Err(
            "ChatGPTアカウント登録の保存容量を超えました。既存の登録は保持されています。"
                .to_owned(),
        );
    }
    Ok(record)
}

fn host_id_record(credentials: &dyn CredentialStore) -> Result<String, String> {
    if let Some(host) = credentials.get(CredentialKind::ChatGptHost)? {
        if host
            .strip_prefix("urn:uuid:")
            .and_then(|value| {
                uuid::Uuid::parse_str(value)
                    .ok()
                    .filter(|id| id.get_version_num() == 4 && id.to_string() == value)
            })
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

fn load_registry_record(credentials: &dyn CredentialStore) -> Result<Registry, String> {
    match credentials.get(CredentialKind::ChatGptSession)? {
        None => Ok(Registry::default()),
        Some(record) => {
            if record.len() > MAX_HTTP_BODY {
                return Err("保存されたChatGPT接続の形式が不正です。".to_owned());
            }
            let registry: Registry = serde_json::from_str(&record)
                .map_err(|_| "保存されたChatGPT接続を読み取れませんでした。".to_owned())?;
            registry.validate()?;
            Ok(registry)
        }
    }
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
        && value.len() <= MAX_CLIENT_ID_BYTES
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
            atomic::{AtomicBool, AtomicUsize, Ordering},
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

    // Only generated mock strings are stored in this test fixture. Production tokens
    // always use SystemCredentials; the empty production lock is exercised here.
    struct ProcessFixtureCredentials {
        root: std::path::PathBuf,
        locking: crate::credentials::SystemCredentials,
    }

    impl ProcessFixtureCredentials {
        fn new(root: std::path::PathBuf) -> Self {
            assert_eq!(
                std::fs::read_to_string(root.join("mock-only")).unwrap(),
                "generated SIWC test fixture"
            );
            let prepared = Arc::new(crate::storage::PreparedAppData::open(root.clone()).unwrap());
            Self {
                root,
                locking: crate::credentials::SystemCredentials::new(prepared),
            }
        }
    }

    impl CredentialStore for ProcessFixtureCredentials {
        fn get(&self, kind: CredentialKind) -> Result<Option<String>, String> {
            assert_eq!(kind, CredentialKind::ChatGptSession);
            match std::fs::read_to_string(self.root.join("mock-registry.json")) {
                Ok(record) => Ok(Some(record)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(_) => Err("cannot read generated fixture".to_owned()),
            }
        }
        fn set(&self, kind: CredentialKind, value: &str) -> Result<(), String> {
            assert_eq!(kind, CredentialKind::ChatGptSession);
            std::fs::write(self.root.join("mock-registry.json"), value)
                .map_err(|_| "cannot write generated fixture".to_owned())
        }
        fn lock_session(&self) -> Result<CredentialSessionLock, String> {
            self.locking.lock_session()
        }
    }

    fn saved_session() -> Session {
        Session {
            id: "f9aad168-6c4e-4d23-94e5-4bdfd7462d82".to_owned(),
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
        pause_token: Arc<AtomicBool>,
        token_started: Arc<tokio::sync::Notify>,
        token_release: Arc<tokio::sync::Semaphore>,
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
            let pause_token = Arc::new(AtomicBool::new(false));
            let token_started = Arc::new(tokio::sync::Notify::new());
            let token_release = Arc::new(tokio::sync::Semaphore::new(0));
            let (server_origin, reply, revoke, requests) = (
                origin.clone(),
                token_reply.clone(),
                revoke_status.clone(),
                forms.clone(),
            );
            let (pause, started, release) = (
                pause_token.clone(),
                token_started.clone(),
                token_release.clone(),
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
                    let (pause, started, release) =
                        (pause.clone(), started.clone(), release.clone());
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
                                started.notify_one();
                                if pause.load(Ordering::SeqCst) {
                                    release.acquire().await.unwrap().forget();
                                }
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
                pause_token,
                token_started,
                token_release,
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
        let mut other = saved_session();
        other.id = uuid::Uuid::new_v4().to_string();
        other.client_id = "oaiapp_other".to_owned();
        other.subject = "other-subject".to_owned();
        client.save_session(&other).unwrap();
        client.select_profile(saved_session().id).await.unwrap();
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
        let registry = load_registry_record(credentials.as_ref()).unwrap();
        let retained = registry
            .profiles
            .iter()
            .find(|profile| profile.id == other.id)
            .unwrap();
        assert_eq!(retained.refresh_token, other.refresh_token);
        assert_eq!(registry.active_profile, Some(saved_session().id));
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

    fn registration_callback(
        server: &FixtureServer,
        expected_client: &'static str,
        issued_client: &'static str,
        outcome: &'static str,
        attempts: Arc<StdMutex<Vec<HashMap<String, String>>>>,
    ) -> impl FnOnce(String) -> Result<(), String> + Send + 'static {
        let token_reply = server.token_reply.clone();
        let runtime = tokio::runtime::Handle::current();
        move |authorization| {
            let query: HashMap<String, String> = Url::parse(&authorization)
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
            assert_eq!(query["client_id"], expected_client);
            assert_eq!(
                query.contains_key("agent_name_hint"),
                expected_client == "dynamic_agent_client"
            );
            let mut observed = attempts.lock().unwrap();
            observed.push(query.clone());
            let code = format!("fixture-code-{}", observed.len());
            drop(observed);
            *token_reply.lock().unwrap() = match outcome {
                "invalid_grant" | "invalid_client" => (400, json!({"error":outcome})),
                _ => (
                    200,
                    json!({"id_token":signed_token(issued_client, &query["nonce"], json!({})), "scope":SCOPES, "access_token":"connected-access", "refresh_token":"connected-refresh", "token_type":"Bearer", "expires_in":3600}),
                ),
            };
            let mut callback = Url::parse(&query["redirect_uri"]).unwrap();
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
                    .append_pair("code", &code)
                    .append_pair("client_id", issued_client);
            }
            runtime.spawn(async move {
                reqwest::get(callback).await.unwrap();
            });
            Ok(())
        }
    }

    #[tokio::test]
    async fn invalid_code_exchange_retains_issued_registration_for_fresh_authorization() {
        let _test = TEST_GATE.lock().await;
        let server = FixtureServer::start().await;
        let credentials = Arc::new(MemoryCredentials::default());
        let client = server.client(credentials.clone());
        let attempts = Arc::new(StdMutex::new(Vec::new()));
        assert!(
            client
                .connect(
                    false,
                    registration_callback(
                        &server,
                        "dynamic_agent_client",
                        "oaiapp_pending",
                        "invalid_grant",
                        attempts.clone()
                    )
                )
                .await
                .is_err()
        );
        let status = read_status(credentials.as_ref()).unwrap();
        assert!(
            !status.connected
                && !status.sharing
                && status.profiles.is_empty()
                && status.active_profile.is_none()
        );
        let stored: Value = serde_json::from_str(
            &credentials
                .get(CredentialKind::ChatGptSession)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            stored["pending_registration"],
            json!({"issuer":AUTH_ORIGIN, "client_id":"oaiapp_pending"})
        );
        assert!(client.access_token().await.is_err());
        // A fresh client factory must recover using the issued ID, never a new dynamic registration.
        let restarted = server.client(credentials.clone());
        restarted
            .connect(
                false,
                registration_callback(
                    &server,
                    "oaiapp_pending",
                    "oaiapp_pending",
                    "success",
                    attempts.clone(),
                ),
            )
            .await
            .unwrap();
        let queries = attempts.lock().unwrap();
        assert_eq!(queries.len(), 2);
        for field in ["state", "nonce", "code_challenge"] {
            assert_ne!(queries[0][field], queries[1][field]);
        }
        assert_eq!(
            queries[0]["ext_agent_host_id"],
            queries[1]["ext_agent_host_id"]
        );
        assert!(
            !queries[1].contains_key("id_token_hint") && !queries[1].contains_key("login_hint")
        );
        let forms = server.forms.lock().unwrap();
        assert_eq!(forms.len(), 2);
        for (index, form) in forms.iter().enumerate() {
            let form: HashMap<_, _> = url::form_urlencoded::parse(form.as_bytes()).collect();
            assert_eq!(form["client_id"], "oaiapp_pending");
            assert_eq!(form["grant_type"], "authorization_code");
            assert_eq!(form["code"], format!("fixture-code-{}", index + 1));
            assert_eq!(
                URL_SAFE_NO_PAD.encode(digest::digest(
                    &digest::SHA256,
                    form["code_verifier"].as_bytes()
                )),
                queries[index]["code_challenge"]
            );
        }
        let status = read_status(credentials.as_ref()).unwrap();
        assert!(status.connected && status.sharing);
        assert_eq!(status.profiles.len(), 1);
        assert_eq!(
            restarted.load_session().unwrap().unwrap().client_id,
            "oaiapp_pending"
        );
        assert!(
            load_registry_record(credentials.as_ref())
                .unwrap()
                .pending_registration
                .is_none()
        );
    }

    #[tokio::test]
    async fn pending_registration_preserves_saved_profiles_and_reconnect_choice() {
        let _test = TEST_GATE.lock().await;
        let server = FixtureServer::start().await;
        let credentials = Arc::new(MemoryCredentials::default());
        let client = server.client(credentials.clone());
        let original = saved_session();
        client.save_session(&original).unwrap();
        credentials
            .set(
                CredentialKind::ApiKey(crate::images::SearchProvider::Openai),
                "fixture-api-key",
            )
            .unwrap();
        credentials
            .set(
                CredentialKind::OpenAiSettings,
                "fixture-model-and-provider-settings",
            )
            .unwrap();
        let before = serde_json::to_value(read_status(credentials.as_ref()).unwrap()).unwrap();
        let attempts = Arc::new(StdMutex::new(Vec::new()));
        assert!(
            client
                .connect(
                    true,
                    registration_callback(
                        &server,
                        "dynamic_agent_client",
                        "oaiapp_pending",
                        "invalid_grant",
                        attempts.clone()
                    )
                )
                .await
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(read_status(credentials.as_ref()).unwrap()).unwrap(),
            before
        );
        let registry = load_registry_record(credentials.as_ref()).unwrap();
        assert_eq!(
            serde_json::to_value(&registry.profiles).unwrap(),
            json!([original])
        );
        assert_eq!(
            registry.pending_registration.unwrap().client_id,
            "oaiapp_pending"
        );
        assert!(
            client
                .connect(
                    true,
                    registration_callback(
                        &server,
                        "oaiapp_pending",
                        "oaiapp_pending",
                        "cancel",
                        attempts.clone()
                    )
                )
                .await
                .is_err()
        );
        let before_substitution = credentials.get(CredentialKind::ChatGptSession).unwrap();
        assert!(
            client
                .connect(
                    true,
                    registration_callback(
                        &server,
                        "oaiapp_pending",
                        "oaiapp_substitution",
                        "success",
                        attempts.clone()
                    )
                )
                .await
                .is_err()
        );
        assert_eq!(
            credentials.get(CredentialKind::ChatGptSession).unwrap(),
            before_substitution
        );
        // Reconnecting the selected verified profile does not consume the unfinished new account.
        client
            .connect(
                false,
                registration_callback(
                    &server,
                    "oaiapp_test",
                    "oaiapp_test",
                    "success",
                    attempts.clone(),
                ),
            )
            .await
            .unwrap();
        assert!(attempts.lock().unwrap()[3].contains_key("id_token_hint"));
        let before_resume =
            serde_json::to_value(load_registry_record(credentials.as_ref()).unwrap().profiles)
                .unwrap();
        let restarted = server.client(credentials.clone());
        restarted
            .connect(
                true,
                registration_callback(
                    &server,
                    "oaiapp_pending",
                    "oaiapp_pending",
                    "success",
                    attempts.clone(),
                ),
            )
            .await
            .unwrap();
        let registry = load_registry_record(credentials.as_ref()).unwrap();
        assert_eq!(registry.profiles.len(), 2);
        assert_eq!(
            serde_json::to_value(&registry.profiles[..1]).unwrap(),
            before_resume
        );
        assert_eq!(registry.active().unwrap().client_id, "oaiapp_pending");
        assert!(registry.pending_registration.is_none());
        assert_eq!(
            credentials
                .get(CredentialKind::ApiKey(
                    crate::images::SearchProvider::Openai
                ))
                .unwrap()
                .as_deref(),
            Some("fixture-api-key")
        );
        assert_eq!(
            credentials
                .get(CredentialKind::OpenAiSettings)
                .unwrap()
                .as_deref(),
            Some("fixture-model-and-provider-settings")
        );
        // Once promoted, another explicit add-account request can start a new registration.
        assert!(
            restarted
                .connect(
                    true,
                    registration_callback(
                        &server,
                        "dynamic_agent_client",
                        "oaiapp_next",
                        "invalid_grant",
                        attempts
                    )
                )
                .await
                .is_err()
        );
        assert_eq!(
            load_registry_record(credentials.as_ref())
                .unwrap()
                .profiles
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn invalid_pending_client_allows_a_new_explicit_registration_without_erasing_profiles() {
        let _test = TEST_GATE.lock().await;
        let server = FixtureServer::start().await;
        let credentials = Arc::new(MemoryCredentials::default());
        let client = server.client(credentials.clone());
        client.save_session(&saved_session()).unwrap();
        let attempts = Arc::new(StdMutex::new(Vec::new()));
        assert!(
            client
                .connect(
                    true,
                    registration_callback(
                        &server,
                        "dynamic_agent_client",
                        "oaiapp_pending",
                        "invalid_grant",
                        attempts.clone()
                    )
                )
                .await
                .is_err()
        );
        let before =
            serde_json::to_value(load_registry_record(credentials.as_ref()).unwrap().profiles)
                .unwrap();
        assert!(
            client
                .connect(
                    false,
                    registration_callback(
                        &server,
                        "oaiapp_test",
                        "oaiapp_test",
                        "invalid_client",
                        attempts.clone()
                    )
                )
                .await
                .is_err()
        );
        assert!(
            load_registry_record(credentials.as_ref())
                .unwrap()
                .pending_registration
                .is_some()
        );
        assert!(
            client
                .connect(
                    true,
                    registration_callback(
                        &server,
                        "oaiapp_pending",
                        "oaiapp_pending",
                        "invalid_client",
                        attempts.clone()
                    )
                )
                .await
                .is_err()
        );
        let registry = load_registry_record(credentials.as_ref()).unwrap();
        assert!(registry.pending_registration.is_none());
        assert_eq!(serde_json::to_value(registry.profiles).unwrap(), before);
        client
            .connect(
                true,
                registration_callback(
                    &server,
                    "dynamic_agent_client",
                    "oaiapp_replacement",
                    "success",
                    attempts,
                ),
            )
            .await
            .unwrap();
        assert_eq!(
            load_registry_record(credentials.as_ref())
                .unwrap()
                .profiles
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn pending_registration_validation_and_registry_capacity_preserve_prior_record() {
        let _test = TEST_GATE.lock().await;
        let credentials = Arc::new(MemoryCredentials::default());
        let client = ChatGptClient::new(credentials.clone());
        client.save_session(&saved_session()).unwrap();
        let valid = credentials
            .get(CredentialKind::ChatGptSession)
            .unwrap()
            .unwrap();
        for pending in [
            json!({"issuer":"https://evil.invalid", "client_id":"oaiapp_pending"}),
            json!({"issuer":AUTH_ORIGIN, "client_id":"dynamic_agent_client"}),
            json!({"issuer":AUTH_ORIGIN, "client_id":"oaiapp_test"}),
            json!({"issuer":AUTH_ORIGIN, "client_id":"x".repeat(513)}),
            json!({"issuer":AUTH_ORIGIN, "client_id":"oaiapp_pending", "subject":"unverified-subject"}),
        ] {
            let mut record: Value = serde_json::from_str(&valid).unwrap();
            record["pending_registration"] = pending;
            credentials
                .set(CredentialKind::ChatGptSession, &record.to_string())
                .unwrap();
            assert!(read_status(credentials.as_ref()).is_err());
            assert_eq!(
                credentials
                    .get(CredentialKind::ChatGptSession)
                    .unwrap()
                    .as_deref(),
                Some(record.to_string().as_str())
            );
        }
        credentials
            .set(CredentialKind::ChatGptSession, &valid)
            .unwrap();
        let mut registry = load_registry_record(credentials.as_ref()).unwrap();
        registry.profiles[0].access_token =
            Some("x".repeat(MAX_HTTP_BODY - valid.len() + "old-access".len() - 32));
        save_registry_record(credentials.as_ref(), &registry).unwrap();
        let before = credentials.get(CredentialKind::ChatGptSession).unwrap();
        assert_eq!(before.as_ref().unwrap().len(), MAX_HTTP_BODY - 32);
        assert!(
            client
                .connect(true, |_| panic!(
                    "insufficient capacity must fail before registration"
                ))
                .await
                .is_err()
        );
        assert_eq!(
            credentials.get(CredentialKind::ChatGptSession).unwrap(),
            before
        );
        registry.pending_registration = Some(PendingRegistration {
            issuer: AUTH_ORIGIN.to_owned(),
            client_id: "oaiapp_pending".to_owned(),
        });
        assert!(save_registry_record(credentials.as_ref(), &registry).is_err());
        assert_eq!(
            credentials.get(CredentialKind::ChatGptSession).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn issued_registration_is_durable_before_exchange_and_survives_cancellation() {
        let _test = TEST_GATE.lock().await;
        let server = FixtureServer::start().await;
        server.pause_token.store(true, Ordering::SeqCst);
        let credentials = Arc::new(MemoryCredentials::default());
        let client = server.client(credentials.clone());
        let attempts = Arc::new(StdMutex::new(Vec::new()));
        let browser = registration_callback(
            &server,
            "dynamic_agent_client",
            "oaiapp_pending",
            "invalid_grant",
            attempts.clone(),
        );
        let connecting = client.clone();
        let command = tokio::spawn(async move { connecting.connect(false, browser).await });
        server.token_started.notified().await;
        let registry = load_registry_record(credentials.as_ref()).unwrap();
        assert!(registry.profiles.is_empty() && registry.active_profile.is_none());
        assert_eq!(
            registry.pending_registration.unwrap().client_id,
            "oaiapp_pending"
        );
        assert!(
            client
                .connect(true, |_| panic!(
                    "overlapping authorization must be rejected"
                ))
                .await
                .is_err()
        );
        command.abort();
        assert!(command.await.unwrap_err().is_cancelled());
        server.pause_token.store(false, Ordering::SeqCst);
        server.token_release.add_permits(1);
        client
            .connect(
                false,
                registration_callback(
                    &server,
                    "oaiapp_pending",
                    "oaiapp_pending",
                    "success",
                    attempts,
                ),
            )
            .await
            .unwrap();
        assert_eq!(
            load_registry_record(credentials.as_ref())
                .unwrap()
                .profiles
                .len(),
            1
        );
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
        let registry = load_registry_record(credentials.as_ref()).unwrap();
        assert_eq!(
            serde_json::to_value(registry.profiles).unwrap(),
            json!([saved_session()])
        );
        assert_eq!(
            registry.pending_registration.unwrap().client_id,
            "oaiapp_new"
        );
        let before_returning = credentials.get(CredentialKind::ChatGptSession).unwrap();
        assert!(
            client
                .connect(false, browser_callback(&server, "wrong-account", false))
                .await
                .is_err()
        );
        assert_eq!(
            credentials.get(CredentialKind::ChatGptSession).unwrap(),
            before_returning
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
    async fn saved_profiles_remain_separate_and_selected_registration_is_reused() {
        let _test = TEST_GATE.lock().await;
        let server = FixtureServer::start().await;
        let credentials = Arc::new(MemoryCredentials::default());
        let client = server.client(credentials.clone());
        let original = saved_session();
        client.save_session(&original).unwrap();
        let host = client.host_id().unwrap();
        client
            .connect(true, browser_callback(&server, "success", true))
            .await
            .unwrap();
        let status = read_status(credentials.as_ref()).unwrap();
        assert_eq!(status.profiles.len(), 2);
        assert!(
            status
                .profiles
                .iter()
                .all(|profile| profile.account == original.email)
        );
        assert_ne!(status.profiles[0].id, status.profiles[1].id);
        let added = status.active_profile.unwrap();
        let added_record = client.load_session().unwrap().unwrap();
        let before = credentials.get(CredentialKind::ChatGptSession).unwrap();
        assert!(
            client
                .connect(true, browser_callback(&server, "success", true))
                .await
                .is_err()
        );
        assert_eq!(
            credentials.get(CredentialKind::ChatGptSession).unwrap(),
            before
        );
        client.select_profile(original.id.clone()).await.unwrap();
        client
            .connect(false, browser_callback(&server, "success", false))
            .await
            .unwrap();
        assert_eq!(client.host_id().unwrap(), host);
        let registry = load_registry_record(credentials.as_ref()).unwrap();
        assert_eq!(registry.profiles.len(), 2);
        assert_eq!(
            registry.active_profile.as_deref(),
            Some(original.id.as_str())
        );
        assert_eq!(registry.active().unwrap().client_id, original.client_id);
        assert_eq!(
            registry
                .profiles
                .iter()
                .find(|profile| profile.id == added)
                .unwrap()
                .refresh_token,
            added_record.refresh_token
        );
        client.disconnect().await.unwrap();
        let status = read_status(credentials.as_ref()).unwrap();
        assert!(!status.connected);
        assert_eq!(status.active_profile.as_deref(), Some(original.id.as_str()));
        assert!(
            status
                .profiles
                .iter()
                .find(|profile| profile.id == added)
                .unwrap()
                .connected
        );
        client.select_profile(added).await.unwrap();
        assert_eq!(client.access_token().await.unwrap(), "connected-access");
        let before = credentials.get(CredentialKind::ChatGptSession).unwrap();
        assert!(
            client
                .select_profile(uuid::Uuid::new_v4().to_string())
                .await
                .is_err()
        );
        assert_eq!(
            credentials.get(CredentialKind::ChatGptSession).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn registry_rejects_corruption_duplicates_and_oversize_without_replacement() {
        let _test = TEST_GATE.lock().await;
        let credentials = Arc::new(MemoryCredentials::default());
        let client = ChatGptClient::new(credentials.clone());
        client.save_session(&saved_session()).unwrap();
        let valid = credentials
            .get(CredentialKind::ChatGptSession)
            .unwrap()
            .unwrap();
        let mut bad_ids: Value = serde_json::from_str(&valid).unwrap();
        bad_ids["profiles"][0]["id"] = json!("not-a-uuid");
        let mut duplicate: Value = serde_json::from_str(&valid).unwrap();
        let profile = duplicate["profiles"][0].clone();
        duplicate["profiles"].as_array_mut().unwrap().push(profile);
        let mut duplicate_client = duplicate.clone();
        duplicate_client["profiles"][1]["id"] = json!(uuid::Uuid::new_v4().to_string());
        let mut bad_issuer: Value = serde_json::from_str(&valid).unwrap();
        bad_issuer["profiles"][0]["issuer"] = json!("https://evil.invalid");
        let mut bad_subject: Value = serde_json::from_str(&valid).unwrap();
        bad_subject["profiles"][0]["subject"] = json!("");
        let mut bad_pointer: Value = serde_json::from_str(&valid).unwrap();
        bad_pointer["active_profile"] = json!(uuid::Uuid::new_v4().to_string());
        for record in [
            "invalid-json".to_owned(),
            bad_ids.to_string(),
            duplicate.to_string(),
            duplicate_client.to_string(),
            bad_issuer.to_string(),
            bad_subject.to_string(),
            bad_pointer.to_string(),
        ] {
            credentials
                .set(CredentialKind::ChatGptSession, &record)
                .unwrap();
            assert!(read_status(credentials.as_ref()).is_err());
            assert!(
                client
                    .connect(true, |_| panic!(
                        "corrupt registry must fail before browser"
                    ))
                    .await
                    .is_err()
            );
            assert_eq!(
                credentials
                    .get(CredentialKind::ChatGptSession)
                    .unwrap()
                    .as_deref(),
                Some(record.as_str())
            );
        }
        credentials
            .set(CredentialKind::ChatGptSession, &valid)
            .unwrap();
        let mut registry = load_registry_record(credentials.as_ref()).unwrap();
        registry.profiles[0].access_token = Some("x".repeat(MAX_HTTP_BODY));
        assert!(save_registry_record(credentials.as_ref(), &registry).is_err());
        assert_eq!(
            credentials
                .get(CredentialKind::ChatGptSession)
                .unwrap()
                .as_deref(),
            Some(valid.as_str())
        );
    }

    #[tokio::test]
    async fn cancelled_refresh_finishes_rotation_before_releasing_lock_and_capacity() {
        let _test = TEST_GATE.lock().await;
        let server = FixtureServer::start().await;
        server.pause_token.store(true, Ordering::SeqCst);
        let credentials = Arc::new(MemoryCredentials::default());
        let capacity = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = Arc::new(capacity.clone().acquire_owned().await.unwrap());
        let client = server
            .client(credentials.clone())
            .with_capacity(permit.clone());
        client.save_session(&saved_session()).unwrap();
        let requesting = client.clone();
        let search = tokio::spawn(async move { requesting.access_token().await });
        server.token_started.notified().await;
        search.abort();
        assert!(search.await.unwrap_err().is_cancelled());
        drop(client);
        drop(permit);
        assert_eq!(capacity.available_permits(), 0);
        assert!(Arc::clone(&SESSION_GATE).try_lock_owned().is_err());
        server.token_release.add_permits(1);
        timeout(Duration::from_secs(2), capacity.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        let record = load_registry_record(credentials.as_ref()).unwrap();
        assert_eq!(
            record.active().unwrap().refresh_token.as_deref(),
            Some("new-refresh")
        );
        assert_eq!(server.forms.lock().unwrap().len(), 1);
    }

    #[test]
    fn cross_process_refresh_child() {
        let Some(root) = std::env::var_os("PAIRRANK_SIWC_MOCK_ROOT") else {
            return;
        };
        let origin = std::env::var("PAIRRANK_SIWC_MOCK_ORIGIN").unwrap();
        let endpoint = Url::parse(&origin).unwrap();
        assert_eq!(endpoint.scheme(), "http");
        assert_eq!(endpoint.host_str(), Some("127.0.0.1"));
        let credentials = Arc::new(ProcessFixtureCredentials::new(root.into()));
        let mut client = ChatGptClient::new(credentials.clone());
        client.discovery_url = format!("{origin}/.well-known/openid-configuration");
        client.endpoint_origin = origin;
        let marker = format!(
            "ready-{}",
            std::env::var("PAIRRANK_SIWC_MOCK_WORKER").unwrap()
        );
        std::fs::write(credentials.root.join(marker), "ready").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        assert_eq!(
            runtime.block_on(client.access_token()).unwrap(),
            "new-access"
        );
    }

    #[tokio::test]
    async fn independent_processes_serialize_read_refresh_and_rotating_record_publish() {
        let _test = TEST_GATE.lock().await;
        let server = FixtureServer::start().await;
        server.pause_token.store(true, Ordering::SeqCst);
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("mock-only"), "generated SIWC test fixture").unwrap();
        let credentials = Arc::new(ProcessFixtureCredentials::new(root.path().to_owned()));
        ChatGptClient::new(credentials.clone())
            .save_session(&saved_session())
            .unwrap();
        let binary = std::env::current_exe().unwrap();
        let mut children = Vec::new();
        for worker in ["one", "two"] {
            children.push(
                std::process::Command::new(&binary)
                    .args(["--exact", "chatgpt::tests::cross_process_refresh_child"])
                    .env("PAIRRANK_SIWC_MOCK_ROOT", root.path())
                    .env("PAIRRANK_SIWC_MOCK_ORIGIN", &server.origin)
                    .env("PAIRRANK_SIWC_MOCK_WORKER", worker)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
        }
        timeout(Duration::from_secs(5), async {
            loop {
                if root.path().join("ready-one").exists() && root.path().join("ready-two").exists()
                {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        timeout(Duration::from_secs(5), server.token_started.notified())
            .await
            .unwrap();
        sleep(Duration::from_millis(100)).await;
        assert_eq!(server.forms.lock().unwrap().len(), 1);
        server.token_release.add_permits(2);
        for child in children {
            let output = tokio::task::spawn_blocking(move || child.wait_with_output())
                .await
                .unwrap()
                .unwrap();
            assert!(
                output.status.success(),
                "mock child failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert_eq!(server.forms.lock().unwrap().len(), 1);
        let registry = load_registry_record(credentials.as_ref()).unwrap();
        assert_eq!(
            registry.active().unwrap().refresh_token.as_deref(),
            Some("new-refresh")
        );
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
    async fn cancelled_process_lock_acquisition_keeps_local_gate_and_capacity() {
        let _test = TEST_GATE.lock().await;
        struct WaitingCredentials {
            locking: crate::credentials::SystemCredentials,
            started: StdMutex<Option<tokio::sync::oneshot::Sender<()>>>,
        }
        impl CredentialStore for WaitingCredentials {
            fn get(&self, _: CredentialKind) -> Result<Option<String>, String> {
                panic!("lock fixture does not access credentials");
            }
            fn set(&self, _: CredentialKind, _: &str) -> Result<(), String> {
                panic!("lock fixture does not access credentials");
            }
            fn lock_session(&self) -> Result<CredentialSessionLock, String> {
                if let Some(started) = self.started.lock().unwrap().take() {
                    started.send(()).unwrap();
                }
                self.locking.lock_session()
            }
        }
        let root = tempfile::tempdir().unwrap();
        let prepared =
            Arc::new(crate::storage::PreparedAppData::open(root.path().to_owned()).unwrap());
        let locking = crate::credentials::SystemCredentials::new(prepared.clone());
        let existing_process = locking.lock_session().unwrap();
        let (started, observed) = tokio::sync::oneshot::channel();
        let credentials = Arc::new(WaitingCredentials {
            locking: crate::credentials::SystemCredentials::new(prepared),
            started: StdMutex::new(Some(started)),
        });
        let capacity = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = Arc::new(capacity.clone().acquire_owned().await.unwrap());
        let client = ChatGptClient::new(credentials).with_capacity(permit.clone());
        let requesting = client.clone();
        let command = tokio::spawn(async move { requesting.acquire_guard(false).await });
        observed.await.unwrap();
        command.abort();
        assert!(command.await.err().unwrap().is_cancelled());
        drop(client);
        drop(permit);
        assert!(Arc::clone(&SESSION_GATE).try_lock_owned().is_err());
        assert_eq!(capacity.available_permits(), 0);
        drop(existing_process);
        let _guard = timeout(
            Duration::from_secs(2),
            Arc::clone(&SESSION_GATE).lock_owned(),
        )
        .await
        .unwrap();
        assert_eq!(capacity.available_permits(), 1);
    }

    #[tokio::test]
    async fn cancelling_a_blocking_credential_worker_keeps_the_shared_gate_until_completion() {
        let _test = TEST_GATE.lock().await;
        let root = tempfile::tempdir().unwrap();
        let prepared =
            Arc::new(crate::storage::PreparedAppData::open(root.path().to_owned()).unwrap());
        let credentials = Arc::new(crate::credentials::SystemCredentials::new(prepared));
        let guard = SessionGuard {
            _local: Arc::clone(&SESSION_GATE).lock_owned().await,
            _process: credentials.lock_session().unwrap(),
        };
        let capacity = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = Arc::new(capacity.clone().acquire_owned().await.unwrap());
        let (started, observed) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let command = tokio::spawn(async move {
            serialized_blocking(guard, move || {
                let _permit = permit;
                started.send(()).unwrap();
                wait.recv().unwrap();
                Ok(())
            })
            .await
        });
        observed.await.unwrap();
        command.abort();
        assert!(command.await.err().unwrap().is_cancelled());
        assert!(Arc::clone(&SESSION_GATE).try_lock_owned().is_err());
        assert_eq!(capacity.available_permits(), 0);
        let (process_acquired, mut process_observed) = tokio::sync::oneshot::channel();
        let other_process = tokio::task::spawn_blocking(move || {
            let _guard = credentials.lock_session().unwrap();
            process_acquired.send(()).unwrap();
        });
        assert!(
            timeout(Duration::from_millis(100), &mut process_observed)
                .await
                .is_err()
        );
        release.send(()).unwrap();
        let _guard = timeout(
            Duration::from_secs(2),
            Arc::clone(&SESSION_GATE).lock_owned(),
        )
        .await
        .unwrap();
        timeout(Duration::from_secs(2), process_observed)
            .await
            .unwrap()
            .unwrap();
        other_process.await.unwrap();
        assert_eq!(capacity.available_permits(), 1);
    }
}
