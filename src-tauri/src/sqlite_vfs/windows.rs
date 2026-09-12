//! Redirect only managed SQLite names while retaining the native Win32 VFS's
//! file locking, shared-memory mapping, retry, and recovery implementations.

use super::{ResolvedPath, resolve};
use crate::storage;
use rusqlite::ffi;
use std::collections::HashMap;
use std::ffi::{CStr, OsString, c_void};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, IntoRawHandle};
use std::sync::{Mutex, OnceLock};
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_DELETE_ON_CLOSE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF,
    FILE_RANDOM_ACCESS, FILE_SEQUENTIAL_ONLY, FILE_SYNCHRONOUS_IO_NONALERT, FILE_WRITE_THROUGH,
};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_NAME, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, HANDLE,
    INVALID_HANDLE_VALUE, SetLastError,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CREATE_NEW, DELETE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
    FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_OVERLAPPED,
    FILE_FLAG_RANDOM_ACCESS, FILE_FLAG_SEQUENTIAL_SCAN, FILE_FLAG_WRITE_THROUGH,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GET_FILEEX_INFO_LEVELS, GetFileExInfoStandard, GetFileInformationByHandle,
    GetFinalPathNameByHandleW, INVALID_FILE_ATTRIBUTES, OPEN_ALWAYS, OPEN_EXISTING, SYNCHRONIZE,
    WIN32_FILE_ATTRIBUTE_DATA,
};
use windows_sys::core::BOOL;

type CreateFile = unsafe extern "system" fn(
    *const u16,
    u32,
    u32,
    *const SECURITY_ATTRIBUTES,
    u32,
    u32,
    HANDLE,
) -> HANDLE;
type DeleteFile = unsafe extern "system" fn(*const u16) -> BOOL;
type GetAttributes = unsafe extern "system" fn(*const u16) -> u32;
type GetAttributesEx =
    unsafe extern "system" fn(*const u16, GET_FILEEX_INFO_LEVELS, *mut c_void) -> BOOL;
type CloseFile = unsafe extern "system" fn(HANDLE) -> BOOL;

struct Originals {
    create: CreateFile,
    delete: DeleteFile,
    attributes: GetAttributes,
    attributes_ex: GetAttributesEx,
    close: CloseFile,
}

static ORIGINALS: OnceLock<Originals> = OnceLock::new();
// A native SHM handle can outlive the connection which first opened it. Hold
// the namespace until native CloseHandle actually closes every such handle.
static HANDLES: OnceLock<Mutex<HashMap<usize, ResolvedPath>>> = OnceLock::new();

#[cfg(test)]
type Checkpoint = std::cell::RefCell<Option<Box<dyn FnOnce()>>>;

#[cfg(test)]
thread_local! {
    static BEFORE_CREATE: Checkpoint = const { std::cell::RefCell::new(None) };
    static BEFORE_ATTRIBUTES: Checkpoint = const { std::cell::RefCell::new(None) };
    static BEFORE_DELETE: Checkpoint = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn checkpoint(hook: &'static std::thread::LocalKey<Checkpoint>) {
    if let Some(callback) = hook.with(|hook| hook.borrow_mut().take()) {
        callback();
    }
}

fn handles() -> &'static Mutex<HashMap<usize, ResolvedPath>> {
    HANDLES.get_or_init(Mutex::default)
}

pub(super) fn is_unc(directory: &cap_std::fs::Dir) -> io::Result<bool> {
    let mut buffer = vec![0_u16; 32_768];
    // SAFETY: directory retains a valid handle and buffer is writable. The
    // default flags return a DOS volume name, including the UNC prefix for a
    // network share, without looking up the directory's former ambient name.
    let length = unsafe {
        GetFinalPathNameByHandleW(
            directory.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            0,
        )
    } as usize;
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    if length >= buffer.len() {
        return Err(io::Error::from_raw_os_error(ERROR_INVALID_NAME as i32));
    }
    let path = String::from_utf16_lossy(&buffer[..length]);
    Ok(path.starts_with(r"\\?\UNC\"))
}

pub(super) unsafe fn install(base: *mut ffi::sqlite3_vfs) -> Result<(), String> {
    // SAFETY: the common installer passes a live native VFS while holding its
    // one-time initialization lock, before creating any SQLite connections.
    let get = unsafe { (*base).xGetSystemCall }
        .ok_or_else(|| "Windows SQLite VFS に syscall 検証機能がありません。".to_owned())?;
    // SAFETY: same VFS lifetime as above.
    let set = unsafe { (*base).xSetSystemCall }
        .ok_or_else(|| "Windows SQLite VFS に syscall 登録機能がありません。".to_owned())?;
    let names: [&CStr; 5] = [
        c"CreateFileW",
        c"DeleteFileW",
        c"GetFileAttributesW",
        c"GetFileAttributesExW",
        c"CloseHandle",
    ];
    let mut original = Vec::with_capacity(names.len());
    for name in names {
        // SAFETY: each name is a static, NUL-terminated SQLite syscall name.
        let function = unsafe { get(base, name.as_ptr()) }.ok_or_else(|| {
            format!(
                "Windows SQLite VFS に必須 syscall {} がありません。",
                name.to_string_lossy()
            )
        })?;
        original.push(function);
    }
    // SAFETY: these signatures are the Win32 ABI signatures of the named
    // entries, verified against the pinned bundled SQLite source.
    let functions = unsafe {
        Originals {
            create: std::mem::transmute::<unsafe extern "C" fn(), CreateFile>(original[0]),
            delete: std::mem::transmute::<unsafe extern "C" fn(), DeleteFile>(original[1]),
            attributes: std::mem::transmute::<unsafe extern "C" fn(), GetAttributes>(original[2]),
            attributes_ex: std::mem::transmute::<unsafe extern "C" fn(), GetAttributesEx>(
                original[3],
            ),
            close: std::mem::transmute::<unsafe extern "C" fn(), CloseFile>(original[4]),
        }
    };
    ORIGINALS
        .set(functions)
        .map_err(|_| "Windows SQLite syscall はすでに初期化されています。".to_owned())?;
    let hooks = [
        create_file as *const (),
        delete_file as *const (),
        get_attributes as *const (),
        get_attributes_ex as *const (),
        close_file as *const (),
    ];
    for (index, (name, hook)) in names.into_iter().zip(hooks).enumerate() {
        // SAFETY: SQLite stores the untyped syscall pointer and invokes it
        // with the named Win32 signature, matching each hook above.
        let function = unsafe { std::mem::transmute::<*const (), unsafe extern "C" fn()>(hook) };
        // SAFETY: static syscall names and ABI-compatible functions.
        if unsafe { set(base, name.as_ptr(), Some(function)) } != ffi::SQLITE_OK {
            for previous in 0..index {
                // SAFETY: restore only the entries this installation changed.
                unsafe {
                    set(base, names[previous].as_ptr(), Some(original[previous]));
                }
            }
            return Err(format!(
                "Windows SQLite syscall {} を登録できませんでした。",
                name.to_string_lossy()
            ));
        }
    }
    Ok(())
}

unsafe fn resolve_wide(path: *const u16) -> io::Result<Option<ResolvedPath>> {
    if path.is_null() {
        return Err(io::Error::from_raw_os_error(ERROR_INVALID_NAME as i32));
    }
    // SAFETY: SQLite's Win32 VFS passes a valid, NUL-terminated UTF-16 buffer.
    // NT filenames cannot exceed this many code units.
    let length = (0..32_768)
        .find(|offset| unsafe { *path.add(*offset) == 0 })
        .ok_or_else(|| io::Error::from_raw_os_error(ERROR_INVALID_NAME as i32))?;
    // SAFETY: the scanned range precedes the terminating NUL.
    let name = OsString::from_wide(unsafe { std::slice::from_raw_parts(path, length) });
    resolve(&name)
}

fn set_error(error: io::Error) {
    let code = match error.raw_os_error() {
        Some(code) => code as u32,
        None => match error.kind() {
            io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => ERROR_INVALID_NAME,
            io::ErrorKind::NotFound => windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND,
            io::ErrorKind::Unsupported => ERROR_NOT_SUPPORTED,
            _ => ERROR_ACCESS_DENIED,
        },
    };
    // SAFETY: SetLastError accepts every DWORD error value.
    unsafe { SetLastError(code) };
}

unsafe extern "system" fn create_file(
    path: *const u16,
    access: u32,
    share: u32,
    security: *const SECURITY_ATTRIBUTES,
    disposition: u32,
    flags: u32,
    template: HANDLE,
) -> HANDLE {
    // SAFETY: the hook receives the Win32 filename buffer from native SQLite.
    let resolved = match unsafe { resolve_wide(path) } {
        Ok(Some(resolved)) => resolved,
        Ok(None) => {
            if let Some(original) = ORIGINALS.get() {
                // SAFETY: pass every unmanaged argument through unchanged.
                return unsafe {
                    (original.create)(path, access, share, security, disposition, flags, template)
                };
            }
            return INVALID_HANDLE_VALUE;
        }
        Err(error) => {
            set_error(error);
            return INVALID_HANDLE_VALUE;
        }
    };
    // The pinned desktop VFS uses no security/template argument. Refuse new
    // contracts instead of silently changing their semantics on SQLite updates.
    const SUPPORTED_FLAGS: u32 = FILE_ATTRIBUTE_NORMAL
        | FILE_ATTRIBUTE_HIDDEN
        | FILE_ATTRIBUTE_TEMPORARY
        | FILE_FLAG_DELETE_ON_CLOSE
        | FILE_FLAG_OVERLAPPED
        | FILE_FLAG_RANDOM_ACCESS
        | FILE_FLAG_SEQUENTIAL_SCAN
        | FILE_FLAG_WRITE_THROUGH;
    if !security.is_null() || !template.is_null() || flags & !SUPPORTED_FLAGS != 0 {
        set_error(io::Error::from_raw_os_error(ERROR_NOT_SUPPORTED as i32));
        return INVALID_HANDLE_VALUE;
    }
    let disposition = match disposition {
        CREATE_NEW => FILE_CREATE,
        OPEN_EXISTING => FILE_OPEN,
        OPEN_ALWAYS => FILE_OPEN_IF,
        _ => {
            set_error(io::Error::from_raw_os_error(ERROR_NOT_SUPPORTED as i32));
            return INVALID_HANDLE_VALUE;
        }
    };
    let mut options = FILE_NON_DIRECTORY_FILE;
    let mut desired = access | FILE_READ_ATTRIBUTES;
    if flags & FILE_FLAG_OVERLAPPED == 0 {
        desired |= SYNCHRONIZE;
        options |= FILE_SYNCHRONOUS_IO_NONALERT;
    }
    if flags & FILE_FLAG_DELETE_ON_CLOSE != 0 {
        desired |= DELETE;
        options |= FILE_DELETE_ON_CLOSE;
    }
    if flags & FILE_FLAG_RANDOM_ACCESS != 0 {
        options |= FILE_RANDOM_ACCESS;
    }
    if flags & FILE_FLAG_SEQUENTIAL_SCAN != 0 {
        options |= FILE_SEQUENTIAL_ONLY;
    }
    if flags & FILE_FLAG_WRITE_THROUGH != 0 {
        options |= FILE_WRITE_THROUGH;
    }
    let attributes =
        flags & (FILE_ATTRIBUTE_NORMAL | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_TEMPORARY);
    #[cfg(test)]
    checkpoint(&BEFORE_CREATE);
    let result = storage::windows::open_relative(
        resolved.storage.directory(),
        &resolved.name,
        desired,
        share,
        disposition,
        attributes,
        options,
    );
    match result {
        Ok(file) => {
            let handle = file.into_raw_handle();
            handles()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(handle as usize, resolved);
            handle
        }
        Err(error) => {
            set_error(error);
            INVALID_HANDLE_VALUE
        }
    }
}

unsafe extern "system" fn delete_file(path: *const u16) -> BOOL {
    // SAFETY: same native SQLite filename contract as create_file.
    match unsafe { resolve_wide(path) } {
        Ok(Some(resolved)) => {
            #[cfg(test)]
            checkpoint(&BEFORE_DELETE);
            match storage::remove_file_in(resolved.storage.directory(), &resolved.name) {
                Ok(()) => 1,
                Err(error) => {
                    set_error(error);
                    0
                }
            }
        }
        Ok(None) => ORIGINALS.get().map_or(0, |original| {
            // SAFETY: unmanaged arguments are passed through unchanged.
            unsafe { (original.delete)(path) }
        }),
        Err(error) => {
            set_error(error);
            0
        }
    }
}

fn attributes(resolved: &ResolvedPath) -> io::Result<WIN32_FILE_ATTRIBUTE_DATA> {
    #[cfg(test)]
    checkpoint(&BEFORE_ATTRIBUTES);
    let file = storage::windows::open_relative(
        resolved.storage.directory(),
        &resolved.name,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        FILE_OPEN,
        FILE_ATTRIBUTE_NORMAL,
        FILE_SYNCHRONOUS_IO_NONALERT,
    )?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: file owns a live handle and info is writable for the call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(WIN32_FILE_ATTRIBUTE_DATA {
        dwFileAttributes: info.dwFileAttributes,
        ftCreationTime: info.ftCreationTime,
        ftLastAccessTime: info.ftLastAccessTime,
        ftLastWriteTime: info.ftLastWriteTime,
        nFileSizeHigh: info.nFileSizeHigh,
        nFileSizeLow: info.nFileSizeLow,
    })
}

unsafe extern "system" fn get_attributes(path: *const u16) -> u32 {
    // SAFETY: same native SQLite filename contract as create_file.
    match unsafe { resolve_wide(path) } {
        Ok(Some(resolved)) => match attributes(&resolved) {
            Ok(attributes) => attributes.dwFileAttributes,
            Err(error) => {
                set_error(error);
                INVALID_FILE_ATTRIBUTES
            }
        },
        Ok(None) => ORIGINALS.get().map_or(INVALID_FILE_ATTRIBUTES, |original| {
            // SAFETY: unmanaged arguments are passed through unchanged.
            unsafe { (original.attributes)(path) }
        }),
        Err(error) => {
            set_error(error);
            INVALID_FILE_ATTRIBUTES
        }
    }
}

unsafe extern "system" fn get_attributes_ex(
    path: *const u16,
    level: GET_FILEEX_INFO_LEVELS,
    output: *mut c_void,
) -> BOOL {
    // SAFETY: same native SQLite filename contract as create_file.
    match unsafe { resolve_wide(path) } {
        Ok(Some(resolved)) => {
            if level != GetFileExInfoStandard || output.is_null() {
                set_error(io::Error::from_raw_os_error(ERROR_INVALID_PARAMETER as i32));
                return 0;
            }
            match attributes(&resolved) {
                Ok(attributes) => {
                    // SAFETY: GetFileExInfoStandard's caller supplies a
                    // writable WIN32_FILE_ATTRIBUTE_DATA output buffer.
                    unsafe { output.cast::<WIN32_FILE_ATTRIBUTE_DATA>().write(attributes) };
                    1
                }
                Err(error) => {
                    set_error(error);
                    0
                }
            }
        }
        Ok(None) => ORIGINALS.get().map_or(0, |original| {
            // SAFETY: unmanaged arguments are passed through unchanged.
            unsafe { (original.attributes_ex)(path, level, output) }
        }),
        Err(error) => {
            set_error(error);
            0
        }
    }
}

unsafe extern "system" fn close_file(handle: HANDLE) -> BOOL {
    // Remove before native close, retaining the guard locally. Otherwise a
    // newly opened file could reuse the closed HANDLE before map removal.
    let retained = handles()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&(handle as usize));
    let result = ORIGINALS.get().map_or(0, |original| {
        // SAFETY: pass the native handle to its original CloseHandle hook.
        unsafe { (original.close)(handle) }
    });
    if result == 0
        && let Some(retained) = retained
    {
        handles()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(handle as usize, retained);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::PreparedAppData;
    use std::ffi::OsStr;
    use std::fs;
    use std::os::windows::fs::MetadataExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Arc;

    struct Fixture {
        _temporary: tempfile::TempDir,
        alias: PathBuf,
        replacement: PathBuf,
        storage: Arc<PreparedAppData>,
    }

    fn junction(link: &Path, target: &Path) {
        let output = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "junction creation failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn fixture() -> Fixture {
        let temporary = tempfile::tempdir().unwrap();
        let original = temporary.path().join("original");
        let replacement = temporary.path().join("replacement");
        fs::create_dir_all(original.join("data")).unwrap();
        fs::create_dir_all(replacement.join("data")).unwrap();
        for name in [
            "test.sqlite3",
            "test.sqlite3-journal",
            "test.sqlite3-wal",
            "test.sqlite3-shm",
        ] {
            fs::write(replacement.join("data").join(name), b"replacement sentinel").unwrap();
        }
        let alias = temporary.path().join("current");
        junction(&alias, &original);
        let storage = Arc::new(PreparedAppData::open(alias.join("data")).unwrap());
        Fixture {
            _temporary: temporary,
            alias,
            replacement,
            storage,
        }
    }

    fn replace_callback(fixture: &Fixture) -> Box<dyn FnOnce()> {
        let alias = fixture.alias.clone();
        let replacement = fixture.replacement.clone();
        Box::new(move || {
            fs::remove_dir(&alias).unwrap();
            junction(&alias, &replacement);
            assert_eq!(
                fs::read(alias.join("data/test.sqlite3")).unwrap(),
                b"replacement sentinel"
            );
        })
    }

    fn snapshot(fixture: &Fixture) -> (u32, Vec<(OsString, u32, Vec<u8>)>) {
        let path = fixture.replacement.join("data");
        let mut entries: Vec<_> = fs::read_dir(&path)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name(),
                    entry.metadata().unwrap().file_attributes(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        (fs::metadata(path).unwrap().file_attributes(), entries)
    }

    #[test]
    fn sqlite_open_does_not_follow_junction_switched_inside_native_create_file() {
        let fixture = fixture();
        let before = snapshot(&fixture);
        BEFORE_CREATE.with(|hook| *hook.borrow_mut() = Some(replace_callback(&fixture)));
        assert!(super::super::open(fixture.storage.clone(), OsStr::new("test.sqlite3")).is_err());
        assert!(fixture.storage.verify().is_err());
        assert_eq!(snapshot(&fixture), before);
    }

    #[test]
    fn sqlite_metadata_does_not_follow_junction_switched_inside_native_attribute_lookup() {
        let fixture = fixture();
        let before = snapshot(&fixture);
        BEFORE_ATTRIBUTES.with(|hook| *hook.borrow_mut() = Some(replace_callback(&fixture)));
        assert!(super::super::open(fixture.storage.clone(), OsStr::new("test.sqlite3")).is_err());
        assert!(fixture.storage.verify().is_err());
        assert_eq!(snapshot(&fixture), before);
    }

    #[test]
    fn sqlite_journal_delete_does_not_follow_junction_switched_inside_native_delete_file() {
        let fixture = fixture();
        let connection =
            super::super::open(fixture.storage.clone(), OsStr::new("test.sqlite3")).unwrap();
        let before = snapshot(&fixture);
        BEFORE_DELETE.with(|hook| *hook.borrow_mut() = Some(replace_callback(&fixture)));
        connection
            .execute_batch("CREATE TABLE test (value INTEGER)")
            .unwrap();
        assert!(fixture.storage.verify().is_err());
        drop(connection);
        assert_eq!(snapshot(&fixture), before);
    }
}
