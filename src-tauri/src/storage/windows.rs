//! Windows operations which never reconstruct an ambient path from a directory handle.

use cap_std::fs::Dir;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::mem::{offset_of, size_of, size_of_val};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::ptr::{null, null_mut};
use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
};
use windows_sys::Win32::Foundation::{
    ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE, OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError,
    UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_DISPOSITION_INFO, FILE_ID_BOTH_DIR_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, FileDispositionInfo,
    FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo, GetFileInformationByHandleEx,
    SYNCHRONIZE, SetFileInformationByHandle,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

const SHARING: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

/// Only a single entry is accepted, so an NT path, alternate stream, or parent
/// component cannot escape `RootDirectory` or change which object is opened.
fn entry_name(name: &OsStr) -> io::Result<Vec<u16>> {
    let wide: Vec<_> = name.encode_wide().collect();
    if wide.is_empty()
        || name == "."
        || name == ".."
        || wide.iter().any(|ch| matches!(*ch, 0 | 47 | 58 | 92))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "保存ファイル名が単一のエントリではありません。",
        ));
    }
    Ok(wide)
}

/// The options are NT create options, not Win32 FILE_FLAG_* values. The
/// returned handle always names the entry itself, including a reparse point.
fn open_entry(
    directory: &Dir,
    wide: &[u16],
    access: u32,
    share: u32,
    disposition: u32,
    attributes: u32,
    options: u32,
) -> io::Result<File> {
    let length = u16::try_from(size_of_val(wide))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "保存ファイル名が長すぎます。"))?;
    let mut name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: wide.as_ptr().cast_mut(),
    };
    let object = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: directory.as_raw_handle(),
        ObjectName: &mut name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: null_mut(),
        SecurityQualityOfService: null_mut(),
    };
    let mut handle = INVALID_HANDLE_VALUE;
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: the directory and name buffers outlive this synchronous call;
    // the output handle is uniquely owned and converted to File exactly once.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &object,
            &mut status,
            null(),
            attributes,
            share,
            disposition,
            options | FILE_OPEN_REPARSE_POINT,
            null(),
            0,
        )
    };
    if result < 0 {
        // SAFETY: the function maps any NTSTATUS to a Win32 error code.
        return Err(io::Error::from_raw_os_error(unsafe {
            RtlNtStatusToDosError(result) as i32
        }));
    }
    // SAFETY: a successful NtCreateFile returns an owned, valid handle.
    Ok(unsafe { File::from_raw_handle(handle) })
}

/// Open an ordinary entry relative to the retained directory and reject all
/// reparse points before the caller can read, write, or map their contents.
pub(crate) fn open_relative(
    directory: &Dir,
    name: &OsStr,
    access: u32,
    share: u32,
    disposition: u32,
    attributes: u32,
    options: u32,
) -> io::Result<File> {
    let wide = entry_name(name)?;
    let file = open_entry(
        directory,
        &wide,
        access | FILE_READ_ATTRIBUTES,
        share,
        disposition,
        attributes,
        options,
    )?;
    if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "保存先に reparse point は使用できません。",
        ));
    }
    Ok(file)
}

pub(super) fn create_private_directory_in(parent: &Dir, name: &OsStr) -> io::Result<Dir> {
    let file = open_relative(
        parent,
        name,
        FILE_LIST_DIRECTORY | FILE_TRAVERSE | SYNCHRONIZE,
        // Match the previous persistent image-directory handle: its lifetime
        // also prevents renaming or deleting this directory on Windows.
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        FILE_OPEN_IF,
        FILE_ATTRIBUTE_DIRECTORY,
        FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT,
    )?;
    Ok(Dir::from_std_file(file))
}

pub(super) fn directory_entries(directory: &Dir) -> io::Result<Vec<OsString>> {
    // A fresh file object has its own enumeration cursor. Duplicating the
    // original handle would share that cursor between concurrent scans.
    let scan = open_entry(
        directory,
        &[u16::from(b'.')],
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        SHARING,
        FILE_OPEN,
        FILE_ATTRIBUTE_NORMAL,
        FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT,
    )?;
    // u64 storage provides the alignment required by FILE_ID_BOTH_DIR_INFO.
    let mut buffer = vec![0_u64; 8192];
    let buffer_bytes = size_of_val(buffer.as_slice());
    let mut restart = true;
    let mut names = Vec::new();
    loop {
        // SAFETY: buffer is writable, aligned, and lives through the call.
        let result = unsafe {
            GetFileInformationByHandleEx(
                scan.as_raw_handle(),
                if restart {
                    FileIdBothDirectoryRestartInfo
                } else {
                    FileIdBothDirectoryInfo
                },
                buffer.as_mut_ptr().cast(),
                buffer_bytes as u32,
            )
        };
        if result == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                return Ok(names);
            }
            return Err(error);
        }
        restart = false;
        let mut offset = 0_usize;
        loop {
            const NAME_OFFSET: usize = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
            if offset + size_of::<FILE_ID_BOTH_DIR_INFO>() > buffer_bytes {
                return Err(invalid_directory_entry());
            }
            // SAFETY: offset is checked against the allocation above. Reading
            // unaligned also handles a malformed filesystem entry safely.
            let entry = unsafe {
                buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<FILE_ID_BOTH_DIR_INFO>()
                    .read_unaligned()
            };
            let name_bytes = entry.FileNameLength as usize;
            if !name_bytes.is_multiple_of(2) || offset + NAME_OFFSET + name_bytes > buffer_bytes {
                return Err(invalid_directory_entry());
            }
            // SAFETY: the file name length and range were checked; Windows
            // entries and NAME_OFFSET are aligned to at least two bytes.
            let wide = unsafe {
                std::slice::from_raw_parts(
                    buffer
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset + NAME_OFFSET)
                        .cast(),
                    name_bytes / 2,
                )
            };
            let name = OsString::from_wide(wide);
            if name != "." && name != ".." {
                entry_name(&name)?;
                names.push(name);
            }
            if entry.NextEntryOffset == 0 {
                break;
            }
            let next = entry.NextEntryOffset as usize;
            if next < NAME_OFFSET + name_bytes
                || !next.is_multiple_of(8)
                || offset + next >= buffer_bytes
            {
                return Err(invalid_directory_entry());
            }
            offset += next;
        }
    }
}

fn invalid_directory_entry() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "ディレクトリエントリが不正です。",
    )
}

pub(super) fn remove_file_in(directory: &Dir, name: &OsStr) -> io::Result<()> {
    let file = open_entry(
        directory,
        &entry_name(name)?,
        DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        SHARING,
        FILE_OPEN,
        FILE_ATTRIBUTE_NORMAL,
        FILE_SYNCHRONOUS_IO_NONALERT,
    )?;
    let attributes = file.metadata()?.file_attributes();
    // Deleting a directory reparse point deletes the link itself. An ordinary
    // directory must never be removed by this file-only operation.
    if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 && attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
    {
        return Err(io::Error::new(
            io::ErrorKind::IsADirectory,
            "画像ファイルの場所がディレクトリです。",
        ));
    }
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: this owned handle was opened with DELETE access; only that
    // handle's object is marked for deletion, without resolving its old path.
    let result = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    struct ReplacedParent {
        _temporary: tempfile::TempDir,
        original: PathBuf,
        replacement: PathBuf,
        directory: Dir,
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

    fn replaced_parent() -> ReplacedParent {
        let temporary = tempfile::tempdir().unwrap();
        let original = temporary.path().join("original");
        let replacement = temporary.path().join("replacement");
        fs::create_dir(&original).unwrap();
        fs::create_dir(&replacement).unwrap();
        fs::write(original.join("entry.png"), b"original image").unwrap();
        fs::write(replacement.join("entry.png"), b"replacement image").unwrap();
        fs::write(replacement.join("sentinel"), b"do not modify").unwrap();
        let alias = temporary.path().join("current");
        junction(&alias, &original);
        let directory = Dir::open_ambient_dir(&alias, cap_std::ambient_authority()).unwrap();
        // Opening the target pins its object, not this junction entry. Actually
        // switch the ambient name; do not count an OS rename refusal as proof.
        fs::remove_dir(&alias).unwrap();
        junction(&alias, &replacement);
        assert_eq!(
            fs::read(alias.join("entry.png")).unwrap(),
            b"replacement image"
        );
        ReplacedParent {
            _temporary: temporary,
            original,
            replacement,
            directory,
        }
    }

    fn snapshot(path: &Path) -> (u32, Vec<(OsString, u32, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(path)
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
    fn create_directory_does_not_follow_replaced_parent_junction() {
        let fixture = replaced_parent();
        let before = snapshot(&fixture.replacement);
        create_private_directory_in(&fixture.directory, OsStr::new("images")).unwrap();
        assert!(fixture.original.join("images").is_dir());
        assert_eq!(snapshot(&fixture.replacement), before);
    }

    #[test]
    fn enumeration_does_not_follow_replaced_parent_junction() {
        let fixture = replaced_parent();
        let before = snapshot(&fixture.replacement);
        for _ in 0..2 {
            assert_eq!(
                directory_entries(&fixture.directory).unwrap(),
                vec![OsString::from("entry.png")]
            );
        }
        assert_eq!(snapshot(&fixture.replacement), before);
    }

    #[test]
    fn deletion_does_not_follow_replaced_parent_junction() {
        let fixture = replaced_parent();
        let before = snapshot(&fixture.replacement);
        remove_file_in(&fixture.directory, OsStr::new("entry.png")).unwrap();
        assert!(!fixture.original.join("entry.png").exists());
        assert_eq!(snapshot(&fixture.replacement), before);
    }

    #[test]
    fn creating_directory_rejects_existing_junction() {
        let temporary = tempfile::tempdir().unwrap();
        let outside = temporary.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel"), b"do not modify").unwrap();
        junction(&temporary.path().join("images"), &outside);
        let directory =
            Dir::open_ambient_dir(temporary.path(), cap_std::ambient_authority()).unwrap();
        let before = snapshot(&outside);
        assert!(create_private_directory_in(&directory, OsStr::new("images")).is_err());
        assert_eq!(snapshot(&outside), before);
    }
}
