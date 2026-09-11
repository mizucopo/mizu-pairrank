use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

pub fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path)?;
    #[cfg(unix)]
    {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "managed directory is not a directory",
            ));
        }
        if metadata.permissions().mode() & 0o7777 != 0o700 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            File::open(path)?.sync_all()?;
        }
    }
    Ok(())
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
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "managed file is not a regular file",
        ));
    }
    if metadata.permissions().mode() & 0o7777 != 0o600 {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.sync_all()?;
    }
    Ok(())
}

#[cfg(unix)]
pub fn prepare_database_file(path: &Path) -> io::Result<()> {
    if path == Path::new(":memory:") {
        return Ok(());
    }
    match create_private_file(path) {
        Ok(file) => {
            file.sync_all()?;
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                File::open(parent)?.sync_all()?;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => restrict_existing_file(path)?,
        Err(error) => return Err(error),
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        match restrict_existing_file(Path::new(&sidecar)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn prepare_database_file(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
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
