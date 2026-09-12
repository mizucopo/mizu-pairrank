use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

#[derive(Default)]
pub struct PreparedDatabaseFile {
    // Extra open/close calls for this inode can release another SQLite connection's
    // process-wide POSIX locks, so keep identity values instead of another descriptor.
    identity: Option<(u64, u64)>,
}

impl PreparedDatabaseFile {
    pub fn has_identity(&self) -> bool {
        self.identity.is_some()
    }

    pub fn verify_path(&self, path: &Path) -> io::Result<()> {
        #[cfg(unix)]
        if let Some(expected) = self.identity {
            let metadata = fs::symlink_metadata(path)?;
            if !metadata.is_file() || (metadata.dev(), metadata.ino()) != expected {
                return Err(io::Error::other("prepared database file was replaced"));
            }
        }
        #[cfg(not(unix))]
        let _ = path;
        Ok(())
    }
}

pub struct PreparedAppData {
    path: std::path::PathBuf,
    identity: same_file::Handle,
}

impl PreparedAppData {
    pub fn open(path: std::path::PathBuf) -> io::Result<Self> {
        #[cfg(unix)]
        let expected = create_private_directory_with(
            &path,
            &mut |directory| File::open(directory)?.sync_all(),
            |_| {},
        )?;
        #[cfg(not(unix))]
        create_private_directory(&path)?;
        let directory = cap_std::fs::Dir::open_ambient_dir(&path, cap_std::ambient_authority())?
            .into_std_file();
        #[cfg(unix)]
        verify_identity(&directory.metadata()?, expected)?;
        let prepared = Self {
            path,
            identity: same_file::Handle::from_file(directory)?,
        };
        prepared.verify()?;
        Ok(prepared)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn verify(&self) -> io::Result<()> {
        #[cfg(unix)]
        if !fs::symlink_metadata(&self.path)?.is_dir() {
            return Err(io::Error::other("app-data directory was replaced"));
        }
        if same_file::Handle::from_path(&self.path)? != self.identity {
            return Err(io::Error::other("app-data directory was replaced"));
        }
        Ok(())
    }
}

pub fn create_private_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    return create_private_directory_with_sync(path, &mut |directory| {
        File::open(directory)?.sync_all()
    });
    #[cfg(not(unix))]
    fs::create_dir_all(path)
}

#[cfg(unix)]
fn create_private_directory_with_sync(
    path: &Path,
    sync: &mut impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    create_private_directory_with(path, sync, |_| {}).map(|_| ())
}

#[cfg(unix)]
fn create_private_directory_with(
    path: &Path,
    sync: &mut impl FnMut(&Path) -> io::Result<()>,
    mut checkpoint: impl FnMut(bool),
) -> io::Result<(u64, u64)> {
    create_directory_tree(path, sync)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "managed directory is not a directory",
        ));
    }
    checkpoint(false);
    let identity = (metadata.dev(), metadata.ino());
    if metadata.permissions().mode() & 0o7777 != 0o700 {
        use rustix::fs::{CWD, Mode, OFlags, openat};
        let opened = openat(
            CWD,
            path,
            OFlags::RDONLY
                | OFlags::DIRECTORY
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK
                | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(io::Error::from);
        let directory = match opened {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                checkpoint(true);
                let parent = path
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                let parent = cap_std::fs::Dir::from_std_file(File::open(parent)?);
                let name = path
                    .file_name()
                    .ok_or_else(|| io::Error::other("missing managed directory name"))?;
                restore_unreadable_entry(&parent, name, identity, 0o700, false)?
            }
            Err(error) => return Err(error),
        };
        verify_identity(&directory.metadata()?, identity)?;
        directory.set_permissions(fs::Permissions::from_mode(0o700))?;
        directory.sync_all()?;
    }
    let current = fs::symlink_metadata(path)?;
    if !current.is_dir() {
        return Err(io::Error::other("managed directory was replaced"));
    }
    verify_identity(&current, identity)?;
    Ok(identity)
}

#[cfg(unix)]
fn create_directory_tree(
    path: &Path,
    sync: &mut impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    let parent = path.parent().unwrap_or(path);
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    if !path.try_exists()? {
        create_directory_tree(parent, sync)?;
    }
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => {}
        Err(error) => return Err(error),
    }
    // Also synchronize a directory left behind by an earlier failed attempt.
    sync(parent)
}

pub fn create_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(path)?;
    #[cfg(unix)]
    if let Err(error) = file.set_permissions(fs::Permissions::from_mode(0o600)) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(file)
}

#[cfg(unix)]
pub fn restrict_existing_file(path: &Path) -> io::Result<()> {
    restrict_existing_file_with(path, |_| {}).map(|_| ())
}

#[cfg(unix)]
fn restrict_existing_file_with(
    path: &Path,
    checkpoint: impl FnMut(bool),
) -> io::Result<(u64, u64)> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let directory = cap_std::fs::Dir::from_std_file(File::open(parent)?);
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("missing managed filename"))?;
    restrict_existing_file_in_with(&directory, name, false, checkpoint)
}

#[cfg(unix)]
pub(crate) fn restrict_existing_file_in(
    directory: &cap_std::fs::Dir,
    name: &std::ffi::OsStr,
) -> io::Result<()> {
    restrict_existing_file_in_with(directory, name, true, |_| {}).map(|_| ())
}

#[cfg(unix)]
fn restrict_existing_file_in_with(
    directory: &cap_std::fs::Dir,
    name: &std::ffi::OsStr,
    single_link: bool,
    mut checkpoint: impl FnMut(bool),
) -> io::Result<(u64, u64)> {
    use cap_std::fs::MetadataExt as _;
    use cap_std::fs::PermissionsExt as _;
    let metadata = directory.symlink_metadata(name)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "managed file is not a regular file",
        ));
    }
    verify_single_link(metadata.nlink(), single_link)?;
    checkpoint(false);
    let identity = (metadata.dev(), metadata.ino());
    if metadata.permissions().mode() & 0o7777 != 0o600 {
        let file = match open_managed_file(directory, name) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                checkpoint(true);
                restore_unreadable_entry(directory, name, identity, 0o600, single_link)?
            }
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        verify_identity(&metadata, identity)?;
        verify_single_link(metadata.nlink(), single_link)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.sync_all()?;
    }
    Ok(identity)
}

#[cfg(unix)]
fn verify_single_link(link_count: u64, required: bool) -> io::Result<()> {
    if required && link_count != 1 {
        return Err(io::Error::other(
            "managed image must have exactly one hard link",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn verify_identity(metadata: &fs::Metadata, expected: (u64, u64)) -> io::Result<()> {
    if (metadata.dev(), metadata.ino()) != expected {
        return Err(io::Error::other("managed storage was replaced"));
    }
    Ok(())
}

pub fn managed_open_options() -> cap_std::fs::OpenOptions {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No).nonblock(true);
    options
}

pub fn open_managed_file(directory: &cap_std::fs::Dir, name: &std::ffi::OsStr) -> io::Result<File> {
    let file = directory
        .open_with(name, &managed_open_options())?
        .into_std();
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("managed file is not a regular file"));
    }
    Ok(file)
}

#[cfg(target_os = "linux")]
fn restore_unreadable_entry(
    directory: &cap_std::fs::Dir,
    name: &std::ffi::OsStr,
    expected: (u64, u64),
    mode: u32,
    single_link: bool,
) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, openat};
    use std::os::fd::AsRawFd;
    // O_PATH can pin an owned 0000 file without following a replacement symlink.
    let pinned = File::from(openat(
        directory,
        name,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let metadata = pinned.metadata()?;
    verify_identity(&metadata, expected)?;
    verify_single_link(metadata.nlink(), single_link)?;
    // chmod through this live descriptor targets the pinned inode, even after rename.
    let descriptor = format!("/proc/self/fd/{}", pinned.as_raw_fd());
    fs::set_permissions(&descriptor, fs::Permissions::from_mode(mode))?;
    File::open(descriptor)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn restore_unreadable_entry(
    _directory: &cap_std::fs::Dir,
    name: &std::ffi::OsStr,
    _expected: (u64, u64),
    mode: u32,
    _single_link: bool,
) -> io::Result<File> {
    // Without a descriptor, pathname chmod could change a replacement inode.
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "{name:?} の権限を安全に復元できません。所有者の読み取り権限（推奨 {mode:o}）を手動で復元して再試行してください。"
        ),
    ))
}

#[cfg(unix)]
pub fn prepare_database_file(path: &Path) -> io::Result<PreparedDatabaseFile> {
    if path == Path::new(":memory:") {
        return Ok(PreparedDatabaseFile::default());
    }
    let identity = match create_private_file(path) {
        Ok(file) => {
            file.sync_all()?;
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                File::open(parent)?.sync_all()?;
            }
            let metadata = file.metadata()?;
            (metadata.dev(), metadata.ino())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            restrict_existing_file_with(path, |_| {})?
        }
        Err(error) => return Err(error),
    };
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        match restrict_existing_file(Path::new(&sidecar)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(PreparedDatabaseFile {
        identity: Some(identity),
    })
}

#[cfg(not(unix))]
pub fn prepare_database_file(_path: &Path) -> io::Result<PreparedDatabaseFile> {
    Ok(PreparedDatabaseFile::default())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn unreadable_file_requires_manual_permission_recovery_without_modification() {
        let directory = tempfile::tempdir().unwrap();
        let managed = directory.path().join("pairrank.sqlite3");
        fs::write(&managed, b"saved ranking").unwrap();
        for original_mode in [0o200, 0o000] {
            fs::set_permissions(&managed, fs::Permissions::from_mode(original_mode)).unwrap();
            let result = restrict_existing_file(&managed);
            let unchanged_mode = mode(&managed);
            fs::set_permissions(&managed, fs::Permissions::from_mode(0o600)).unwrap();
            let error = result.unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert!(error.to_string().contains("所有者の読み取り権限"));
            assert_eq!(unchanged_mode, original_mode);
            assert_eq!(fs::read(&managed).unwrap(), b"saved ranking");
        }
    }

    #[test]
    fn directory_permission_repair_preserves_replacements_after_metadata_validation() {
        for symlink in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let managed = root.path().join("app-data");
            let previous = root.path().join("previous");
            let replacement = root.path().join("replacement");
            fs::create_dir(&managed).unwrap();
            fs::create_dir(&replacement).unwrap();
            fs::write(managed.join("original"), b"original").unwrap();
            fs::write(replacement.join("replacement"), b"replacement").unwrap();
            fs::set_permissions(&managed, fs::Permissions::from_mode(0o755)).unwrap();
            fs::set_permissions(&replacement, fs::Permissions::from_mode(0o750)).unwrap();
            let result = create_private_directory_with(
                &managed,
                &mut |directory| File::open(directory)?.sync_all(),
                |_| {
                    fs::rename(&managed, &previous).unwrap();
                    if symlink {
                        std::os::unix::fs::symlink(&replacement, &managed).unwrap();
                    } else {
                        fs::rename(&replacement, &managed).unwrap();
                    }
                },
            );
            assert!(result.is_err());
            assert_eq!(mode(&managed), 0o750);
            assert_eq!(mode(&previous), 0o755);
            assert_eq!(
                fs::read(managed.join("replacement")).unwrap(),
                b"replacement"
            );
            assert_eq!(fs::read(previous.join("original")).unwrap(), b"original");
        }
    }

    #[test]
    fn directory_permission_repair_preserves_data_and_rejects_unsafe_recovery() {
        let root = tempfile::tempdir().unwrap();
        let managed = root.path().join("app-data");
        fs::create_dir(&managed).unwrap();
        fs::write(managed.join("saved"), b"saved").unwrap();
        for original_mode in [0o755, 0o500, 0o300, 0o000] {
            fs::set_permissions(&managed, fs::Permissions::from_mode(original_mode)).unwrap();
            let result = create_private_directory(&managed);
            let repaired_mode = mode(&managed);
            fs::set_permissions(&managed, fs::Permissions::from_mode(0o700)).unwrap();
            if cfg!(target_os = "linux") || original_mode & 0o400 != 0 {
                assert!(
                    result.is_ok(),
                    "could not repair {original_mode:o}: {result:?}"
                );
                assert_eq!(repaired_mode, 0o700);
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                assert!(error.to_string().contains("所有者の読み取り権限"));
                assert_eq!(repaired_mode, original_mode);
            }
            assert_eq!(fs::read(managed.join("saved")).unwrap(), b"saved");
        }
    }

    #[test]
    fn image_permission_repair_rejects_a_hardlink_created_after_metadata_validation() {
        for original_mode in [0o644, 0o000] {
            let directory = tempfile::tempdir().unwrap();
            let name = std::ffi::OsStr::new("managed.png");
            let managed = directory.path().join(name);
            let external = directory.path().join("external.png");
            fs::write(&managed, b"unchanged image").unwrap();
            fs::set_permissions(&managed, fs::Permissions::from_mode(original_mode)).unwrap();
            let pinned = cap_std::fs::Dir::from_std_file(File::open(directory.path()).unwrap());
            let result = restrict_existing_file_in_with(&pinned, name, true, |repairing| {
                if !repairing {
                    fs::hard_link(&managed, &external).unwrap();
                }
            });
            let external_mode = mode(&external);
            fs::set_permissions(&managed, fs::Permissions::from_mode(0o600)).unwrap();
            assert!(result.is_err());
            assert_eq!(external_mode, original_mode);
            assert_eq!(fs::read(external).unwrap(), b"unchanged image");
        }
    }

    #[test]
    fn permission_repair_rejects_an_ordinary_replacement_before_chmod() {
        for (original_mode, replace_in_repair) in [(0o644, false), (0o000, false), (0o000, true)] {
            let directory = tempfile::tempdir().unwrap();
            let managed = directory.path().join("pairrank.sqlite3");
            let previous = directory.path().join("previous.sqlite3");
            fs::write(&managed, b"original").unwrap();
            fs::set_permissions(&managed, fs::Permissions::from_mode(original_mode)).unwrap();
            let result = restrict_existing_file_with(&managed, |repairing| {
                if repairing != replace_in_repair {
                    return;
                }
                fs::rename(&managed, &previous).unwrap();
                fs::write(&managed, b"replacement").unwrap();
                fs::set_permissions(&managed, fs::Permissions::from_mode(0o640)).unwrap();
            });
            assert!(result.is_err());
            assert_eq!(mode(&managed), 0o640);
            assert_eq!(mode(&previous), original_mode);
            assert_eq!(fs::read(managed).unwrap(), b"replacement");
            fs::set_permissions(&previous, fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(fs::read(previous).unwrap(), b"original");
        }
    }

    #[test]
    fn permission_repair_rejects_symlink_replacement_without_changing_the_target() {
        for (original_mode, replace_in_repair) in [(0o644, false), (0o000, false), (0o000, true)] {
            let directory = tempfile::tempdir().unwrap();
            let managed = directory.path().join("pairrank.sqlite3");
            let external = directory.path().join("external.sqlite3");
            fs::write(&managed, b"managed").unwrap();
            fs::set_permissions(&managed, fs::Permissions::from_mode(original_mode)).unwrap();
            fs::write(&external, b"external").unwrap();
            fs::set_permissions(&external, fs::Permissions::from_mode(0o644)).unwrap();
            let mut replaced = false;
            let result = restrict_existing_file_with(&managed, |repairing| {
                if repairing == replace_in_repair {
                    fs::rename(&managed, directory.path().join("previous.sqlite3")).unwrap();
                    std::os::unix::fs::symlink(&external, &managed).unwrap();
                    replaced = true;
                }
            });
            assert!(replaced);
            assert!(result.is_err());
            assert_eq!(mode(&external), 0o644);
            assert_eq!(fs::read(&external).unwrap(), b"external");
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn new_directory_entries_are_synced_before_creating_the_next_child() {
        let parent = tempfile::tempdir().unwrap();
        let app_data = parent.path().join("app-data");
        let images = app_data.join("images");
        let mut synced = Vec::new();
        create_private_directory_with_sync(&images, &mut |directory| {
            if directory == parent.path() {
                assert!(app_data.is_dir());
                assert!(!images.exists());
            }
            File::open(directory)?.sync_all()?;
            synced.push(directory.to_path_buf());
            Ok(())
        })
        .unwrap();
        assert!(synced.ends_with(&[parent.path().to_path_buf(), app_data.clone()]));
        assert_eq!(mode(&app_data), 0o700);
        assert_eq!(mode(&images), 0o700);
    }

    #[test]
    fn failed_ancestor_sync_stops_creation_and_is_retried() {
        let parent = tempfile::tempdir().unwrap();
        let app_data = parent.path().join("app-data");
        let images = app_data.join("images");
        let error = create_private_directory_with_sync(&images, &mut |directory| {
            if directory == parent.path() {
                return Err(io::Error::other("parent sync failed"));
            }
            File::open(directory)?.sync_all()
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "parent sync failed");
        assert!(app_data.is_dir());
        assert!(!images.exists());

        let mut synced = Vec::new();
        create_private_directory_with_sync(&images, &mut |directory| {
            File::open(directory)?.sync_all()?;
            synced.push(directory.to_path_buf());
            Ok(())
        })
        .unwrap();
        assert_eq!(synced, [parent.path(), app_data.as_path()]);
        assert!(images.is_dir());
    }

    #[test]
    fn database_creation_and_reopening_keep_private_modes_and_saved_rankings() {
        let parent = tempfile::tempdir().unwrap();
        let directory = parent.path().join("app-data");
        create_private_directory(&directory).unwrap();
        assert_eq!(mode(&directory), 0o700);
        let path = directory.join("pairrank.sqlite3");
        {
            let mut database = crate::database::Database::open(&path).unwrap();
            database.create_list("Private ranking".into()).unwrap();
            assert_eq!(mode(&path), 0o600);
        }
        let before = fs::read(&path).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        create_private_directory(&directory).unwrap();
        let database = crate::database::Database::open(&path).unwrap();
        assert_eq!(mode(&directory), 0o700);
        assert_eq!(mode(&path), 0o600);
        assert_eq!(
            database.list_summaries().unwrap()[0].name,
            "Private ranking"
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn reopening_a_read_only_database_restores_write_access_without_changing_saved_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pairrank.sqlite3");
        {
            let mut database = crate::database::Database::open(&path).unwrap();
            database.create_list("Restored ranking".into()).unwrap();
        }
        let before = fs::read(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();

        let mut database = crate::database::Database::open(&path).unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(
            database.list_summaries().unwrap()[0].name,
            "Restored ranking"
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        database.create_list("New ranking".into()).unwrap();
        assert_eq!(database.list_summaries().unwrap().len(), 2);
    }

    #[test]
    fn image_storage_permission_handling_preserves_managed_and_source_images() {
        let directory = tempfile::tempdir().unwrap();
        let images = directory.path().join("images");
        create_private_directory(&images).unwrap();
        let managed = images.join(format!("{}.png", uuid::Uuid::new_v4()));
        let source = directory.path().join("source.png");
        fs::write(&managed, b"managed image").unwrap();
        fs::write(&source, b"source image").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o400)).unwrap();

        for restored_mode in [0o400, 0o444, 0o200, 0o000] {
            fs::set_permissions(&managed, fs::Permissions::from_mode(restored_mode)).unwrap();
            let result = crate::images::ImageService::new(directory.path().to_owned());
            let repaired_mode = mode(&managed);
            fs::set_permissions(&managed, fs::Permissions::from_mode(0o600)).unwrap();
            if cfg!(target_os = "linux") || restored_mode & 0o400 != 0 {
                assert!(result.is_ok());
                assert_eq!(repaired_mode, 0o600);
            } else {
                assert!(result.err().unwrap().contains("所有者の読み取り権限"));
                assert_eq!(repaired_mode, restored_mode);
            }
            assert_eq!(fs::read(&managed).unwrap(), b"managed image");
            assert_eq!(mode(&source), 0o400);
            assert_eq!(fs::read(&source).unwrap(), b"source image");
        }
    }

    #[test]
    fn existing_database_sidecars_are_restricted_without_modifying_their_contents() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("pairrank.sqlite3");
        prepare_database_file(&database).unwrap();
        let sidecars: Vec<_> = ["-wal", "-shm", "-journal"]
            .into_iter()
            .map(|suffix| {
                let path = directory.path().join(format!("pairrank.sqlite3{suffix}"));
                fs::write(&path, b"sidecar content").unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
                path
            })
            .collect();
        prepare_database_file(&database).unwrap();
        for path in sidecars {
            assert_eq!(mode(&path), 0o600);
            assert_eq!(fs::read(path).unwrap(), b"sidecar content");
        }
    }

    #[test]
    fn a_database_symlink_is_rejected_without_changing_its_target() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.png");
        fs::write(&source, b"unchanged source").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).unwrap();
        let database = directory.path().join("pairrank.sqlite3");
        std::os::unix::fs::symlink(&source, &database).unwrap();
        assert!(prepare_database_file(&database).is_err());
        assert_eq!(mode(&source), 0o644);
        assert_eq!(fs::read(source).unwrap(), b"unchanged source");
    }
}
