use crate::images::SearchProvider;

/// Keys never leave the native layer or get persisted in the application database.
pub(crate) trait CredentialStore: Send + Sync {
    fn get(&self, provider: SearchProvider) -> Result<Option<String>, String>;
    fn set(&self, provider: SearchProvider, key: &str) -> Result<(), String>;
}

pub(crate) struct SystemCredentials;

impl SystemCredentials {
    fn entry(provider: SearchProvider) -> Result<keyring::Entry, String> {
        let account = match provider {
            SearchProvider::Brave => "brave-image-search",
            SearchProvider::Ollama => "ollama-web-search",
        };
        keyring::Entry::new("dev.mizu.pairrank", account)
            .map_err(|_| "OSの資格情報ストアを開けませんでした。".to_owned())
    }
}

impl CredentialStore for SystemCredentials {
    fn get(&self, provider: SearchProvider) -> Result<Option<String>, String> {
        match Self::entry(provider)?.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err("OSの資格情報ストアからAPIキーを取得できませんでした。".to_owned()),
        }
    }

    fn set(&self, provider: SearchProvider, key: &str) -> Result<(), String> {
        let entry = Self::entry(provider)?;
        if key.is_empty() {
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(_) => Err("APIキーを削除できませんでした。".to_owned()),
            }
        } else {
            entry
                .set_password(key)
                .map_err(|_| "APIキーをOSの資格情報ストアに保存できませんでした。".to_owned())
        }
    }
}
