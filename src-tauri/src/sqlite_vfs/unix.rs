use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString, OsStr, c_char, c_int};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

use rusqlite::ffi;

use super::{ResolvedPath, resolve, storage_error};

type Open = unsafe extern "C" fn(*const c_char, c_int, c_int) -> c_int;
type Close = unsafe extern "C" fn(c_int) -> c_int;
type Stat = unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int;
type Fstat = unsafe extern "C" fn(c_int, *mut libc::stat) -> c_int;
type Access = unsafe extern "C" fn(*const c_char, c_int) -> c_int;
type Unlink = unsafe extern "C" fn(*const c_char) -> c_int;
type OpenDirectory = unsafe extern "C" fn(*const c_char, *mut c_int) -> c_int;
type Fchmod = unsafe extern "C" fn(c_int, libc::mode_t) -> c_int;

struct Originals {
    open: Open,
    close: Close,
    stat: Stat,
    lstat: Option<Stat>,
    fstat: Fstat,
    access: Access,
    unlink: Unlink,
    open_directory: OpenDirectory,
    fchmod: Option<Fchmod>,
}

static ORIGINALS: OnceLock<Originals> = OnceLock::new();
struct TrackedFile {
    _path: Option<ResolvedPath>,
    identity: Option<Identity>,
    deferred: bool,
}

impl TrackedFile {
    fn new(fd: c_int, path: Option<ResolvedPath>) -> Self {
        Self {
            _path: path,
            identity: stat_fd(fd).ok().map(|metadata| identity(&metadata)),
            deferred: false,
        }
    }
}

static FILES: OnceLock<Mutex<HashMap<c_int, TrackedFile>>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
}

fn identity(metadata: &libc::stat) -> Identity {
    Identity {
        #[cfg(target_os = "macos")]
        device: metadata.st_dev as u64,
        #[cfg(not(target_os = "macos"))]
        device: metadata.st_dev,
        inode: metadata.st_ino,
    }
}

struct Capture {
    path: ResolvedPath,
    expected: Option<Identity>,
    descriptor: c_int,
    rejected: bool,
}

thread_local! {
    static REQUEST: RefCell<Option<Rc<RefCell<Capture>>>> = const { RefCell::new(None) };
    static CAPTURE: RefCell<Option<Rc<RefCell<Capture>>>> = const { RefCell::new(None) };
}

pub(super) struct PreparedOpen(Rc<RefCell<Capture>>);

impl PreparedOpen {
    pub(super) fn new(path: ResolvedPath) -> io::Result<Self> {
        let metadata = stat_path(&path)?;
        #[cfg(target_os = "macos")]
        if metadata
            .as_ref()
            .is_some_and(|metadata| metadata.st_mode & 0o400 == 0)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "保存データの権限を安全に復元できません。所有者の読み取り権限（推奨 600）を手動で復元して再試行してください。",
            ));
        }
        let expected = metadata.map(|metadata| identity(&metadata));
        Ok(Self(Rc::new(RefCell::new(Capture {
            path,
            expected,
            descriptor: -1,
            rejected: false,
        }))))
    }

    pub(super) fn run<T>(&self, action: impl FnOnce() -> T) -> T {
        struct Restore(Option<Rc<RefCell<Capture>>>);
        impl Drop for Restore {
            fn drop(&mut self) {
                REQUEST.with(|slot| *slot.borrow_mut() = self.0.take());
            }
        }
        let _restore = Restore(REQUEST.with(|slot| slot.replace(Some(self.0.clone()))));
        action()
    }

    pub(super) fn verify(&self) -> io::Result<()> {
        let state = self.0.borrow();
        if state.rejected || state.descriptor < 0 {
            return Err(replaced());
        }
        let metadata = stat_fd(state.descriptor)?;
        validate(&metadata)?;
        let current = stat_path(&state.path)?.ok_or_else(replaced)?;
        if identity(&current) != identity(&metadata)
            || state
                .expected
                .is_some_and(|expected| identity(&metadata) != expected)
        {
            return Err(replaced());
        }
        Ok(())
    }

    pub(super) fn prepare_sidecars(&self) -> io::Result<()> {
        let state = self.0.borrow();
        let mut files = Vec::new();
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut path = state.path.clone();
            path.name.push(suffix);
            if let Some(metadata) = stat_path(&path)? {
                files.push((path, metadata));
            }
        }
        for (path, metadata) in files {
            if metadata.st_mode & 0o7777 == 0o600 {
                continue;
            }
            #[cfg(target_os = "linux")]
            restore_unreadable(&path, identity(&metadata))?;
            #[cfg(not(target_os = "linux"))]
            restrict_sidecar(&path, identity(&metadata))?;
        }
        Ok(())
    }
}

fn replaced() -> io::Error {
    io::Error::other("prepared database file was replaced")
}

fn validate(metadata: &libc::stat) -> io::Result<()> {
    if metadata.st_mode & libc::S_IFMT != libc::S_IFREG || metadata.st_nlink != 1 {
        return Err(io::Error::other(
            "managed SQLite file must be a regular file with one link",
        ));
    }
    Ok(())
}

fn name(path: &ResolvedPath) -> io::Result<CString> {
    CString::new(path.name.as_bytes()).map_err(|_| replaced())
}

fn stat_path(path: &ResolvedPath) -> io::Result<Option<libc::stat>> {
    let name = name(path)?;
    // SAFETY: the directory handle and C string are live and metadata points
    // to writable storage of the exact platform stat layout.
    unsafe {
        let mut metadata = std::mem::zeroed();
        if libc::fstatat(
            path.storage.directory().as_raw_fd(),
            name.as_ptr(),
            &mut metadata,
            libc::AT_SYMLINK_NOFOLLOW,
        ) != 0
        {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(error);
        }
        validate(&metadata)?;
        Ok(Some(metadata))
    }
}

fn stat_fd(fd: c_int) -> io::Result<libc::stat> {
    // SAFETY: fstat borrows the descriptor and writes to initialized storage.
    unsafe {
        let mut metadata = std::mem::zeroed();
        if libc::fstat(fd, &mut metadata) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(metadata)
    }
}

pub(super) unsafe fn install(base: *mut ffi::sqlite3_vfs) -> Result<(), String> {
    // SAFETY: the caller checked the live native VFS and invokes this once.
    unsafe {
        let get = (*base)
            .xGetSystemCall
            .ok_or_else(|| storage_error("missing syscall lookup"))?;
        let set = (*base)
            .xSetSystemCall
            .ok_or_else(|| storage_error("missing syscall setter"))?;
        macro_rules! required {
            ($name:expr, $type:ty) => {{
                let callback = get(base, $name.as_ptr()).ok_or_else(|| {
                    storage_error(concat!("missing native syscall: ", stringify!($name)))
                })?;
                std::mem::transmute::<unsafe extern "C" fn(), $type>(callback)
            }};
        }
        let originals = Originals {
            open: required!(c"open", Open),
            close: required!(c"close", Close),
            stat: required!(c"stat", Stat),
            fstat: required!(c"fstat", Fstat),
            access: required!(c"access", Access),
            unlink: required!(c"unlink", Unlink),
            open_directory: required!(c"openDirectory", OpenDirectory),
            lstat: get(base, c"lstat".as_ptr())
                .map(|callback| std::mem::transmute::<unsafe extern "C" fn(), Stat>(callback)),
            fchmod: get(base, c"fchmod".as_ptr())
                .map(|callback| std::mem::transmute::<unsafe extern "C" fn(), Fchmod>(callback)),
        };
        ORIGINALS
            .set(originals)
            .map_err(|_| storage_error("syscalls were already initialized"))?;
        FILES.get_or_init(|| Mutex::new(HashMap::new()));
        macro_rules! replace {
            ($name:expr, $callback:expr, $type:ty) => {{
                let callback =
                    std::mem::transmute::<$type, unsafe extern "C" fn()>($callback as $type);
                if set(base, $name.as_ptr(), Some(callback)) != ffi::SQLITE_OK {
                    return Err(storage_error("failed to install SQLite syscall"));
                }
            }};
        }
        replace!(c"open", open, Open);
        replace!(c"close", close, Close);
        replace!(c"stat", stat, Stat);
        replace!(c"fstat", fstat, Fstat);
        replace!(c"access", access, Access);
        replace!(c"unlink", unlink, Unlink);
        replace!(c"openDirectory", open_directory, OpenDirectory);
        if ORIGINALS.get().unwrap().lstat.is_some() {
            replace!(c"lstat", lstat, Stat);
        }
        if ORIGINALS.get().unwrap().fchmod.is_some() {
            replace!(c"fchmod", fchmod, Fchmod);
        }
        Ok(())
    }
}

pub(super) unsafe fn open_file(
    base: *mut ffi::sqlite3_vfs,
    filename: ffi::sqlite3_filename,
    file: *mut ffi::sqlite3_file,
    flags: c_int,
    output_flags: *mut c_int,
    path: ResolvedPath,
) -> c_int {
    // SAFETY: the caller supplies the native xOpen argument contract.
    unsafe { open_file_inner(base, filename, file, flags, output_flags, path, false) }
}

unsafe fn open_file_inner(
    base: *mut ffi::sqlite3_vfs,
    filename: ffi::sqlite3_filename,
    file: *mut ffi::sqlite3_file,
    mut flags: c_int,
    output_flags: *mut c_int,
    path: ResolvedPath,
    retried: bool,
) -> c_int {
    let is_main = flags & ffi::SQLITE_OPEN_MAIN_DB != 0;
    let requested = REQUEST
        .with(|slot| slot.borrow().clone())
        .filter(|request| {
            let request = request.borrow();
            request.path.name == path.name
                && std::sync::Arc::ptr_eq(&request.path._namespace, &path._namespace)
        });
    let preparation = match requested {
        Some(capture) => Ok(PreparedOpen(capture)),
        None => PreparedOpen::new(path.clone()),
    };
    let prepared = match preparation {
        Ok(prepared) => prepared,
        Err(_) => return ffi::SQLITE_CANTOPEN,
    };
    if is_main && prepared.0.borrow().expected.is_some() {
        flags &= !ffi::SQLITE_OPEN_CREATE;
    }
    struct Restore(Option<Rc<RefCell<Capture>>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CAPTURE.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(CAPTURE.with(|slot| slot.replace(Some(prepared.0.clone()))));
    // SAFETY: the native VFS owns initialization and cleanup of this file.
    unsafe {
        (*file).pMethods = std::ptr::null();
        let mut actual_flags = 0;
        let status = ((*base).xOpen.unwrap())(base, filename, file, flags, &mut actual_flags);
        if status != ffi::SQLITE_OK {
            return status;
        }
        let result = prepared.verify().and_then(|()| {
            let state = prepared.0.borrow();
            let fd = state.descriptor;
            // Register reused native descriptors too. No duplicate or owned
            // Rust File is constructed from this borrowed descriptor.
            FILES
                .get()
                .unwrap()
                .lock()
                .map_err(|_| replaced())?
                .insert(fd, TrackedFile::new(fd, Some(path.clone())));
            let metadata = stat_fd(fd)?;
            if metadata.st_mode & 0o7777 != 0o600 {
                restrict_fd(fd)?;
            }
            if is_main && state.expected.is_none() {
                if libc::fsync(fd) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::fsync(path.storage.directory().as_raw_fd()) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
        if result.is_err() {
            // Native xClose defers the real close while another connection
            // holds POSIX locks. Calling libc::close here would lose them.
            ((*(*file).pMethods).xClose.unwrap())(file);
            (*file).pMethods = std::ptr::null();
            return ffi::SQLITE_IOERR;
        }
        if flags & ffi::SQLITE_OPEN_READWRITE != 0 && actual_flags & ffi::SQLITE_OPEN_READONLY != 0
        {
            // A readable 0400 file can be opened by SQLite's readonly fallback.
            // After correcting permissions, reopen through the native VFS so
            // its pager and descriptor both become writable.
            ((*(*file).pMethods).xClose.unwrap())(file);
            (*file).pMethods = std::ptr::null();
            if retried {
                return ffi::SQLITE_READONLY;
            }
            prepared.0.borrow_mut().descriptor = -1;
            return open_file_inner(base, filename, file, flags, output_flags, path, true);
        }
        if !output_flags.is_null() {
            *output_flags = actual_flags;
        }
        ffi::SQLITE_OK
    }
}

unsafe fn resolve_c(path: *const c_char) -> io::Result<Option<ResolvedPath>> {
    // SAFETY: each native syscall callback receives a NUL-terminated path.
    unsafe { resolve(OsStr::from_bytes(CStr::from_ptr(path).to_bytes())) }
}

fn fail(error: io::Error) -> c_int {
    set_errno(error.raw_os_error().unwrap_or(libc::EACCES));
    -1
}
fn set_errno(value: c_int) {
    // SAFETY: these functions return the current thread's errno location.
    unsafe {
        #[cfg(target_os = "linux")]
        {
            *libc::__errno_location() = value;
        }
        #[cfg(target_os = "macos")]
        {
            *libc::__error() = value;
        }
    }
}

unsafe extern "C" fn open(path: *const c_char, flags: c_int, mode: c_int) -> c_int {
    // SAFETY: callback arguments have the native open ABI; all opened file
    // descriptors are transferred to SQLite, never wrapped in an owned File.
    unsafe {
        let resolved = match resolve_c(path) {
            Ok(Some(path)) => path,
            Ok(None) => {
                let fd = (ORIGINALS.get().unwrap().open)(path, flags, mode);
                if fd >= 0 {
                    FILES
                        .get()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .insert(fd, TrackedFile::new(fd, None));
                }
                return fd;
            }
            Err(error) => return fail(error),
        };
        let expected = match stat_path(&resolved) {
            Ok(metadata) => metadata.map(|metadata| identity(&metadata)),
            Err(error) => return fail(error),
        };
        let capture = CAPTURE
            .with(|slot| slot.borrow().clone())
            .filter(|state| state.borrow().path.name == resolved.name);
        if let Some(capture) = &capture {
            let expected_preparation = capture.borrow().expected;
            if expected_preparation.is_some() && expected_preparation != expected {
                return fail(replaced());
            }
        }
        let name = match name(&resolved) {
            Ok(name) => name,
            Err(error) => return fail(error),
        };
        let directory = resolved.storage.directory().as_raw_fd();
        #[cfg(test)]
        native_checkpoint(NativeCheckpoint::Open);
        #[cfg(test)]
        if resolved.name.as_bytes().ends_with(b"-shm") {
            native_checkpoint(NativeCheckpoint::ShmOpen);
        }
        let fd = libc::openat(
            directory,
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0o600,
        );
        #[cfg(target_os = "linux")]
        let fd = if fd < 0
            && io::Error::last_os_error().kind() == io::ErrorKind::PermissionDenied
            && let Some(expected) = expected
        {
            if let Err(error) = restore_unreadable(&resolved, expected) {
                if let Some(capture) = &capture {
                    // SQLite may retry readonly after chmod succeeded. Keep
                    // the failed synchronization visible to final verification.
                    capture.borrow_mut().rejected = true;
                }
                return fail(error);
            }
            libc::openat(
                directory,
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                0o600,
            )
        } else {
            fd
        };
        if fd < 0 {
            return fd;
        }
        let valid = stat_fd(fd).and_then(|metadata| {
            validate(&metadata)?;
            if expected.is_some_and(|expected| identity(&metadata) != expected) {
                return Err(replaced());
            }
            Ok(())
        });
        if let Some(capture) = &capture {
            let mut capture = capture.borrow_mut();
            capture.descriptor = fd;
            capture.rejected |= valid.is_err();
        } else if let Err(error) = valid {
            // SHM descriptors are opened internally by SQLite, outside xOpen.
            discard_rejected(fd, resolved.clone());
            return fail(error);
        }
        let mut tracked = TrackedFile::new(fd, Some(resolved.clone()));
        tracked.deferred = capture
            .as_ref()
            .is_some_and(|capture| capture.borrow().rejected);
        FILES.get().unwrap().lock().unwrap().insert(fd, tracked);
        // Native SHM open does not pass through xOpen. Set its permissions on
        // the actual opened descriptor before SQLite maps or truncates it.
        if capture.is_none() && libc::fchmod(fd, 0o600) != 0 {
            let error = io::Error::last_os_error();
            discard_rejected(fd, resolved);
            return fail(error);
        }
        fd
    }
}

fn restrict_fd(fd: c_int) -> io::Result<()> {
    #[cfg(test)]
    if PERMISSION_FAILURE.with(|failure| failure.get() == Some(PermissionFailure::Chmod)) {
        return Err(io::Error::other("injected chmod failure"));
    }
    // SAFETY: the caller borrows a live native descriptor for this operation.
    unsafe {
        if libc::fchmod(fd, 0o600) != 0 {
            return Err(io::Error::last_os_error());
        }
        #[cfg(test)]
        if PERMISSION_FAILURE.with(|failure| failure.get() == Some(PermissionFailure::Sync)) {
            return Err(io::Error::other("injected permission sync failure"));
        }
        if libc::fsync(fd) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum PermissionFailure {
    Chmod,
    Sync,
}

#[cfg(test)]
type NativeCheckpointCallback = Option<(NativeCheckpoint, Box<dyn FnOnce()>)>;

#[cfg(test)]
thread_local! {
    static PERMISSION_FAILURE: std::cell::Cell<Option<PermissionFailure>> = const { std::cell::Cell::new(None) };
    static NATIVE_CHECKPOINT: RefCell<NativeCheckpointCallback> = const { RefCell::new(None) };
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum NativeCheckpoint {
    Open,
    ShmOpen,
    Unlink,
}

#[cfg(test)]
fn native_checkpoint(point: NativeCheckpoint) {
    let callback = NATIVE_CHECKPOINT.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot
            .as_ref()
            .is_some_and(|(registered, _)| *registered == point)
        {
            slot.take().map(|(_, callback)| callback)
        } else {
            None
        }
    });
    if let Some(callback) = callback {
        callback();
    }
}

#[cfg(target_os = "linux")]
fn restore_unreadable(path: &ResolvedPath, expected: Identity) -> io::Result<()> {
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::PermissionsExt;
    let name = name(path)?;
    // O_PATH does not open the file for I/O. Linux's filp_flush explicitly
    // excludes FMODE_PATH from locks_remove_posix, so closing this pin cannot
    // cancel a SQLite connection's POSIX locks.
    let pinned = unsafe {
        let fd = libc::openat(
            path.storage.directory().as_raw_fd(),
            name.as_ptr(),
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        );
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        std::fs::File::from_raw_fd(fd)
    };
    let metadata = stat_fd(pinned.as_raw_fd())?;
    validate(&metadata)?;
    if identity(&metadata) != expected {
        return Err(replaced());
    }
    std::fs::set_permissions(
        format!("/proc/self/fd/{}", pinned.as_raw_fd()),
        std::fs::Permissions::from_mode(0o600),
    )?;
    #[cfg(test)]
    if PERMISSION_FAILURE.with(|failure| failure.get() == Some(PermissionFailure::Sync)) {
        return Err(io::Error::other("injected permission sync failure"));
    }
    // O_PATH cannot be fsynced. Sync the filesystem through the retained
    // readable directory instead of opening and closing another I/O descriptor
    // for this inode, which could cancel SQLite's POSIX locks. This broader
    // writeback is needed only while repairing non-private permissions.
    // SAFETY: the pinned storage keeps this readable directory descriptor live.
    if unsafe { libc::syncfs(path.storage.directory().as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

unsafe extern "C" fn close(fd: c_int) -> c_int {
    // Retain the namespace until after the native close has finished.
    let mut files = FILES.get().unwrap().lock().unwrap();
    let retained = files.remove(&fd);
    if retained.as_ref().is_some_and(|entry| {
        entry.deferred
            && files
                .values()
                .any(|other| !other.deferred && other.identity == entry.identity)
    }) {
        // A rejected journal/SHM FD can refer to another live database after
        // a raced hardlink replacement. Native nolockClose does not know that
        // relationship, so defer only this rejected close until the inode is
        // no longer used. Successful ordinary closes are never accumulated.
        files.insert(fd, retained.unwrap());
        return 0;
    }
    let result = unsafe { (ORIGINALS.get().unwrap().close)(fd) };
    let pending: Vec<_> = files
        .iter()
        .filter(|(_, entry)| {
            entry.deferred
                && !files
                    .values()
                    .any(|other| !other.deferred && other.identity == entry.identity)
        })
        .map(|(fd, _)| *fd)
        .collect();
    for fd in pending {
        files.remove(&fd);
        unsafe {
            (ORIGINALS.get().unwrap().close)(fd);
        }
    }
    drop(files);
    drop(retained);
    result
}

fn discard_rejected(fd: c_int, path: ResolvedPath) {
    let mut files = FILES.get().unwrap().lock().unwrap();
    let mut entry = TrackedFile::new(fd, Some(path));
    entry.deferred = true;
    files.insert(fd, entry);
    drop(files);
    // SAFETY: SQLite has not received this rejected descriptor. The close
    // hook either closes it or retains it without canceling another FD's locks.
    unsafe {
        close(fd);
    }
}

#[cfg(not(target_os = "linux"))]
fn restrict_sidecar(path: &ResolvedPath, expected: Identity) -> io::Result<()> {
    let mut files = FILES.get().unwrap().lock().map_err(|_| replaced())?;
    // The lock also prevents native open callbacks from returning a new FD
    // while this temporary descriptor is being closed. Existing native FDs
    // are borrowed instead, including SQLite's internally opened SHM handle.
    let borrowed = files
        .iter()
        .find(|(_, file)| !file.deferred && file.identity == Some(expected))
        .map(|(fd, _)| *fd);
    let name = name(path)?;
    let fd = if let Some(fd) = borrowed {
        fd
    } else {
        let fd = unsafe {
            libc::openat(
                path.storage.directory().as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "副ファイルの所有者の読み取り権限を手動で復元して再試行してください。",
            ));
        }
        fd
    };
    let result = stat_fd(fd).and_then(|metadata| {
        validate(&metadata)?;
        if identity(&metadata) != expected {
            return Err(replaced());
        }
        unsafe {
            if libc::fchmod(fd, 0o600) != 0 || libc::fsync(fd) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    });
    if borrowed.is_none() {
        let mut opened = TrackedFile::new(fd, Some(path.clone()));
        if files
            .values()
            .any(|file| !file.deferred && file.identity == opened.identity)
        {
            // A raced replacement can be another database that already has
            // POSIX locks. Keep this rejected FD until that inode is unused.
            opened.deferred = true;
            files.insert(fd, opened);
        } else {
            unsafe {
                libc::close(fd);
            }
        }
    }
    result
}

unsafe extern "C" fn fstat(fd: c_int, output: *mut libc::stat) -> c_int {
    let result = unsafe { (ORIGINALS.get().unwrap().fstat)(fd, output) };
    if result == 0 {
        CAPTURE.with(|slot| {
            if let Some(state) = slot.borrow().as_ref() {
                state.borrow_mut().descriptor = fd;
            }
        });
    }
    result
}

unsafe extern "C" fn fchmod(fd: c_int, mode: libc::mode_t) -> c_int {
    if CAPTURE.with(|slot| {
        slot.borrow().as_ref().is_some_and(|capture| {
            let capture = capture.borrow();
            capture.descriptor == fd && capture.rejected
        })
    }) {
        return fail(replaced());
    }
    unsafe { (ORIGINALS.get().unwrap().fchmod.unwrap())(fd, mode) }
}

unsafe fn path_stat(path: *const c_char, output: *mut libc::stat, follow: bool) -> c_int {
    unsafe {
        let resolved = match resolve_c(path) {
            Ok(Some(path)) => path,
            Ok(None) => {
                return if follow {
                    (ORIGINALS.get().unwrap().stat)(path, output)
                } else {
                    (ORIGINALS.get().unwrap().lstat.unwrap())(path, output)
                };
            }
            Err(error) => return fail(error),
        };
        let name = match name(&resolved) {
            Ok(name) => name,
            Err(error) => return fail(error),
        };
        let result = libc::fstatat(
            resolved.storage.directory().as_raw_fd(),
            name.as_ptr(),
            output,
            libc::AT_SYMLINK_NOFOLLOW,
        );
        if result == 0
            && let Err(error) = validate(&*output)
        {
            return fail(error);
        }
        result
    }
}

unsafe extern "C" fn stat(path: *const c_char, output: *mut libc::stat) -> c_int {
    unsafe { path_stat(path, output, true) }
}
unsafe extern "C" fn lstat(path: *const c_char, output: *mut libc::stat) -> c_int {
    unsafe { path_stat(path, output, false) }
}

unsafe extern "C" fn access(path: *const c_char, mode: c_int) -> c_int {
    unsafe {
        let resolved = match resolve_c(path) {
            Ok(Some(path)) => path,
            Ok(None) => return (ORIGINALS.get().unwrap().access)(path, mode),
            Err(error) => return fail(error),
        };
        if let Err(error) = stat_path(&resolved) {
            return fail(error);
        }
        let name = match name(&resolved) {
            Ok(name) => name,
            Err(error) => return fail(error),
        };
        libc::faccessat(
            resolved.storage.directory().as_raw_fd(),
            name.as_ptr(),
            mode,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    }
}

unsafe extern "C" fn unlink(path: *const c_char) -> c_int {
    unsafe {
        let resolved = match resolve_c(path) {
            Ok(Some(path)) => path,
            Ok(None) => return (ORIGINALS.get().unwrap().unlink)(path),
            Err(error) => return fail(error),
        };
        if let Err(error) = stat_path(&resolved) {
            return fail(error);
        }
        let name = match name(&resolved) {
            Ok(name) => name,
            Err(error) => return fail(error),
        };
        #[cfg(test)]
        native_checkpoint(NativeCheckpoint::Unlink);
        libc::unlinkat(resolved.storage.directory().as_raw_fd(), name.as_ptr(), 0)
    }
}

unsafe extern "C" fn open_directory(path: *const c_char, output: *mut c_int) -> c_int {
    unsafe {
        let resolved = match resolve_c(path) {
            Ok(Some(path)) => path,
            Ok(None) => return (ORIGINALS.get().unwrap().open_directory)(path, output),
            Err(error) => {
                fail(error);
                return ffi::SQLITE_CANTOPEN;
            }
        };
        // Directory descriptors have no SQLite database byte-range locks.
        let fd = libc::fcntl(
            resolved.storage.directory().as_raw_fd(),
            libc::F_DUPFD_CLOEXEC,
            3,
        );
        if fd < 0 {
            return ffi::SQLITE_CANTOPEN;
        }
        *output = fd;
        ffi::SQLITE_OK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::Arc;

    fn storage(root: &Path) -> Arc<crate::storage::PreparedAppData> {
        Arc::new(crate::storage::PreparedAppData::open(root.join("app-data")).unwrap())
    }

    use std::path::Path;

    #[test]
    fn lock_probe() {
        let Some(path) = std::env::var_os("PAIRRANK_VFS_LOCK_PROBE") else {
            return;
        };
        super::super::initialize().unwrap();
        let connection = rusqlite::Connection::open(path).unwrap();
        connection.busy_timeout(std::time::Duration::ZERO).unwrap();
        let error = connection.execute_batch("BEGIN IMMEDIATE").unwrap_err();
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy)
        );
    }

    pub(super) fn assert_locked(path: &Path) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "sqlite_vfs::unix::tests::lock_probe",
                "--nocapture",
            ])
            .env("PAIRRANK_VFS_LOCK_PROBE", path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn permission_failures_and_reused_descriptors_preserve_other_connections_locks() {
        for failure in [PermissionFailure::Chmod, PermissionFailure::Sync] {
            let root = tempfile::tempdir().unwrap();
            let storage = storage(root.path());
            let path = storage.path().join("pairrank.sqlite3");
            let first =
                super::super::open(storage.clone(), OsStr::new("pairrank.sqlite3")).unwrap();
            first
                .execute_batch(
                    "CREATE TABLE saved(value); BEGIN IMMEDIATE; INSERT INTO saved VALUES (1)",
                )
                .unwrap();
            // Closing this connection leaves a native reusable descriptor
            // while the first connection still owns the transaction lock.
            let second =
                super::super::open(storage.clone(), OsStr::new("pairrank.sqlite3")).unwrap();
            drop(second);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            PERMISSION_FAILURE.with(|slot| slot.set(Some(failure)));
            let result = super::super::open(storage, OsStr::new("pairrank.sqlite3"));
            PERMISSION_FAILURE.with(|slot| slot.set(None));
            assert!(result.is_err());
            assert_locked(&path);
            first.execute_batch("ROLLBACK").unwrap();
        }
    }

    #[test]
    fn renamed_app_data_keeps_journal_and_wal_in_the_opened_directory() {
        for wal in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let storage = storage(root.path());
            let original = root.path().join("original");
            let path = storage.path().to_owned();
            let connection = super::super::open(storage, OsStr::new("pairrank.sqlite3")).unwrap();
            if wal {
                connection
                    .pragma_update(None, "journal_mode", "WAL")
                    .unwrap();
            }
            connection
                .execute_batch("CREATE TABLE saved(value)")
                .unwrap();
            std::fs::rename(&path, &original).unwrap();
            std::fs::create_dir(&path).unwrap();
            connection
                .execute_batch("INSERT INTO saved VALUES (1)")
                .unwrap();
            drop(connection);
            assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
            assert!(original.join("pairrank.sqlite3").exists());
        }
    }

    #[test]
    fn sidecar_replacements_after_open_are_rejected_without_changing_targets() {
        for suffix in ["-journal", "-wal", "-shm"] {
            let root = tempfile::tempdir().unwrap();
            let storage = storage(root.path());
            let path = storage.path().to_owned();
            let connection = super::super::open(storage, OsStr::new("pairrank.sqlite3")).unwrap();
            let target = root.path().join("external");
            std::fs::write(&target, b"keep unchanged").unwrap();
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
            symlink(&target, path.join(format!("pairrank.sqlite3{suffix}"))).unwrap();
            let result = if suffix == "-journal" {
                connection.execute_batch("CREATE TABLE saved(value)")
            } else {
                connection.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE saved(value)")
            };
            assert!(result.is_err(), "{suffix}");
            assert_eq!(std::fs::read(&target).unwrap(), b"keep unchanged");
            assert_eq!(
                std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::Arc;

    #[cfg(target_os = "linux")]
    #[test]
    fn permission_restoration_sync_failure_rejects_open_without_releasing_writer_locks() {
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let root = tempfile::tempdir().unwrap();
            let storage = Arc::new(
                crate::storage::PreparedAppData::open(root.path().join("app-data")).unwrap(),
            );
            let path = storage.path().join("pairrank.sqlite3");
            let first =
                super::super::open(storage.clone(), OsStr::new("pairrank.sqlite3")).unwrap();
            if matches!(suffix, "-wal" | "-shm") {
                first.pragma_update(None, "journal_mode", "WAL").unwrap();
            }
            first
                .execute_batch(
                    "CREATE TABLE saved(value); BEGIN IMMEDIATE; INSERT INTO saved VALUES (1)",
                )
                .unwrap();
            let repaired = storage.path().join(format!("pairrank.sqlite3{suffix}"));
            std::fs::set_permissions(&repaired, std::fs::Permissions::from_mode(0o000)).unwrap();
            PERMISSION_FAILURE.with(|slot| slot.set(Some(PermissionFailure::Sync)));
            let result = super::super::open(storage, OsStr::new("pairrank.sqlite3"));
            PERMISSION_FAILURE.with(|slot| slot.set(None));
            assert!(result.is_err(), "sync failure accepted for {suffix:?}");
            super::tests::assert_locked(&path);
            first.execute_batch("ROLLBACK").unwrap();
        }
    }

    #[test]
    fn a_raced_sidecar_hardlink_does_not_cancel_another_databases_lock() {
        for suffix in ["-journal", "-shm"] {
            let root = tempfile::tempdir().unwrap();
            let storage = Arc::new(
                crate::storage::PreparedAppData::open(root.path().join("app-data")).unwrap(),
            );
            let locked_path = storage.path().join("locked.sqlite3");
            let journal = storage.path().join(format!("other.sqlite3{suffix}"));
            let first = super::super::open(storage.clone(), OsStr::new("locked.sqlite3")).unwrap();
            let second = super::super::open(storage, OsStr::new("other.sqlite3")).unwrap();
            first
                .execute_batch("CREATE TABLE saved(value); BEGIN IMMEDIATE")
                .unwrap();
            second.execute_batch("CREATE TABLE saved(value)").unwrap();
            let target = locked_path.clone();
            NATIVE_CHECKPOINT.with(|slot| {
                *slot.borrow_mut() = Some((
                    if suffix == "-journal" {
                        NativeCheckpoint::Open
                    } else {
                        NativeCheckpoint::ShmOpen
                    },
                    Box::new(move || {
                        std::fs::hard_link(&target, &journal).unwrap();
                    }),
                ))
            });
            let sql = if suffix == "-journal" {
                "INSERT INTO saved VALUES (1)"
            } else {
                "PRAGMA journal_mode=WAL; INSERT INTO saved VALUES (1)"
            };
            assert!(second.execute_batch(sql).is_err());
            assert!(NATIVE_CHECKPOINT.with(|slot| slot.borrow().is_none()));
            super::tests::assert_locked(&locked_path);
            first.execute_batch("ROLLBACK").unwrap();
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unreadable_database_reports_manual_permission_restoration() {
        for mode in [0o000, 0o200] {
            let root = tempfile::tempdir().unwrap();
            let storage = Arc::new(
                crate::storage::PreparedAppData::open(root.path().join("app-data")).unwrap(),
            );
            let path = storage.path().join("pairrank.sqlite3");
            std::fs::write(&path, b"saved data").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            let error = super::super::open(storage, OsStr::new("pairrank.sqlite3")).unwrap_err();
            assert!(error.contains("手動で復元"), "{error}");
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                mode
            );
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), b"saved data");
        }
    }

    #[test]
    fn directory_replacement_inside_native_open_and_commit_never_changes_the_destination() {
        for point in [NativeCheckpoint::Open, NativeCheckpoint::Unlink] {
            for link in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let storage = Arc::new(
                    crate::storage::PreparedAppData::open(root.path().join("app-data")).unwrap(),
                );
                let path = storage.path().to_owned();
                let replacement = root.path().join("replacement");
                let original = root.path().join("original");
                std::fs::create_dir(&replacement).unwrap();
                for name in ["pairrank.sqlite3", "image.png"] {
                    std::fs::write(replacement.join(name), b"saved external data").unwrap();
                    std::fs::set_permissions(
                        replacement.join(name),
                        std::fs::Permissions::from_mode(0o640),
                    )
                    .unwrap();
                }
                let connection = if point == NativeCheckpoint::Unlink {
                    let connection =
                        super::super::open(storage.clone(), OsStr::new("pairrank.sqlite3"))
                            .unwrap();
                    connection
                        .execute_batch("CREATE TABLE saved(value)")
                        .unwrap();
                    Some(connection)
                } else {
                    None
                };
                let callback_path = path.clone();
                let callback_replacement = replacement.clone();
                NATIVE_CHECKPOINT.with(|slot| {
                    *slot.borrow_mut() = Some((
                        point,
                        Box::new(move || {
                            std::fs::rename(&callback_path, &original).unwrap();
                            if link {
                                symlink(&callback_replacement, &callback_path).unwrap();
                            } else {
                                std::fs::rename(&callback_replacement, &callback_path).unwrap();
                            }
                        }),
                    ))
                });
                if let Some(connection) = connection {
                    connection
                        .execute_batch("INSERT INTO saved VALUES (1)")
                        .unwrap();
                } else {
                    assert!(super::super::open(storage, OsStr::new("pairrank.sqlite3")).is_err());
                }
                assert!(NATIVE_CHECKPOINT.with(|slot| slot.borrow().is_none()));
                let destination = if link { &replacement } else { &path };
                assert_eq!(std::fs::read_dir(destination).unwrap().count(), 2);
                for name in ["pairrank.sqlite3", "image.png"] {
                    assert_eq!(
                        std::fs::read(destination.join(name)).unwrap(),
                        b"saved external data"
                    );
                    assert_eq!(
                        std::fs::metadata(destination.join(name))
                            .unwrap()
                            .permissions()
                            .mode()
                            & 0o777,
                        0o640
                    );
                }
            }
        }
    }

    #[test]
    fn missing_native_callbacks_are_rejected_before_installation() {
        unsafe extern "C" fn missing(
            _vfs: *mut ffi::sqlite3_vfs,
            _name: *const c_char,
        ) -> ffi::sqlite3_syscall_ptr {
            None
        }
        super::super::initialize().unwrap();
        // SAFETY: copy the live native descriptor but never register the copy.
        unsafe {
            let native = super::super::INITIALIZED.get().unwrap().as_ref().unwrap().0;
            let before = ((*native).xGetSystemCall.unwrap())(native, c"open".as_ptr());
            let mut incomplete = *native;
            incomplete.xGetSystemCall = Some(missing);
            assert!(
                install(&mut incomplete)
                    .unwrap_err()
                    .contains("missing native syscall")
            );
            let after = ((*native).xGetSystemCall.unwrap())(native, c"open".as_ptr());
            assert_eq!(
                before.map(|callback| callback as usize),
                after.map(|callback| callback as usize)
            );
        }
    }

    #[test]
    fn correcting_open_shm_permissions_preserves_the_wal_writer_lock() {
        let root = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(crate::storage::PreparedAppData::open(root.path().join("app-data")).unwrap());
        let path = storage.path().join("pairrank.sqlite3");
        let first = super::super::open(storage.clone(), OsStr::new("pairrank.sqlite3")).unwrap();
        first.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE saved(value); BEGIN IMMEDIATE; INSERT INTO saved VALUES(1)").unwrap();
        std::fs::set_permissions(
            storage.path().join("pairrank.sqlite3-shm"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let second = super::super::open(storage, OsStr::new("pairrank.sqlite3")).unwrap();
        super::tests::assert_locked(&path);
        drop(second);
        super::tests::assert_locked(&path);
        first.execute_batch("ROLLBACK").unwrap();
    }
}
