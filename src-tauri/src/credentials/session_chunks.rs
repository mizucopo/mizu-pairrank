//! Windows generic credentials hold at most 2560 bytes. Store immutable session
//! generations and publish their manifest last, keeping tokens in the OS store.
//! https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentiala

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

const MANIFEST_ACCOUNT: &str = "chatgpt-image-search-session";
const CHUNK_BYTES: usize = 2560;
const MAX_SESSION_BYTES: usize = 128 * 1024;
static CHUNK_GATE: Mutex<()> = Mutex::new(());

pub(super) trait ChunkStore {
    fn get(&self, account: &str) -> Result<Option<Vec<u8>>, String>;
    fn set(&self, account: &str, secret: &[u8]) -> Result<(), String>;
    fn delete(&self, account: &str) -> Result<(), String>;
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u8,
    generation: String,
    byte_count: usize,
    chunk_count: usize,
}

impl Manifest {
    fn new(bytes: usize) -> Self {
        Self {
            version: 1,
            generation: uuid::Uuid::new_v4().to_string(),
            byte_count: bytes,
            chunk_count: bytes.div_ceil(CHUNK_BYTES),
        }
    }

    fn validate(&self) -> Result<(), String> {
        let generation = uuid::Uuid::parse_str(&self.generation).map_err(|_| invalid_record())?;
        if self.version != 1
            || generation.get_version_num() != 4
            || generation.to_string() != self.generation
            || !(1..=MAX_SESSION_BYTES).contains(&self.byte_count)
            || self.chunk_count != self.byte_count.div_ceil(CHUNK_BYTES)
        {
            return Err(invalid_record());
        }
        Ok(())
    }

    fn account(&self, index: usize) -> String {
        format!("{MANIFEST_ACCOUNT}-{}-{index}", self.generation)
    }
}

fn invalid_record() -> String {
    "Windows資格情報ストアのChatGPT接続記録が不完全または不正です。".to_owned()
}

fn manifest(store: &impl ChunkStore) -> Result<Option<Manifest>, String> {
    store
        .get(MANIFEST_ACCOUNT)?
        .map(|bytes| {
            if bytes.len() > CHUNK_BYTES {
                return Err(invalid_record());
            }
            let manifest: Manifest =
                serde_json::from_slice(&bytes).map_err(|_| invalid_record())?;
            manifest.validate()?;
            Ok(manifest)
        })
        .transpose()
}

pub(super) fn get(store: &impl ChunkStore) -> Result<Option<String>, String> {
    let _guard = CHUNK_GATE
        .lock()
        .map_err(|_| "Windows資格情報ストアのChatGPT接続を読み取れませんでした。".to_owned())?;
    let Some(manifest) = manifest(store)? else {
        return Ok(None);
    };
    let mut record = Vec::with_capacity(manifest.byte_count);
    for index in 0..manifest.chunk_count {
        let secret = store
            .get(&manifest.account(index))?
            .ok_or_else(invalid_record)?;
        let expected = CHUNK_BYTES.min(manifest.byte_count - record.len());
        if secret.len() != expected {
            return Err(invalid_record());
        }
        record.extend_from_slice(&secret);
    }
    String::from_utf8(record)
        .map(Some)
        .map_err(|_| invalid_record())
}

fn cleanup(store: &impl ChunkStore, manifest: &Manifest) -> bool {
    // Try every part even if one deletion fails; do not discard later cleanup work.
    let mut complete = true;
    for index in 0..manifest.chunk_count {
        if store.delete(&manifest.account(index)).is_err() {
            complete = false;
        }
    }
    complete
}

pub(super) fn set(store: &impl ChunkStore, record: &str) -> Result<(), String> {
    let _guard = CHUNK_GATE
        .lock()
        .map_err(|_| "Windows資格情報ストアのChatGPT接続を更新できませんでした。".to_owned())?;
    if record.len() > MAX_SESSION_BYTES {
        return Err("ChatGPT接続記録が大きすぎるため保存できませんでした。".to_owned());
    }
    let previous = manifest(store)?;
    if record.is_empty() {
        store.delete(MANIFEST_ACCOUNT)?;
        if previous
            .as_ref()
            .is_some_and(|previous| !cleanup(store, previous))
        {
            return Err("ChatGPT接続の参照は削除しましたが、一部の資格情報がOSに残っています。Windows資格情報マネージャーからmizu-pairrankのChatGPT資格情報を削除してください。".to_owned());
        }
        return Ok(());
    }
    let pending = Manifest::new(record.len());
    let write = || -> Result<(), String> {
        for (index, chunk) in record.as_bytes().chunks(CHUNK_BYTES).enumerate() {
            store.set(&pending.account(index), chunk)?;
        }
        let pointer = serde_json::to_vec(&pending).map_err(|_| invalid_record())?;
        store.set(MANIFEST_ACCOUNT, &pointer)
    };
    if write().is_err() {
        return if cleanup(store, &pending) {
            Err("ChatGPT接続を保存できませんでした。以前の接続は保持されています。再試行してください。".to_owned())
        } else {
            Err("ChatGPT接続を保存できず、以前の接続は保持されています。一部の未使用資格情報がOSに残りました。Windows資格情報マネージャーで確認してください。".to_owned())
        };
    }
    if previous
        .as_ref()
        .is_some_and(|previous| !cleanup(store, previous))
    {
        return Err("ChatGPT接続は更新されましたが、一部の古い資格情報がOSに残っています。Windows資格情報マネージャーでmizu-pairrankの古いChatGPT資格情報を削除してください。".to_owned());
    }
    Ok(())
}

#[cfg(windows)]
pub(super) struct WindowsChunks;

#[cfg(windows)]
impl WindowsChunks {
    fn entry(account: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new("dev.mizu.pairrank", account)
            .map_err(|_| "OSの資格情報ストアを開けませんでした。".to_owned())
    }
}

#[cfg(windows)]
impl ChunkStore for WindowsChunks {
    fn get(&self, account: &str) -> Result<Option<Vec<u8>>, String> {
        match Self::entry(account)?.get_secret() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => {
                Err("OSの資格情報ストアからChatGPT資格情報を取得できませんでした。".to_owned())
            }
        }
    }
    fn set(&self, account: &str, secret: &[u8]) -> Result<(), String> {
        Self::entry(account)?
            .set_secret(secret)
            .map_err(|_| "ChatGPT資格情報をOSの資格情報ストアに保存できませんでした。".to_owned())
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => {
                Err("OSの資格情報ストアからChatGPT資格情報を削除できませんでした。".to_owned())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Mutex};

    #[derive(Default)]
    struct MockChunks {
        records: Mutex<HashMap<String, Vec<u8>>>,
        writes: Mutex<usize>,
        fail_write: Mutex<Option<usize>>,
        fail_manifest: Mutex<bool>,
        fail_delete: Mutex<Option<String>>,
    }

    impl ChunkStore for MockChunks {
        fn get(&self, account: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.records.lock().unwrap().get(account).cloned())
        }
        fn set(&self, account: &str, secret: &[u8]) -> Result<(), String> {
            assert!(secret.len() <= CHUNK_BYTES, "Windows credential blob limit");
            let mut writes = self.writes.lock().unwrap();
            *writes += 1;
            if *self.fail_write.lock().unwrap() == Some(*writes)
                || (account == MANIFEST_ACCOUNT && *self.fail_manifest.lock().unwrap())
            {
                return Err("fixture write failure".to_owned());
            }
            self.records
                .lock()
                .unwrap()
                .insert(account.to_owned(), secret.to_vec());
            Ok(())
        }
        fn delete(&self, account: &str) -> Result<(), String> {
            if self.fail_delete.lock().unwrap().as_deref() == Some(account) {
                return Err("fixture deletion failure".to_owned());
            }
            self.records.lock().unwrap().remove(account);
            Ok(())
        }
    }

    fn large_record() -> String {
        serde_json::json!({"email":"日本語🦀@example.invalid","id_token":"a".repeat(8000),"access_token":"b".repeat(7000),"refresh_token":"c".repeat(5000),"display":"日本語🦀".repeat(2000)}).to_string()
    }

    #[test]
    fn large_unicode_and_tokens_roundtrip_as_bounded_raw_byte_parts() {
        let store = MockChunks::default();
        let record = large_record();
        set(&store, &record).unwrap();
        assert_eq!(get(&store).unwrap().as_deref(), Some(record.as_str()));
        let pointer = manifest(&store).unwrap().unwrap();
        assert_eq!(pointer.byte_count, record.len());
        assert_eq!(store.records.lock().unwrap().len(), pointer.chunk_count + 1);
    }

    #[test]
    fn partial_write_failure_keeps_old_pointer_and_cleans_new_parts() {
        let store = MockChunks::default();
        set(&store, "previous").unwrap();
        let before = store.records.lock().unwrap().clone();
        *store.fail_write.lock().unwrap() = Some(*store.writes.lock().unwrap() + 2);
        assert!(set(&store, &large_record()).is_err());
        assert_eq!(*store.records.lock().unwrap(), before);
        assert_eq!(get(&store).unwrap().as_deref(), Some("previous"));
    }

    #[test]
    fn manifest_publish_failure_preserves_old_generation() {
        let store = MockChunks::default();
        set(&store, "previous").unwrap();
        let before = store.records.lock().unwrap().clone();
        *store.fail_manifest.lock().unwrap() = true;
        assert!(set(&store, &large_record()).is_err());
        assert_eq!(*store.records.lock().unwrap(), before);
        assert_eq!(get(&store).unwrap().as_deref(), Some("previous"));
    }

    #[test]
    fn rejects_missing_wrong_sized_parts_and_invalid_utf8() {
        let store = MockChunks::default();
        set(&store, "valid").unwrap();
        let part = manifest(&store).unwrap().unwrap().account(0);
        for bytes in [
            None,
            Some(vec![]),
            Some(vec![b'x'; CHUNK_BYTES + 1]),
            Some(vec![0xff; 5]),
        ] {
            if let Some(bytes) = bytes {
                store.records.lock().unwrap().insert(part.clone(), bytes);
            } else {
                store.records.lock().unwrap().remove(&part);
            }
            assert!(get(&store).is_err());
        }
    }

    #[test]
    fn rejects_corrupt_unbounded_manifest_and_direct_json_without_fallback() {
        let store = MockChunks::default();
        let generation = uuid::Uuid::new_v4();
        for pointer in [
            serde_json::json!({"version":1,"generation":"../invalid","byte_count":1,"chunk_count":1}),
            serde_json::json!({"version":1,"generation":generation,"byte_count":MAX_SESSION_BYTES+1,"chunk_count":1}),
            serde_json::json!({"version":1,"generation":generation,"byte_count":1,"chunk_count":100000000}),
            serde_json::json!({"version":1,"generation":generation,"byte_count":0,"chunk_count":0}),
            serde_json::json!({"version":1,"generation":generation,"byte_count":1,"chunk_count":1,"unexpected":true}),
            serde_json::json!({"access_token":"old-direct-json"}),
        ] {
            store.records.lock().unwrap().insert(
                MANIFEST_ACCOUNT.to_owned(),
                serde_json::to_vec(&pointer).unwrap(),
            );
            assert!(get(&store).is_err());
        }
        store
            .records
            .lock()
            .unwrap()
            .insert(MANIFEST_ACCOUNT.to_owned(), vec![b'x'; CHUNK_BYTES + 1]);
        assert!(get(&store).is_err());
    }

    #[test]
    fn shrink_and_delete_remove_previous_generations() {
        let store = MockChunks::default();
        set(&store, &large_record()).unwrap();
        set(&store, "smaller").unwrap();
        assert_eq!(store.records.lock().unwrap().len(), 2);
        assert_eq!(get(&store).unwrap().as_deref(), Some("smaller"));
        set(&store, "").unwrap();
        assert!(store.records.lock().unwrap().is_empty());
        assert!(get(&store).unwrap().is_none());
    }

    #[test]
    fn cleanup_failure_reports_committed_update_and_deleted_pointer_accurately() {
        let store = MockChunks::default();
        set(&store, "previous").unwrap();
        let old_part = manifest(&store).unwrap().unwrap().account(0);
        *store.fail_delete.lock().unwrap() = Some(old_part.clone());
        assert!(
            set(&store, "replacement")
                .unwrap_err()
                .contains("更新されました")
        );
        assert_eq!(get(&store).unwrap().as_deref(), Some("replacement"));
        let active_part = manifest(&store).unwrap().unwrap().account(0);
        *store.fail_delete.lock().unwrap() = Some(active_part.clone());
        assert!(set(&store, "").unwrap_err().contains("参照は削除しました"));
        assert!(get(&store).unwrap().is_none());
        assert!(store.records.lock().unwrap().contains_key(&active_part));
    }

    #[test]
    fn oversized_record_never_mutates_an_existing_session() {
        let store = MockChunks::default();
        set(&store, "previous").unwrap();
        let before = store.records.lock().unwrap().clone();
        assert!(set(&store, &"x".repeat(MAX_SESSION_BYTES + 1)).is_err());
        assert_eq!(*store.records.lock().unwrap(), before);
    }
}
