//! Keep SQLite's native locking and recovery while resolving managed names
//! relative to the directory that was actually approved during startup.

use std::ffi::{CStr, OsStr, OsString, c_char, c_int};
use std::io;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use rusqlite::{Connection, OpenFlags, ffi};

use crate::storage::PreparedAppData;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

const VFS_NAME: &CStr = c"pairrank-directory";
const LOCAL_PREFIX: &str = "/__pairrank_vfs/";
const UNC_PREFIX: &str = "//pairrank-vfs/";
const SQLITE_SOURCE: &str =
    "2026-03-13 10:38:09 737ae4a34738ffa0c3ff7f9bb18df914dd1cad163f28fd6b6e114a344fe6d618";

struct Namespace {
    identity: same_file::Handle,
    storage: Arc<PreparedAppData>,
    token: String,
    names: Mutex<Vec<OsString>>,
    unc: bool,
}

// A native descriptor keeps this value alive until SQLite really closes it,
// including descriptors retained by its POSIX deferred-close machinery.
#[derive(Clone)]
pub(super) struct ResolvedPath {
    pub storage: Arc<PreparedAppData>,
    pub name: OsString,
    _namespace: Arc<Namespace>,
}

static NAMESPACES: Mutex<Vec<Weak<Namespace>>> = Mutex::new(Vec::new());
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
static INITIALIZED: OnceLock<Result<NativeVfs, String>> = OnceLock::new();

#[derive(Clone, Copy)]
struct NativeVfs(*mut ffi::sqlite3_vfs);

// SQLite owns the process-wide VFS object and permits concurrent calls. Its
// callbacks are installed once, before our VFS is published.
unsafe impl Send for NativeVfs {}
unsafe impl Sync for NativeVfs {}

pub fn open(storage: Arc<PreparedAppData>, filename: &OsStr) -> Result<Connection, String> {
    open_with(storage, filename, |_| {})
}

pub(crate) fn open_with(
    storage: Arc<PreparedAppData>,
    filename: &OsStr,
    mut checkpoint: impl FnMut(bool),
) -> Result<Connection, String> {
    initialize()?;
    let mut components = Path::new(filename).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err("保存データのファイル名が不正です。".to_owned());
    }
    storage.verify().map_err(storage_error)?;
    let namespace = namespace(storage.clone())?;
    let logical = {
        let mut names = namespace
            .names
            .lock()
            .map_err(|_| storage_error("name registry poisoned"))?;
        let index = match names.iter().position(|name| name == filename) {
            Some(index) => index,
            None => {
                names.push(filename.to_owned());
                names.len() - 1
            }
        };
        let prefix = if namespace.unc {
            UNC_PREFIX
        } else {
            LOCAL_PREFIX
        };
        format!("{prefix}{}/db-{index}.sqlite3", namespace.token)
    };
    #[cfg(windows)]
    let logical = if namespace.unc {
        logical.replace('/', "\\")
    } else {
        logical
    };
    // No URI options, ATTACH, alternate VFS, or filename interpretation is
    // needed for this internal entrypoint.
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    #[cfg(unix)]
    let prepared = unix::PreparedOpen::new(
        resolve(OsStr::new(&logical))
            .map_err(storage_error)?
            .ok_or_else(|| storage_error("missing managed name"))?,
    )
    .map_err(storage_error)?;
    #[cfg(unix)]
    prepared.prepare_sidecars().map_err(storage_error)?;
    checkpoint(false);
    #[cfg(unix)]
    let connection =
        prepared.run(|| Connection::open_with_flags_and_vfs(&logical, flags, VFS_NAME));
    #[cfg(not(unix))]
    let connection = Connection::open_with_flags_and_vfs(&logical, flags, VFS_NAME);
    checkpoint(true);
    let connection = connection.map_err(|error| format!("保存データを開けません: {error}"))?;
    #[cfg(unix)]
    prepared.verify().map_err(storage_error)?;
    storage.verify().map_err(storage_error)?;
    Ok(connection)
}

fn namespace(storage: Arc<PreparedAppData>) -> Result<Arc<Namespace>, String> {
    let file = storage
        .directory()
        .try_clone()
        .map_err(storage_error)?
        .into_std_file();
    let identity = same_file::Handle::from_file(file).map_err(storage_error)?;
    let mut entries = NAMESPACES
        .lock()
        .map_err(|_| storage_error("directory registry poisoned"))?;
    entries.retain(|entry| entry.strong_count() != 0);
    if let Some(found) = entries
        .iter()
        .filter_map(Weak::upgrade)
        .find(|entry| entry.identity == identity)
    {
        return Ok(found);
    }
    #[cfg(windows)]
    let unc = windows::is_unc(storage.directory()).map_err(storage_error)?;
    #[cfg(not(windows))]
    let unc = false;
    let namespace = Arc::new(Namespace {
        identity,
        storage,
        token: format!(
            "{}-{}",
            std::process::id(),
            NEXT_TOKEN.fetch_add(1, Ordering::Relaxed)
        ),
        names: Mutex::new(Vec::new()),
        unc,
    });
    entries.push(Arc::downgrade(&namespace));
    Ok(namespace)
}

pub(super) fn resolve(path: &OsStr) -> io::Result<Option<ResolvedPath>> {
    let Some(path) = path.to_str() else {
        let lossy = path.to_string_lossy();
        #[cfg(windows)]
        let lossy = lossy.replace('\\', "/");
        if lossy.starts_with(LOCAL_PREFIX) || lossy.starts_with(UNC_PREFIX) {
            return Err(invalid_name());
        }
        return Ok(None);
    };
    #[cfg(windows)]
    let normalized = path.replace('\\', "/");
    #[cfg(windows)]
    let path = normalized
        .strip_prefix("//?/UNC/")
        .map(|suffix| format!("//{suffix}"))
        .unwrap_or_else(|| {
            normalized
                .strip_prefix("//?/")
                .unwrap_or(&normalized)
                .to_owned()
        });
    #[cfg(windows)]
    let path = path.as_str();
    let Some(rest) = path
        .strip_prefix(LOCAL_PREFIX)
        .or_else(|| path.strip_prefix(UNC_PREFIX))
    else {
        return Ok(None);
    };
    let (token, leaf) = rest.split_once('/').ok_or_else(invalid_name)?;
    let namespace = NAMESPACES
        .lock()
        .map_err(|_| invalid_name())?
        .iter()
        .filter_map(Weak::upgrade)
        .find(|entry| entry.token == token)
        .ok_or_else(invalid_name)?;
    let names = namespace.names.lock().map_err(|_| invalid_name())?;
    let mut result = None;
    for (index, name) in names.iter().enumerate() {
        let base = format!("db-{index}.sqlite3");
        if let Some(suffix) = leaf.strip_prefix(&base)
            && matches!(suffix, "" | "-journal" | "-wal" | "-shm")
        {
            let mut name = name.clone();
            name.push(suffix);
            result = Some(name);
            break;
        }
    }
    let name = result.ok_or_else(invalid_name)?;
    drop(names);
    Ok(Some(ResolvedPath {
        storage: namespace.storage.clone(),
        name,
        _namespace: namespace,
    }))
}

fn invalid_name() -> io::Error {
    io::Error::other("unknown managed SQLite name")
}

fn storage_error(error: impl std::fmt::Display) -> String {
    format!("保存先を安全に開けません: {error}")
}

pub(crate) fn initialize() -> Result<(), String> {
    INITIALIZED
        .get_or_init(|| {
            // SAFETY: SQLite's initialization and VFS registry APIs are process-wide;
            // this block runs once and publishes our VFS only after all checks/hooks.
            unsafe {
                if ffi::sqlite3_initialize() != ffi::SQLITE_OK
                    || CStr::from_ptr(ffi::sqlite3_sourceid()).to_bytes()
                        != SQLITE_SOURCE.as_bytes()
                    || ffi::sqlite3_threadsafe() == 0
                    || ffi::sqlite3_compileoption_used(c"ENABLE_8_3_NAMES".as_ptr()) != 0
                    || ffi::sqlite3_compileoption_used(c"OMIT_WAL".as_ptr()) != 0
                {
                    return Err(storage_error("unsupported bundled SQLite build"));
                }
                #[cfg(target_os = "macos")]
                let name = c"unix-posix";
                #[cfg(all(unix, not(target_os = "macos")))]
                let name = c"unix";
                #[cfg(windows)]
                let name = c"win32";
                let base = ffi::sqlite3_vfs_find(name.as_ptr());
                if base.is_null()
                    || (*base).iVersion < 3
                    || (*base).xOpen.is_none()
                    || (*base).xGetSystemCall.is_none()
                    || (*base).xSetSystemCall.is_none()
                {
                    return Err(storage_error("native SQLite VFS is unavailable"));
                }
                #[cfg(unix)]
                unix::install(base)?;
                #[cfg(windows)]
                windows::install(base)?;
                let mut vfs = Box::new(*base);
                vfs.zName = VFS_NAME.as_ptr();
                vfs.pNext = std::ptr::null_mut();
                vfs.xFullPathname = Some(full_pathname);
                vfs.xOpen = Some(open_file);
                // The native pAppData must remain intact: it selects POSIX locking
                // on Unix and the native I/O method table on Windows.
                let vfs = Box::into_raw(vfs);
                if ffi::sqlite3_vfs_register(vfs, 0) != ffi::SQLITE_OK {
                    drop(Box::from_raw(vfs));
                    return Err(storage_error("SQLite VFS registration failed"));
                }
                Ok(NativeVfs(base))
            }
        })
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}

unsafe extern "C" fn full_pathname(
    _vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    size: c_int,
    output: *mut c_char,
) -> c_int {
    // SAFETY: SQLite provides a NUL-terminated input and a size-byte output.
    unsafe {
        if size <= 0 {
            return ffi::SQLITE_CANTOPEN;
        }
        *output = 0;
        let name = CStr::from_ptr(name);
        let Ok(text) = name.to_str() else {
            return ffi::SQLITE_CANTOPEN;
        };
        if !matches!(resolve(OsStr::new(text)), Ok(Some(_)))
            || name.to_bytes_with_nul().len() > size as usize
        {
            return ffi::SQLITE_CANTOPEN;
        }
        std::ptr::copy_nonoverlapping(name.as_ptr(), output, name.to_bytes_with_nul().len());
        ffi::SQLITE_OK
    }
}

unsafe extern "C" fn open_file(
    _vfs: *mut ffi::sqlite3_vfs,
    name: ffi::sqlite3_filename,
    file: *mut ffi::sqlite3_file,
    flags: c_int,
    output_flags: *mut c_int,
) -> c_int {
    // SAFETY: SQLite owns all arguments for the duration of this synchronous
    // callback. The native VFS initializes and owns its sqlite3_file contents.
    unsafe {
        let Some(Ok(native)) = INITIALIZED.get() else {
            return ffi::SQLITE_CANTOPEN;
        };
        let base = native.0;
        let Some(callback) = (*base).xOpen else {
            return ffi::SQLITE_CANTOPEN;
        };
        if name.is_null() {
            return callback(base, name, file, flags, output_flags);
        }
        let Ok(name_text) = CStr::from_ptr(name).to_str() else {
            return ffi::SQLITE_CANTOPEN;
        };
        let Ok(Some(resolved)) = resolve(OsStr::new(name_text)) else {
            return ffi::SQLITE_CANTOPEN;
        };
        #[cfg(unix)]
        return unix::open_file(base, name, file, flags, output_flags, resolved);
        #[cfg(windows)]
        {
            let _ = resolved;
            callback(base, name, file, flags, output_flags)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_names_fail_closed_and_expire_after_native_close() {
        let root = tempfile::tempdir().unwrap();
        let storage = Arc::new(PreparedAppData::open(root.path().join("app-data")).unwrap());
        let connection = open(storage, OsStr::new("pairrank.sqlite3")).unwrap();
        let logical = connection.path().unwrap().to_owned();
        assert!(resolve(OsStr::new(&logical)).unwrap().is_some());
        for suffix in ["/outside", "-unknown", "/../outside"] {
            assert!(resolve(OsStr::new(&format!("{logical}{suffix}"))).is_err());
        }
        assert!(resolve(OsStr::new("/__pairrank_vfs/missing/db-0.sqlite3")).is_err());
        assert!(resolve(OsStr::new("ordinary.sqlite3")).unwrap().is_none());
        drop(connection);
        assert!(resolve(OsStr::new(&logical)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn malformed_unicode_in_a_reserved_name_is_never_unmanaged() {
        use std::os::unix::ffi::OsStrExt;
        assert!(resolve(OsStr::from_bytes(b"/__pairrank_vfs/\xff/db-0.sqlite3")).is_err());
        assert!(
            resolve(OsStr::from_bytes(b"ordinary-\xff"))
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn replacing_an_older_parent_alias_does_not_invalidate_a_real_path_connection() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        let alias = root.path().join("alias");
        std::fs::create_dir(&real).unwrap();
        symlink(&real, &alias).unwrap();
        let first_storage = Arc::new(PreparedAppData::open(alias.join("app-data")).unwrap());
        let real_storage = Arc::new(PreparedAppData::open(real.join("app-data")).unwrap());
        let first = open(first_storage, OsStr::new("pairrank.sqlite3")).unwrap();
        let second = open(real_storage.clone(), OsStr::new("pairrank.sqlite3")).unwrap();
        drop(first);
        std::fs::remove_file(&alias).unwrap();
        let third = open(real_storage, OsStr::new("pairrank.sqlite3")).unwrap();
        third
            .execute_batch("CREATE TABLE saved(value); INSERT INTO saved VALUES (1)")
            .unwrap();
        assert_eq!(
            second
                .query_row("SELECT value FROM saved", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
