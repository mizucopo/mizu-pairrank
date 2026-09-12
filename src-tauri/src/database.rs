use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};

use crate::models::{ImageAsset, Item, ListState, ListSummary};
use crate::rating::{
    MODEL_PARAMETERS_JSON, MODEL_VERSION, Preference, Rating, convergence, ranking_ids, update_pair,
};

struct Migration {
    version: u32,
    sql: &'static str,
}

// Released migrations are immutable. Append a new consecutive version for every schema change.
const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: include_str!("../migrations/001_initial.sql"),
}];

#[derive(Debug)]
pub struct ImageChange<T> {
    pub value: T,
    pub cleanup_paths: Vec<String>,
}

pub struct Database {
    connection: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_with(path, |_| {})
    }

    fn open_with(path: &Path, mut checkpoint: impl FnMut(bool)) -> Result<Self, String> {
        // Resolve legitimate parent aliases (for example macOS /var), never the DB leaf.
        #[cfg(unix)]
        let resolved = if path == Path::new(":memory:") {
            path.to_owned()
        } else {
            let parent = path
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            parent
                .canonicalize()
                .map_err(|error| format!("保存先を開けません: {error}"))?
                .join(
                    path.file_name()
                        .ok_or_else(|| "保存データのファイル名がありません。".to_owned())?,
                )
        };
        #[cfg(unix)]
        let path = resolved.as_path();
        let prepared = crate::storage::prepare_database_file(path)
            .map_err(|error| format!("保存データの権限を設定できません: {error}"))?;
        checkpoint(false);
        let mut flags = OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        if prepared.has_identity() {
            flags.remove(OpenFlags::SQLITE_OPEN_CREATE);
        }
        let mut connection = Connection::open_with_flags(path, flags).map_err(db_error)?;
        checkpoint(true);
        verify_prepared_database(&connection, &prepared, path)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(db_error)?;
        migrate(&mut connection, MIGRATIONS)?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(db_error)?;
        let database = Self { connection };
        // Never silently interpret stored ratings with a different model or configuration.
        database.list_summaries()?;
        Ok(database)
    }

    pub fn list_summaries(&self) -> Result<Vec<ListSummary>, String> {
        let transaction = self.connection.unchecked_transaction().map_err(db_error)?;
        let mut statement = transaction
            .prepare("SELECT id FROM lists ORDER BY id")
            .map_err(db_error)?;
        let ids = statement
            .query_map([], |row| row.get::<_, i64>(0))
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        ids.into_iter()
            .map(|id| {
                let state = read_list(&transaction, id)?;
                Ok(ListSummary {
                    id,
                    name: state.name,
                    item_count: state.items.len() as u64,
                    comparison_count: state.comparison_count,
                    converged: state.convergence.converged,
                })
            })
            .collect()
    }

    pub fn create_list(&mut self, name: String) -> Result<ListState, String> {
        let name = validated_name(name)?;
        let transaction = self.connection.transaction().map_err(db_error)?;
        transaction
            .execute(
                "INSERT INTO lists (name, model_version, model_parameters) VALUES (?1, ?2, ?3)",
                params![name, MODEL_VERSION, MODEL_PARAMETERS_JSON],
            )
            .map_err(db_error)?;
        let id = transaction.last_insert_rowid();
        reset_snapshots(&transaction, id)?;
        let state = read_list(&transaction, id)?;
        transaction.commit().map_err(db_error)?;
        Ok(state)
    }

    pub fn rename_list(&mut self, id: i64, name: String) -> Result<ListState, String> {
        let name = validated_name(name)?;
        self.mutate_list(id, |transaction| {
            transaction
                .execute(
                    "UPDATE lists SET name = ?1 WHERE id = ?2",
                    params![name, id],
                )
                .map_err(db_error)?;
            Ok(())
        })
    }

    pub fn delete_list(&mut self, id: i64) -> Result<ImageChange<()>, String> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        ensure_supported_model(&transaction, id)?;
        let cleanup_paths = read_image_paths(&transaction, id, None)?;
        transaction
            .execute("DELETE FROM lists WHERE id = ?1", [id])
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(ImageChange {
            value: (),
            cleanup_paths,
        })
    }

    pub fn get_list(&self, id: i64) -> Result<ListState, String> {
        let transaction = self.connection.unchecked_transaction().map_err(db_error)?;
        read_list(&transaction, id)
    }

    pub fn add_items(&mut self, list_id: i64, names: Vec<String>) -> Result<ListState, String> {
        let names: Vec<String> = names
            .into_iter()
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect();
        if names.is_empty() {
            return Err("項目名を入力してください。".to_owned());
        }
        self.mutate_list(list_id, |transaction| {
            let initial = Rating::default();
            let mut statement = transaction
                .prepare("INSERT INTO items (list_id, name, mu, sigma) VALUES (?1, ?2, ?3, ?4)")
                .map_err(db_error)?;
            for name in names {
                statement
                    .execute(params![list_id, name, initial.mu, initial.sigma])
                    .map_err(db_error)?;
            }
            reset_snapshots(transaction, list_id)
        })
    }

    pub fn rename_item(
        &mut self,
        list_id: i64,
        item_id: i64,
        name: String,
    ) -> Result<ListState, String> {
        let name = validated_name(name)?;
        self.mutate_list(list_id, |transaction| {
            require_item_change(transaction.execute(
                "UPDATE items SET name = ?1 WHERE id = ?2 AND list_id = ?3 AND deleted = 0",
                params![name, item_id, list_id],
            ))
        })
    }

    pub fn delete_item(
        &mut self,
        list_id: i64,
        item_id: i64,
    ) -> Result<ImageChange<ListState>, String> {
        self.mutate_images(list_id, item_id, |transaction| {
            require_item_change(transaction.execute(
                "UPDATE items SET deleted = 1 WHERE id = ?1 AND list_id = ?2 AND deleted = 0",
                params![item_id, list_id],
            ))?;
            reset_snapshots(transaction, list_id)
        })
    }

    pub fn set_image(
        &mut self,
        list_id: i64,
        item_id: i64,
        image: Option<ImageAsset>,
    ) -> Result<ImageChange<ListState>, String> {
        self.mutate_images(list_id, item_id, |transaction| {
            require_item_change(transaction.execute(
                "UPDATE items SET image_path = ?1, image_source_url = ?2
                 WHERE id = ?3 AND list_id = ?4 AND deleted = 0",
                params![
                    image.as_ref().map(|image| &image.path),
                    image.as_ref().and_then(|image| image.source_url.as_ref()),
                    item_id,
                    list_id,
                ],
            ))
        })
    }

    pub fn image_in_use(&self, path: &str) -> Result<bool, String> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM items WHERE image_path = ?1 AND deleted = 0)",
                [path],
                |row| row.get(0),
            )
            .map_err(db_error)
    }

    pub fn image_paths(&self) -> Result<HashSet<String>, String> {
        self.connection
            .prepare("SELECT image_path FROM items WHERE image_path IS NOT NULL AND deleted = 0")
            .map_err(db_error)?
            .query_map([], |row| row.get(0))
            .map_err(db_error)?
            .collect::<Result<_, _>>()
            .map_err(db_error)
    }

    pub fn resume_list(&mut self, list_id: i64) -> Result<ListState, String> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let mut state = read_list(&transaction, list_id)?;
        // Another instance may already have resumed and made progress.
        if state.convergence.converged {
            reset_snapshots(&transaction, list_id)?;
            transaction
                .execute(
                    "UPDATE lists SET revision = revision + 1 WHERE id = ?1",
                    [list_id],
                )
                .map_err(db_error)?;
            state = read_list(&transaction, list_id)?;
        }
        transaction.commit().map_err(db_error)?;
        Ok(state)
    }

    pub fn answer(
        &mut self,
        list_id: i64,
        a_id: i64,
        b_id: i64,
        answer: Preference,
        expected_revision: u64,
    ) -> Result<ListState, String> {
        self.mutate_list(list_id, |transaction| {
            let state = read_list(transaction, list_id)?;
            if state.revision != expected_revision {
                return Err("リストが更新されています。最新の比較を読み直してください。".to_owned());
            }
            if a_id == b_id {
                return Err("異なる2項目を選択してください。".to_owned());
            }
            let a = state.items.iter().find(|item| item.id == a_id);
            let b = state.items.iter().find(|item| item.id == b_id);
            let (Some(a), Some(b)) = (a, b) else {
                return Err("比較する項目が見つかりません。".to_owned());
            };
            let (new_a, new_b) = update_pair(a.rating, b.rating, answer)?;
            for (id, rating) in [(a_id, new_a), (b_id, new_b)] {
                if !rating.mu.is_finite() || !rating.sigma.is_finite() || rating.sigma <= 0.0 {
                    return Err("評価値の計算に失敗しました。回答は保存していません。".to_owned());
                }
                require_item_change(transaction.execute(
                    "UPDATE items SET mu = ?1, sigma = ?2, comparison_count = comparison_count + 1
                     WHERE id = ?3 AND list_id = ?4 AND deleted = 0",
                    params![rating.mu, rating.sigma, id, list_id],
                ))?;
            }
            let preference = serde_json::to_value(answer).map_err(|error| error.to_string())?;
            let preference = preference
                .as_str()
                .ok_or_else(|| "回答の保存形式が不正です。".to_owned())?;
            transaction
                .execute(
                    "INSERT INTO comparisons (list_id, a_id, b_id, preference) VALUES (?1, ?2, ?3, ?4)",
                    params![list_id, a_id, b_id, preference],
                )
                .map_err(db_error)?;
            append_snapshot(transaction, list_id)?;
            let keep = state.items.len().max(20) + 1;
            transaction
                .execute(
                    "DELETE FROM rank_snapshots WHERE list_id = ?1 AND id NOT IN
                     (SELECT id FROM rank_snapshots WHERE list_id = ?1 ORDER BY id DESC LIMIT ?2)",
                    params![list_id, keep as i64],
                )
                .map_err(db_error)?;
            Ok(())
        })
    }

    pub fn pair_counts(&self, list_id: i64) -> Result<HashMap<(i64, i64), u64>, String> {
        ensure_supported_model(&self.connection, list_id)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT min(a_id, b_id), max(a_id, b_id), count(*) FROM comparisons
                 WHERE list_id = ?1 GROUP BY min(a_id, b_id), max(a_id, b_id)",
            )
            .map_err(db_error)?;
        statement
            .query_map([list_id], |row| {
                Ok(((row.get(0)?, row.get(1)?), nonnegative_integer(row, 2)?))
            })
            .map_err(db_error)?
            .collect::<Result<HashMap<_, _>, _>>()
            .map_err(db_error)
    }

    fn mutate_images(
        &mut self,
        list_id: i64,
        item_id: i64,
        operation: impl FnOnce(&Transaction<'_>) -> Result<(), String>,
    ) -> Result<ImageChange<ListState>, String> {
        let (value, cleanup_paths) = self.mutate_list_with_result(list_id, |transaction| {
            let paths = read_image_paths(transaction, list_id, Some(item_id))?;
            operation(transaction)?;
            Ok(paths)
        })?;
        Ok(ImageChange {
            value,
            cleanup_paths,
        })
    }

    fn mutate_list(
        &mut self,
        list_id: i64,
        operation: impl FnOnce(&Transaction<'_>) -> Result<(), String>,
    ) -> Result<ListState, String> {
        self.mutate_list_with_result(list_id, operation)
            .map(|(state, ())| state)
    }

    fn mutate_list_with_result<T>(
        &mut self,
        list_id: i64,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T, String>,
    ) -> Result<(ListState, T), String> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        ensure_supported_model(&transaction, list_id)?;
        let result = operation(&transaction)?;
        transaction
            .execute(
                "UPDATE lists SET revision = revision + 1 WHERE id = ?1",
                [list_id],
            )
            .map_err(db_error)?;
        let state = read_list(&transaction, list_id)?;
        transaction.commit().map_err(db_error)?;
        Ok((state, result))
    }
}

fn read_image_paths(
    transaction: &Transaction<'_>,
    list_id: i64,
    item_id: Option<i64>,
) -> Result<Vec<String>, String> {
    let mut statement = transaction
        .prepare(
            "SELECT DISTINCT image_path FROM items
         WHERE list_id = ?1 AND deleted = 0 AND image_path IS NOT NULL
           AND (?2 IS NULL OR id = ?2)",
        )
        .map_err(db_error)?;
    statement
        .query_map(params![list_id, item_id], |row| row.get(0))
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)
}

fn verify_prepared_database(
    connection: &Connection,
    prepared: &crate::storage::PreparedDatabaseFile,
    path: &Path,
) -> Result<(), String> {
    if !prepared.has_identity() {
        return Ok(());
    }
    let changed =
        || "保存データが起動中に差し替えられました。アプリを再起動してください。".to_owned();
    prepared.verify_path(path).map_err(|_| changed())?;
    let mut moved: std::os::raw::c_int = 0;
    // SAFETY: the connection is live, "main" is NUL-terminated, and the opcode
    // writes one C int synchronously to the valid, exclusively borrowed pointer.
    let result = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            connection.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_HAS_MOVED,
            (&mut moved as *mut std::os::raw::c_int).cast(),
        )
    };
    if result != rusqlite::ffi::SQLITE_OK || moved != 0 {
        return Err(changed());
    }
    Ok(())
}

fn db_error(error: rusqlite::Error) -> String {
    format!("データベース処理に失敗しました: {error}")
}

fn nonnegative_integer(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    value
        .try_into()
        .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

fn validated_name(name: String) -> Result<String, String> {
    let name = name.trim().to_owned();
    if name.is_empty() {
        Err("名前を入力してください。".to_owned())
    } else {
        Ok(name)
    }
}

fn require_item_change(result: rusqlite::Result<usize>) -> Result<(), String> {
    if result.map_err(db_error)? == 1 {
        Ok(())
    } else {
        Err("項目が見つかりません。".to_owned())
    }
}

fn ensure_supported_model(connection: &Connection, list_id: i64) -> Result<(), String> {
    let stored = connection
        .query_row(
            "SELECT model_version, model_parameters FROM lists WHERE id = ?1",
            [list_id],
            |row| Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(db_error)?;
    let Some((version, parameters)) = stored else {
        return Err("リストが見つかりません。".to_owned());
    };
    let stored_parameters = serde_json::from_str::<serde_json::Value>(&parameters);
    let supported_parameters = serde_json::from_str::<serde_json::Value>(MODEL_PARAMETERS_JSON);
    if version != MODEL_VERSION
        || stored_parameters.is_err()
        || supported_parameters.is_err()
        || stored_parameters.ok() != supported_parameters.ok()
    {
        return Err(
            "保存済みの評価モデルに対応していません。対応するアプリを使用してください。".to_owned(),
        );
    }
    Ok(())
}

fn read_items(connection: &Connection, list_id: i64) -> Result<Vec<Item>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, name, image_path, image_source_url, mu, sigma, comparison_count
             FROM items WHERE list_id = ?1 AND deleted = 0 ORDER BY mu DESC, id ASC",
        )
        .map_err(db_error)?;
    statement
        .query_map([list_id], |row| {
            let path: Option<String> = row.get(2)?;
            let source_url = row.get(3)?;
            Ok(Item {
                id: row.get(0)?,
                list_id,
                name: row.get(1)?,
                image: path.map(|path| ImageAsset { path, source_url }),
                rating: Rating {
                    mu: row.get(4)?,
                    sigma: row.get(5)?,
                },
                comparison_count: nonnegative_integer(row, 6)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)
}

fn read_list(connection: &Transaction<'_>, list_id: i64) -> Result<ListState, String> {
    ensure_supported_model(connection, list_id)?;
    let (name, revision) = connection
        .query_row(
            "SELECT name, revision FROM lists WHERE id = ?1",
            [list_id],
            |row| Ok((row.get(0)?, nonnegative_integer(row, 1)?)),
        )
        .map_err(db_error)?;
    let items = read_items(connection, list_id)?;
    let comparison_count = connection
        .query_row(
            "SELECT count(*) FROM comparisons WHERE list_id = ?1",
            [list_id],
            |row| nonnegative_integer(row, 0),
        )
        .map_err(db_error)?;
    let mut statement = connection
        .prepare("SELECT ranking FROM rank_snapshots WHERE list_id = ?1 ORDER BY id")
        .map_err(db_error)?;
    let snapshots = statement
        .query_map([list_id], |row| row.get::<_, String>(0))
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?
        .into_iter()
        .map(|json| serde_json::from_str::<Vec<i64>>(&json).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let ratings: Vec<_> = items.iter().map(|item| (item.id, item.rating)).collect();
    Ok(ListState {
        id: list_id,
        name,
        revision,
        items,
        comparison_count,
        convergence: convergence(&ratings, &snapshots),
    })
}

fn append_snapshot(connection: &Connection, list_id: i64) -> Result<(), String> {
    let ratings: Vec<_> = read_items(connection, list_id)?
        .iter()
        .map(|item| (item.id, item.rating))
        .collect();
    let ranking =
        serde_json::to_string(&ranking_ids(&ratings)).map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO rank_snapshots (list_id, ranking) VALUES (?1, ?2)",
            params![list_id, ranking],
        )
        .map_err(db_error)?;
    Ok(())
}

fn reset_snapshots(connection: &Connection, list_id: i64) -> Result<(), String> {
    connection
        .execute("DELETE FROM rank_snapshots WHERE list_id = ?1", [list_id])
        .map_err(db_error)?;
    append_snapshot(connection, list_id)
}

fn schema_version(connection: &Connection) -> Result<u32, String> {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(db_error)
}

fn validate_schema_version(
    connection: &Connection,
    current: u32,
    latest: u32,
) -> Result<(), String> {
    if current > latest {
        return Err(format!(
            "データベースのバージョン {current} はこのアプリの対応版 {latest} より新しいため開けません。アプリを更新してください。"
        ));
    }
    if current == 0 {
        let objects: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*'",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if objects > 0 {
            return Err(
                "バージョン未管理のデータベースです。既存データを保護するため更新を中止しました。"
                    .to_owned(),
            );
        }
    }
    Ok(())
}

fn migrate(connection: &mut Connection, migrations: &[Migration]) -> Result<(), String> {
    for (index, migration) in migrations.iter().enumerate() {
        if migration.version as usize != index + 1 {
            return Err("アプリに含まれるマイグレーションの連番が不正です。".to_owned());
        }
    }
    let latest = migrations.last().map_or(0, |migration| migration.version);
    let current = schema_version(connection)?;
    if current != 0 && current == latest {
        return Ok(());
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    // Validate version and schema together: another process may have just initialized the DB.
    let current = schema_version(&transaction)?;
    validate_schema_version(&transaction, current, latest)?;
    for migration in migrations
        .iter()
        .filter(|migration| migration.version > current)
    {
        transaction.execute_batch(migration.sql).map_err(|error| {
            format!(
                "データベースの更新（バージョン {}）に失敗しました。変更を取り消しました: {error}",
                migration.version
            )
        })?;
        transaction
            .pragma_update(None, "user_version", migration.version)
            .map_err(db_error)?;
    }
    let foreign_key_violations: i64 = transaction
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .map_err(db_error)?;
    if foreign_key_violations != 0 {
        return Err(
            "データベースの更新で参照関係の不整合が検出されたため、変更を取り消しました。"
                .to_owned(),
        );
    }
    transaction.commit().map_err(db_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn check_ordinary_database_replacement(restore_before_validation: bool) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pairrank.sqlite3");
        let previous = directory.path().join("previous.sqlite3");
        let replacement = directory.path().join("replacement.sqlite3");
        {
            let mut database = Database::open(&path).unwrap();
            database.create_list("Saved rankings".to_owned()).unwrap();
        }
        let original = std::fs::read(&path).unwrap();
        std::fs::write(&replacement, []).unwrap();
        let result = Database::open_with(&path, |opened| {
            if !opened {
                std::fs::rename(&path, &previous).unwrap();
                std::fs::rename(&replacement, &path).unwrap();
            } else if restore_before_validation {
                std::fs::rename(&path, &replacement).unwrap();
                std::fs::rename(&previous, &path).unwrap();
            }
        });
        assert!(result.is_err());
        let (original_path, replacement_path) = if restore_before_validation {
            (&path, &replacement)
        } else {
            (&previous, &path)
        };
        assert_eq!(std::fs::read(original_path).unwrap(), original);
        assert!(std::fs::read(replacement_path).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn ordinary_database_replacement_after_preparation_is_rejected_before_migration() {
        check_ordinary_database_replacement(false);
    }

    #[cfg(unix)]
    #[test]
    fn ordinary_database_replacement_restored_after_sqlite_open_is_still_rejected() {
        check_ordinary_database_replacement(true);
    }

    #[cfg(unix)]
    #[test]
    fn a_prepared_database_removed_before_sqlite_open_is_not_recreated() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pairrank.sqlite3");
        let result = Database::open_with(&path, |opened| {
            if !opened {
                std::fs::remove_file(&path).unwrap();
            }
        });
        assert!(result.is_err());
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn database_preparation_preserves_an_existing_connections_process_lock() {
        const PROBE_PATH: &str = "PAIRRANK_DATABASE_LOCK_PROBE_PATH";
        if let Some(path) = std::env::var_os(PROBE_PATH) {
            let connection = Connection::open(std::path::PathBuf::from(path)).unwrap();
            connection.busy_timeout(Duration::ZERO).unwrap();
            let error = connection.execute_batch("BEGIN IMMEDIATE").unwrap_err();
            assert!(matches!(error, rusqlite::Error::SqliteFailure(error, _)
                if error.code == rusqlite::ErrorCode::DatabaseBusy));
            return;
        }

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pairrank.sqlite3");
        let database = Database::open(&path).unwrap();
        database
            .connection
            .execute_batch("BEGIN IMMEDIATE")
            .unwrap();
        {
            let prepared = crate::storage::prepare_database_file(&path).unwrap();
            verify_prepared_database(&database.connection, &prepared, &path).unwrap();
        }
        // A different process observes the OS lock, independent of SQLite's
        // in-process bookkeeping for its own connections.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "database::tests::database_preparation_preserves_an_existing_connections_process_lock", "--nocapture"])
            .env(PROBE_PATH, &path)
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_open_rejects_symlink_replacement_after_file_preparation() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pairrank.sqlite3");
        let external = directory.path().join("external.sqlite3");
        std::fs::write(&external, []).unwrap();
        std::fs::set_permissions(&external, std::fs::Permissions::from_mode(0o644)).unwrap();
        let result = Database::open_with(&path, |opened| {
            if !opened {
                std::fs::rename(&path, directory.path().join("previous.sqlite3")).unwrap();
                std::os::unix::fs::symlink(&external, &path).unwrap();
            }
        });
        assert!(result.is_err());
        assert!(std::fs::read(&external).unwrap().is_empty());
        assert_eq!(
            std::fs::metadata(&external).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert!(!directory.path().join("external.sqlite3-journal").exists());
    }

    #[cfg(unix)]
    #[test]
    fn database_opens_through_a_legitimate_parent_directory_alias() {
        let directory = tempfile::tempdir().unwrap();
        let actual = directory.path().join("actual");
        let alias = directory.path().join("alias");
        std::fs::create_dir(&actual).unwrap();
        std::os::unix::fs::symlink(&actual, &alias).unwrap();
        let path = alias.join("pairrank.sqlite3");
        {
            let mut database = Database::open(&path).unwrap();
            database
                .create_list("Saved through alias".to_owned())
                .unwrap();
        }
        let reopened = Database::open(&path).unwrap();
        assert_eq!(
            reopened.list_summaries().unwrap()[0].name,
            "Saved through alias"
        );
        assert!(actual.join("pairrank.sqlite3").is_file());
    }

    fn memory_database() -> Database {
        Database::open(Path::new(":memory:")).unwrap()
    }

    fn populated_list(database: &mut Database, name: &str) -> ListState {
        let list = database.create_list(name.to_owned()).unwrap();
        database
            .add_items(list.id, vec!["A".to_owned(), "B".to_owned()])
            .unwrap()
    }

    #[test]
    fn deletion_waits_for_another_writer_before_reading_validation() {
        use std::cell::RefCell;
        use std::sync::mpsc;

        enum Event {
            Waiting,
            Finished(Result<(), String>),
        }
        thread_local! {
            static NOTIFY_BUSY: RefCell<Option<mpsc::Sender<Event>>> = const { RefCell::new(None) };
        }

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("shared.sqlite3");
        let mut database = Database::open(&path).unwrap();
        let list = populated_list(&mut database, "Delete after another writer");
        let mut other = Connection::open(&path).unwrap();
        let writer = other
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        writer
            .execute("UPDATE lists SET name = 'Changed' WHERE id = ?1", [list.id])
            .unwrap();
        let (sender, receiver) = mpsc::channel();
        let delete = std::thread::spawn(move || {
            NOTIFY_BUSY.with(|notify| *notify.borrow_mut() = Some(sender.clone()));
            database
                .connection
                .busy_handler(Some(|_| {
                    NOTIFY_BUSY.with(|notify| {
                        if let Some(sender) = notify.borrow_mut().take() {
                            sender.send(Event::Waiting).unwrap();
                        }
                    });
                    std::thread::sleep(Duration::from_millis(1));
                    true
                }))
                .unwrap();
            sender
                .send(Event::Finished(
                    database.delete_list(list.id).map(|change| change.value),
                ))
                .unwrap();
        });
        let first = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        writer.commit().unwrap();
        delete.join().unwrap();
        assert!(
            matches!(first, Event::Waiting),
            "deletion must wait for the writer before validation"
        );
        let Event::Finished(result) = receiver.recv_timeout(Duration::from_secs(5)).unwrap() else {
            panic!("missing deletion result");
        };
        result.unwrap();
        assert_eq!(
            other
                .query_row("SELECT count(*) FROM lists", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    fn snapshot_count(database: &Database, list_id: i64) -> i64 {
        database
            .connection
            .query_row(
                "SELECT count(*) FROM rank_snapshots WHERE list_id = ?1",
                [list_id],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn answer_once(database: &mut Database, state: &ListState) -> ListState {
        database
            .answer(
                state.id,
                state.items[0].id,
                state.items[1].id,
                Preference::AWeak,
                state.revision,
            )
            .unwrap()
    }

    #[test]
    fn new_database_is_initialized_once_and_reopens_without_data_loss() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rankings.sqlite3");
        let expected = {
            let mut database = Database::open(&path).unwrap();
            assert_eq!(schema_version(&database.connection).unwrap(), 1);
            let state = populated_list(&mut database, "お気に入り");
            let state = answer_once(&mut database, &state);
            let state = database
                .set_image(
                    state.id,
                    state.items[0].id,
                    Some(ImageAsset {
                        path: "/assets/one.png".to_owned(),
                        source_url: Some("https://example.org/one".to_owned()),
                    }),
                )
                .unwrap()
                .value;
            serde_json::to_value(state).unwrap()
        };
        let database = Database::open(&path).unwrap();
        let id = expected["id"].as_i64().unwrap();
        assert_eq!(
            serde_json::to_value(database.get_list(id).unwrap()).unwrap(),
            expected
        );
        assert_eq!(snapshot_count(&database, id), 2);
    }

    #[test]
    fn concurrent_initial_migration_does_not_misclassify_the_database_as_unmanaged() {
        use std::cell::RefCell;
        use std::rc::Rc;

        thread_local! {
            static MIGRATE_AFTER_VERSION: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        }
        fn after_version(event: rusqlite::trace::TraceEvent<'_>) {
            if let rusqlite::trace::TraceEvent::Profile(statement, _) = event
                && statement.sql().contains("user_version")
            {
                let migrate = MIGRATE_AFTER_VERSION.with(|pending| pending.borrow_mut().take());
                if let Some(migrate) = migrate {
                    migrate();
                }
            }
        }

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("initial-migration.sqlite3");
        let mut first = Connection::open(&path).unwrap();
        first.pragma_update(None, "journal_mode", "WAL").unwrap();
        let mut second = Connection::open(&path).unwrap();
        let result = Rc::new(RefCell::new(None));
        let completed = result.clone();
        MIGRATE_AFTER_VERSION.with(|pending| {
            *pending.borrow_mut() = Some(Box::new(move || {
                *completed.borrow_mut() = Some(migrate(&mut second, MIGRATIONS).and_then(|()| {
                    second.execute(
                        "INSERT INTO lists (name, model_version, model_parameters) VALUES (?1, ?2, ?3)",
                        params!["Created by another instance", MODEL_VERSION, MODEL_PARAMETERS_JSON],
                    ).map(|_| ()).map_err(db_error)
                }));
            }));
        });
        first.trace_v2(
            rusqlite::trace::TraceEventCodes::SQLITE_TRACE_PROFILE,
            Some(after_version),
        );
        let migrated = migrate(&mut first, MIGRATIONS);
        first.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
        MIGRATE_AFTER_VERSION.with(|pending| pending.borrow_mut().take());
        assert_eq!(*result.borrow(), Some(Ok(())));
        migrated.unwrap();
        assert_eq!(schema_version(&first).unwrap(), 1);
        let name: String = first
            .query_row("SELECT name FROM lists", [], |row| row.get(0))
            .unwrap();
        assert_eq!(name, "Created by another instance");
    }

    #[test]
    fn pending_migrations_preserve_data_and_apply_in_order_as_one_upgrade() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection, MIGRATIONS).unwrap();
        connection
            .execute(
                "INSERT INTO lists (name, model_version, model_parameters) VALUES ('before', 1, '{}')",
                [],
            )
            .unwrap();
        let migrations = [
            Migration {
                version: 1,
                sql: MIGRATIONS[0].sql,
            },
            Migration {
                version: 2,
                sql: "ALTER TABLE lists ADD COLUMN note TEXT NOT NULL DEFAULT 'kept';",
            },
            Migration {
                version: 3,
                sql: "UPDATE lists SET name = name || '-after', note = note || '-updated';",
            },
        ];
        migrate(&mut connection, &migrations).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), 3);
        let values: (String, String) = connection
            .query_row("SELECT name, note FROM lists", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(
            values,
            ("before-after".to_owned(), "kept-updated".to_owned())
        );
        migrate(&mut connection, &migrations).unwrap();
        let name: String = connection
            .query_row("SELECT name FROM lists", [], |row| row.get(0))
            .unwrap();
        assert_eq!(name, "before-after");
    }

    #[test]
    fn failed_upgrade_rolls_back_every_pending_schema_data_and_version_change() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection, MIGRATIONS).unwrap();
        connection.execute(
            "INSERT INTO lists (name, model_version, model_parameters) VALUES ('original', 1, '{}')",
            [],
        ).unwrap();
        let migrations = [
            Migration {
                version: 1,
                sql: MIGRATIONS[0].sql,
            },
            Migration {
                version: 2,
                sql: "ALTER TABLE lists ADD COLUMN note TEXT; UPDATE lists SET name = 'changed';",
            },
            Migration {
                version: 3,
                sql: "CREATE TABLE temporary_upgrade (id INTEGER); INSERT INTO missing_table VALUES (1);",
            },
        ];
        assert!(migrate(&mut connection, &migrations).is_err());
        assert_eq!(schema_version(&connection).unwrap(), 1);
        let name: String = connection
            .query_row("SELECT name FROM lists", [], |row| row.get(0))
            .unwrap();
        assert_eq!(name, "original");
        assert!(connection.prepare("SELECT note FROM lists").is_err());
        assert!(
            connection
                .prepare("SELECT * FROM temporary_upgrade")
                .is_err()
        );
        assert!(connection.is_autocommit());
        migrate(&mut connection, MIGRATIONS).unwrap();
    }

    #[test]
    fn failed_initial_migration_leaves_an_empty_unversioned_database() {
        let mut connection = Connection::open_in_memory().unwrap();
        let migrations = [Migration {
            version: 1,
            sql: "CREATE TABLE partial (id INTEGER); INSERT INTO missing_table VALUES (1);",
        }];
        assert!(migrate(&mut connection, &migrations).is_err());
        assert_eq!(schema_version(&connection).unwrap(), 0);
        assert!(connection.prepare("SELECT * FROM partial").is_err());
        migrate(&mut connection, MIGRATIONS).unwrap();
    }

    #[test]
    fn migration_rejects_broken_references_before_commit() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&mut connection, MIGRATIONS).unwrap();
        // Table-rebuild migrations may run with immediate foreign key enforcement disabled.
        connection
            .pragma_update(None, "foreign_keys", false)
            .unwrap();
        let migrations = [
            Migration {
                version: 1,
                sql: MIGRATIONS[0].sql,
            },
            Migration {
                version: 2,
                sql: "INSERT INTO items (list_id, name, mu, sigma) VALUES (999, 'orphan', 25, 8);",
            },
        ];
        assert!(
            migrate(&mut connection, &migrations)
                .unwrap_err()
                .contains("参照関係")
        );
        assert_eq!(schema_version(&connection).unwrap(), 1);
        let count: i64 = connection
            .query_row("SELECT count(*) FROM items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn newer_or_unmanaged_databases_are_not_changed() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE existing (value TEXT); INSERT INTO existing VALUES ('saved');",
            )
            .unwrap();
        assert!(
            migrate(&mut connection, MIGRATIONS)
                .unwrap_err()
                .contains("未管理")
        );
        assert_eq!(schema_version(&connection).unwrap(), 0);
        connection.pragma_update(None, "user_version", 99).unwrap();
        assert!(
            migrate(&mut connection, MIGRATIONS)
                .unwrap_err()
                .contains("アプリを更新")
        );
        assert_eq!(schema_version(&connection).unwrap(), 99);
        let value: String = connection
            .query_row("SELECT value FROM existing", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "saved");
        assert!(connection.prepare("SELECT * FROM lists").is_err());
    }

    #[test]
    fn unmanaged_table_with_similar_sqlite_prefix_is_not_mistaken_for_internal_metadata() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE sqliteBackup (value TEXT); INSERT INTO sqliteBackup VALUES ('saved');",
            )
            .unwrap();
        assert!(
            migrate(&mut connection, MIGRATIONS)
                .unwrap_err()
                .contains("未管理")
        );
        assert_eq!(schema_version(&connection).unwrap(), 0);
        let value: String = connection
            .query_row("SELECT value FROM sqliteBackup", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "saved");
        let object_count: i64 = connection
            .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))
            .unwrap();
        assert_eq!(object_count, 1);
        assert!(connection.prepare("SELECT * FROM lists").is_err());
    }

    #[test]
    fn lists_and_items_are_independent_and_names_are_validated() {
        let mut database = memory_database();
        assert!(database.create_list("  \n".to_owned()).is_err());
        let first = database.create_list(" first ".to_owned()).unwrap();
        assert_eq!(first.name, "first");
        assert_eq!(first.revision, 0);
        assert!(database.add_items(first.id, vec![" ".to_owned()]).is_err());
        let first = database
            .add_items(
                first.id,
                vec![" same ".to_owned(), "".to_owned(), "same".to_owned()],
            )
            .unwrap();
        assert_eq!(first.items.len(), 2);
        assert_eq!(first.items[0].name, "same");
        assert!(first.items[0].id < first.items[1].id);
        assert_eq!(first.items[0].rating.sigma, Rating::default().sigma);
        let second = populated_list(&mut database, "second");
        assert!(
            database
                .rename_item(first.id, second.items[0].id, "wrong".to_owned())
                .is_err()
        );
        assert!(database.delete_item(first.id, second.items[0].id).is_err());
        assert!(
            database
                .set_image(first.id, second.items[0].id, None)
                .is_err()
        );
        let changed = database
            .rename_item(first.id, first.items[0].id, "renamed".to_owned())
            .unwrap();
        assert_eq!(changed.revision, first.revision + 1);
        assert_eq!(
            database.get_list(second.id).unwrap().revision,
            second.revision
        );
        let renamed = database
            .rename_list(first.id, "new list".to_owned())
            .unwrap();
        assert_eq!(renamed.name, "new list");
        assert_eq!(database.list_summaries().unwrap().len(), 2);
        database.delete_list(first.id).unwrap();
        assert!(database.get_list(first.id).is_err());
        assert_eq!(database.list_summaries().unwrap().len(), 1);
        assert_eq!(database.get_list(second.id).unwrap().items.len(), 2);
        let remaining: i64 = database
            .connection
            .query_row(
                "SELECT count(*) FROM items WHERE list_id = ?1",
                [first.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn answers_are_revision_guarded_and_reject_invalid_pairs() {
        let mut database = memory_database();
        let first = populated_list(&mut database, "first");
        let second = populated_list(&mut database, "second");
        let a = first.items[0].id;
        let b = first.items[1].id;
        for (a_id, b_id) in [(a, a), (a, second.items[0].id), (a, i64::MAX)] {
            assert!(
                database
                    .answer(first.id, a_id, b_id, Preference::Equal, first.revision)
                    .is_err()
            );
        }
        assert_eq!(
            database.get_list(first.id).unwrap().revision,
            first.revision
        );
        let answered = answer_once(&mut database, &first);
        assert_eq!(answered.comparison_count, 1);
        assert!(answered.items.iter().all(|item| item.comparison_count == 1));
        assert_eq!(answered.revision, first.revision + 1);
        assert!(
            database
                .answer(first.id, a, b, Preference::AWeak, first.revision)
                .is_err()
        );
        let answered = database
            .answer(first.id, b, a, Preference::BWeak, answered.revision)
            .unwrap();
        assert_eq!(
            database.pair_counts(first.id).unwrap()[&(a.min(b), a.max(b))],
            2
        );
        assert_eq!(answered.comparison_count, 2);
        assert_eq!(database.get_list(second.id).unwrap().comparison_count, 0);
    }

    #[test]
    fn failed_answer_does_not_save_partial_ratings_history_or_revision() {
        let mut database = memory_database();
        let state = populated_list(&mut database, "test");
        let before = serde_json::to_value(&state).unwrap();
        database.connection.execute_batch(
            "CREATE TRIGGER reject_answer BEFORE INSERT ON comparisons BEGIN SELECT RAISE(ABORT, 'test failure'); END;"
        ).unwrap();
        assert!(
            database
                .answer(
                    state.id,
                    state.items[0].id,
                    state.items[1].id,
                    Preference::AStrong,
                    state.revision
                )
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(database.get_list(state.id).unwrap()).unwrap(),
            before
        );
        assert_eq!(snapshot_count(&database, state.id), 1);
        assert!(database.pair_counts(state.id).unwrap().is_empty());
    }

    #[test]
    fn membership_changes_reset_window_but_preserve_ratings_and_history() {
        let mut database = memory_database();
        let state = populated_list(&mut database, "test");
        let answered = answer_once(&mut database, &state);
        assert_eq!(answered.convergence.observed_answers, 1);
        assert_eq!(snapshot_count(&database, state.id), 2);
        let existing = answered.items[0].clone();
        let added = database
            .add_items(state.id, vec!["new".to_owned()])
            .unwrap();
        assert_eq!(added.convergence.observed_answers, 0);
        assert_eq!(snapshot_count(&database, state.id), 1);
        let retained = added
            .items
            .iter()
            .find(|item| item.id == existing.id)
            .unwrap();
        assert_eq!(retained.rating.mu, existing.rating.mu);
        assert_eq!(retained.rating.sigma, existing.rating.sigma);
        assert_eq!(
            added
                .items
                .iter()
                .find(|item| item.name == "new")
                .unwrap()
                .rating
                .sigma,
            Rating::default().sigma
        );
        let answered = answer_once(&mut database, &added);
        let deleted = database.delete_item(state.id, existing.id).unwrap().value;
        assert_eq!(deleted.items.len(), 2);
        assert_eq!(deleted.comparison_count, answered.comparison_count);
        assert_eq!(deleted.convergence.observed_answers, 0);
        assert!(!database.pair_counts(state.id).unwrap().is_empty());
        let kept_row: i64 = database
            .connection
            .query_row(
                "SELECT deleted FROM items WHERE id = ?1",
                [existing.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kept_row, 1);
    }

    #[test]
    fn resume_uses_current_convergence_and_preserves_another_instances_progress() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("resume.sqlite3");
        let mut first = Database::open(&path).unwrap();
        let initial = populated_list(&mut first, "shared");
        let mut second = Database::open(&path).unwrap();
        let mut settled = second.get_list(initial.id).unwrap();
        for _ in 0..20 {
            settled = second
                .answer(
                    settled.id,
                    settled.items[0].id,
                    settled.items[1].id,
                    Preference::Equal,
                    settled.revision,
                )
                .unwrap();
        }
        assert!(settled.convergence.converged);
        let resumed = first.resume_list(initial.id).unwrap();
        assert!(!resumed.convergence.converged);
        assert_eq!(resumed.convergence.observed_answers, 0);
        assert_eq!(resumed.revision, settled.revision + 1);
        assert_eq!(resumed.comparison_count, settled.comparison_count);
        assert_eq!(
            serde_json::to_value(&resumed.items).unwrap(),
            serde_json::to_value(&settled.items).unwrap()
        );
        let progress = answer_once(&mut second, &resumed);
        let unchanged = first.resume_list(settled.id).unwrap();
        assert_eq!(
            serde_json::to_value(unchanged).unwrap(),
            serde_json::to_value(progress).unwrap()
        );
        assert_eq!(snapshot_count(&first, initial.id), 2);
    }

    #[test]
    fn ranking_window_keeps_exactly_required_answers_and_baseline() {
        let mut database = memory_database();
        let mut state = populated_list(&mut database, "test");
        for _ in 0..27 {
            state = database
                .answer(
                    state.id,
                    state.items[0].id,
                    state.items[1].id,
                    Preference::Equal,
                    state.revision,
                )
                .unwrap();
        }
        assert_eq!(state.comparison_count, 27);
        assert_eq!(state.convergence.observed_answers, 20);
        assert_eq!(snapshot_count(&database, state.id), 21);
        let state = database
            .add_items(
                state.id,
                (0..23).map(|number| format!("item {number}")).collect(),
            )
            .unwrap();
        assert_eq!(state.convergence.required_answers, 25);
        assert_eq!(snapshot_count(&database, state.id), 1);
    }

    #[test]
    fn incompatible_model_version_or_parameters_are_rejected() {
        let mut database = memory_database();
        let state = populated_list(&mut database, "test");
        database
            .connection
            .execute(
                "UPDATE lists SET model_version = model_version + 1 WHERE id = ?1",
                [state.id],
            )
            .unwrap();
        assert!(
            database
                .get_list(state.id)
                .unwrap_err()
                .contains("評価モデル")
        );
        assert!(
            database
                .rename_list(state.id, "changed".to_owned())
                .is_err()
        );
        database
            .connection
            .execute(
                "UPDATE lists SET model_version = ?1, model_parameters = '{}' WHERE id = ?2",
                params![MODEL_VERSION, state.id],
            )
            .unwrap();
        assert!(
            database
                .get_list(state.id)
                .unwrap_err()
                .contains("評価モデル")
        );
        assert!(database.list_summaries().is_err());
    }
    thread_local! {
        static WRITE_AFTER_SELECT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    }

    struct ConcurrentReadWrite<'a> {
        connection: &'a Connection,
        result: std::rc::Rc<std::cell::RefCell<Option<Result<(), String>>>>,
    }

    impl ConcurrentReadWrite<'_> {
        fn assert_committed(&self) {
            assert_eq!(
                *self.result.borrow(),
                Some(Ok(())),
                "the second connection must commit during the read"
            );
        }
    }

    impl Drop for ConcurrentReadWrite<'_> {
        fn drop(&mut self) {
            self.connection
                .trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
            WRITE_AFTER_SELECT.with(|pending| pending.borrow_mut().take());
        }
    }

    fn write_after_first_select(
        reader: &Database,
        write: impl FnOnce() -> Result<(), String> + 'static,
    ) -> ConcurrentReadWrite<'_> {
        fn after_select(event: rusqlite::trace::TraceEvent<'_>) {
            if let rusqlite::trace::TraceEvent::Profile(statement, _) = event
                && statement.sql().trim_start().starts_with("SELECT")
            {
                let write = WRITE_AFTER_SELECT.with(|pending| pending.borrow_mut().take());
                if let Some(write) = write {
                    write();
                }
            }
        }
        let result = std::rc::Rc::new(std::cell::RefCell::new(None));
        let completed = result.clone();
        WRITE_AFTER_SELECT.with(|pending| {
            assert!(pending.borrow().is_none());
            *pending.borrow_mut() = Some(Box::new(move || {
                *completed.borrow_mut() = Some(write());
            }));
        });
        reader.connection.trace_v2(
            rusqlite::trace::TraceEventCodes::SQLITE_TRACE_PROFILE,
            Some(after_select),
        );
        ConcurrentReadWrite {
            connection: &reader.connection,
            result,
        }
    }

    #[test]
    fn get_list_keeps_model_validation_and_state_in_one_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("snapshots.sqlite3");
        let mut reader = Database::open(&path).unwrap();
        let before = populated_list(&mut reader, "Before");
        reader
            .connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        let writer = Database::open(&path).unwrap();
        let concurrent = write_after_first_select(&reader, move || {
            writer.connection.execute_batch("BEGIN IMMEDIATE; UPDATE lists SET model_version = 99, name = 'Changed'; UPDATE items SET name = 'Changed'; COMMIT;").map_err(db_error)
        });
        let observed = reader.get_list(before.id).unwrap();
        concurrent.assert_committed();
        drop(concurrent);
        assert_eq!(
            serde_json::to_value(observed).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
        assert!(
            reader
                .get_list(before.id)
                .unwrap_err()
                .contains("評価モデル")
        );
    }

    #[test]
    fn list_summaries_keep_list_membership_and_each_summary_in_one_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("summary-snapshots.sqlite3");
        let mut reader = Database::open(&path).unwrap();
        let first = populated_list(&mut reader, "First");
        let second = populated_list(&mut reader, "Second");
        let before = serde_json::to_value(reader.list_summaries().unwrap()).unwrap();
        reader
            .connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        let mut writer = Database::open(&path).unwrap();
        let concurrent = write_after_first_select(&reader, move || {
            writer.delete_list(second.id).map(|change| change.value)
        });
        let observed = reader.list_summaries();
        concurrent.assert_committed();
        drop(concurrent);
        assert_eq!(serde_json::to_value(observed.unwrap()).unwrap(), before);
        let next = reader.list_summaries().unwrap();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].id, first.id);
    }
}
