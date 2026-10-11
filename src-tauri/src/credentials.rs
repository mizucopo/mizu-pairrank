use crate::images::SearchProvider;

#[cfg(any(windows, test))]
mod session_chunks;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CredentialKind {
    ApiKey(SearchProvider),
    OpenAiSettings,
    ChatGptHost,
    ChatGptSession,
}

/// Keys never leave the native layer or get persisted in the application database.
pub(crate) trait CredentialStore: Send + Sync {
    fn get(&self, kind: CredentialKind) -> Result<Option<String>, String>;
    fn set(&self, kind: CredentialKind, key: &str) -> Result<(), String>;
}

pub(crate) struct SystemCredentials;

impl SystemCredentials {
    fn entry(kind: CredentialKind) -> Result<keyring::Entry, String> {
        let account = match kind {
            CredentialKind::ApiKey(SearchProvider::Brave) => "brave-image-search",
            CredentialKind::ApiKey(SearchProvider::Ollama) => "ollama-web-search",
            CredentialKind::ApiKey(SearchProvider::Openai) => "openai-image-search",
            CredentialKind::OpenAiSettings => "openai-image-search-settings",
            CredentialKind::ChatGptHost => "chatgpt-image-search-host",
            CredentialKind::ChatGptSession => "chatgpt-image-search-session",
        };
        keyring::Entry::new("dev.mizu.pairrank", account)
            .map_err(|_| "OSの資格情報ストアを開けませんでした。".to_owned())
    }
}

impl CredentialStore for SystemCredentials {
    fn get(&self, kind: CredentialKind) -> Result<Option<String>, String> {
        #[cfg(windows)]
        if kind == CredentialKind::ChatGptSession {
            return session_chunks::get(&session_chunks::WindowsChunks);
        }
        match Self::entry(kind)?.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err("OSの資格情報ストアから資格情報を取得できませんでした。".to_owned()),
        }
    }

    fn set(&self, kind: CredentialKind, key: &str) -> Result<(), String> {
        #[cfg(windows)]
        if kind == CredentialKind::ChatGptSession {
            return session_chunks::set(&session_chunks::WindowsChunks, key);
        }
        let entry = Self::entry(kind)?;
        if key.is_empty() {
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_) => Err("資格情報を削除できませんでした。".to_owned()),
            }
        } else {
            entry
                .set_password(key)
                .map_err(|_| "資格情報をOSの資格情報ストアに保存できませんでした。".to_owned())
        }
    }
}
