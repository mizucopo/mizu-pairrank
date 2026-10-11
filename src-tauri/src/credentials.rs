use crate::images::SearchProvider;
use std::{fs::File, sync::Arc};

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
    // In-memory test stores need no process lock. The production store always supplies one.
    fn lock_session(&self) -> Result<CredentialSessionLock, String> {
        Ok(CredentialSessionLock { file: None })
    }
}

pub(crate) struct CredentialSessionLock {
    file: Option<File>,
}

impl CredentialSessionLock {
    pub(crate) fn acquire(file: File) -> Result<Self, String> {
        file.lock()
            .map_err(|_| "ChatGPT接続を他のアプリプロセスと同期できませんでした。".to_owned())?;
        Ok(Self { file: Some(file) })
    }
}

impl Drop for CredentialSessionLock {
    fn drop(&mut self) {
        if let Some(file) = &self.file {
            let _ = file.unlock();
        }
    }
}

pub(crate) struct SystemCredentials {
    app_data: Arc<crate::storage::PreparedAppData>,
}

impl SystemCredentials {
    pub(crate) fn new(app_data: Arc<crate::storage::PreparedAppData>) -> Self {
        Self { app_data }
    }

    fn session_file(&self) -> std::io::Result<File> {
        self.app_data.verify()?;
        #[cfg(unix)]
        {
            use rustix::fs::{Mode, OFlags, openat};
            // Reopen rather than dup: independent open-file descriptions must contend.
            Ok(File::from(openat(
                self.app_data.directory(),
                ".",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )?))
        }
        #[cfg(windows)]
        self.lock_file(".chatgpt-session.lock")
    }

    #[cfg(windows)]
    fn lock_file(&self, name: &str) -> std::io::Result<File> {
        use windows_sys::Wdk::Storage::FileSystem::{
            FILE_NON_DIRECTORY_FILE, FILE_OPEN_IF, FILE_SYNCHRONOUS_IO_NONALERT,
        };
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ,
            FILE_SHARE_WRITE, SYNCHRONIZE,
        };
        self.app_data.verify()?;
        let file = crate::storage::windows::open_relative(
            self.app_data.directory(),
            std::ffi::OsStr::new(name),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN_IF,
            FILE_ATTRIBUTE_NORMAL,
            FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT,
        )?;
        if !file.metadata()?.is_file() || file.metadata()?.len() != 0 {
            return Err(std::io::Error::other(
                "ChatGPT lock is not an empty regular file",
            ));
        }
        Ok(file)
    }

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
    fn lock_session(&self) -> Result<CredentialSessionLock, String> {
        let file = self
            .session_file()
            .map_err(|_| "ChatGPT接続のプロセス間ロックを開けませんでした。".to_owned())?;
        let guard = CredentialSessionLock::acquire(file)?;
        self.app_data
            .verify()
            .map_err(|_| "ChatGPT接続の保存先が差し替えられました。".to_owned())?;
        Ok(guard)
    }

    fn get(&self, kind: CredentialKind) -> Result<Option<String>, String> {
        #[cfg(windows)]
        if kind == CredentialKind::ChatGptSession {
            let _guard =
                CredentialSessionLock::acquire(self.lock_file(".chatgpt-chunks.lock").map_err(
                    |_| "ChatGPT資格情報のプロセス間ロックを開けませんでした。".to_owned(),
                )?)?;
            self.app_data
                .verify()
                .map_err(|_| "ChatGPT接続の保存先が差し替えられました。".to_owned())?;
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
            let _guard =
                CredentialSessionLock::acquire(self.lock_file(".chatgpt-chunks.lock").map_err(
                    |_| "ChatGPT資格情報のプロセス間ロックを開けませんでした。".to_owned(),
                )?)?;
            self.app_data
                .verify()
                .map_err(|_| "ChatGPT接続の保存先が差し替えられました。".to_owned())?;
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
