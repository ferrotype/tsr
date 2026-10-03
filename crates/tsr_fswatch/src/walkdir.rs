#![forbid(unsafe_code)]
use crate::{watcher::join_suffix, Error};
use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{open, openat, statat, AtFlags, Dir, FileType, Mode, OFlags};
const FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::CLOEXEC)
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOCTTY)
    .union(OFlags::NONBLOCK)
    .union(OFlags::NOFOLLOW);
/// Descriptor-relative traversal: descendant symlinks are delivered as files,
/// never followed; deleted and unreadable descendant directories are skipped.
// port: tsc/internal/fswatch/walkdir_unix.go:walkDir
pub(crate) fn walk_dir(
    path: &[u8],
    recursive: bool,
    callback: &mut impl FnMut(&[u8], bool) -> Result<(), Error>,
) -> Result<(), Error> {
    let fd = open(path, FLAGS, Mode::empty())?;
    visit(fd, path, recursive, callback)
}
// port: tsc/internal/fswatch/walkdir_unix.go:iterateDir
fn visit(
    fd: OwnedFd,
    path: &[u8],
    recursive: bool,
    callback: &mut impl FnMut(&[u8], bool) -> Result<(), Error>,
) -> Result<(), Error> {
    callback(path, true)?;
    let mut reader = Dir::read_from(fd.as_fd())?;
    let mut entries = Vec::new();
    for entry in &mut reader {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if include_entry(entry.ino(), name) {
            entries.push((name.to_vec(), entry.file_type()));
        }
    }
    for (name, kind) in entries {
        let path = join_suffix(path, &name);
        let directory = if kind == FileType::Unknown {
            match statat(&fd, name.as_slice(), AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => FileType::from_raw_mode(stat.st_mode) == FileType::Directory,
                Err(_) => continue,
            }
        } else {
            kind == FileType::Directory
        };
        if !directory || !recursive {
            callback(&path, directory)?;
            continue;
        }
        match openat(&fd, name.as_slice(), FLAGS, Mode::empty()) {
            Ok(child) => visit(child, &path, recursive, callback)?,
            Err(
                rustix::io::Errno::ACCESS | rustix::io::Errno::NOTDIR | rustix::io::Errno::NOENT,
            ) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
// A zero inode denotes a deleted/unused directory record at the pin.
fn include_entry(inode: u64, name: &[u8]) -> bool {
    inode != 0 && name != b"." && name != b".."
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;
    #[test]
    fn deleted_directory_records_are_not_visited() {
        assert!(!include_entry(0, b"deleted"));
        assert!(!include_entry(1, b"."));
        assert!(!include_entry(1, b".."));
        assert!(include_entry(1, b"child"));
    }
    #[test]
    // source: tsc/internal/fswatch/walkdir_test.go:TestWalkDirDoesNotFollowSymlinkedDir
    fn recursion_does_not_follow_descendant_symlinks() {
        let temp = crate::test_support::TempDir::new();
        let dir = &temp.0;
        std::fs::create_dir_all(dir.join("a/deep")).unwrap();
        std::fs::write(dir.join("a/file"), b"x").unwrap();
        std::os::unix::fs::symlink("a", dir.join("link")).unwrap();
        let mut seen = Vec::new();
        walk_dir(dir.as_os_str().as_bytes(), true, &mut |path, is_dir| {
            seen.push((path.to_vec(), is_dir));
            Ok(())
        })
        .unwrap();
        assert_eq!(seen.len(), 5);
        assert!(seen
            .iter()
            .any(|(path, is_dir)| path.ends_with(b"/link") && !*is_dir));
    }
    #[test]
    // source: tsc/internal/fswatch/walkdir_test.go:TestWalkDirDoesNotFollowRootSymlinkedDir
    fn root_symlink_is_not_followed() {
        let temp = crate::test_support::TempDir::new();
        let target = crate::test_support::TempDir::new();
        std::os::unix::fs::symlink(&target.0, temp.0.join("link")).unwrap();
        assert!(walk_dir(
            temp.0.join("link").as_os_str().as_bytes(),
            true,
            &mut |_, _| panic!("root symlink must fail before callback")
        )
        .is_err());
    }
    #[test]
    fn shallow_walk_visits_root_and_direct_entries() {
        let temp = crate::test_support::TempDir::new();
        std::fs::create_dir_all(temp.0.join("sub/deep")).unwrap();
        std::fs::write(temp.0.join("file"), b"x").unwrap();
        let mut found = Vec::new();
        walk_dir(
            temp.0.as_os_str().as_bytes(),
            false,
            &mut |path, directory| {
                found.push((path.to_vec(), directory));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(found.len(), 3);
        assert_eq!(found.iter().filter(|(_, dir)| *dir).count(), 2);
        assert!(!found.iter().any(|(path, _)| path.ends_with(b"/deep")));
    }
    #[test]
    // source: tsc/internal/fswatch/walkdir_test.go:TestWalkDirMissingDir
    fn missing_root_fails_before_callback() {
        let temp = crate::test_support::TempDir::new();
        assert!(walk_dir(
            temp.0.join("missing").as_os_str().as_bytes(),
            true,
            &mut |_, _| panic!("missing root must fail before callback")
        )
        .is_err());
    }
    #[test]
    // source: tsc/internal/fswatch/walkdir_test.go:TestWalkDirNotADir
    fn non_directory_root_fails_before_callback() {
        let temp = crate::test_support::TempDir::new();
        std::fs::write(temp.0.join("file"), b"x").unwrap();
        assert!(walk_dir(
            temp.0.join("file").as_os_str().as_bytes(),
            true,
            &mut |_, _| { panic!("invalid root must not call visitor") }
        )
        .is_err());
    }
    #[test]
    // source: tsc/internal/fswatch/walkdir_test.go:TestWalkDirEntries
    fn recursive_walk_reports_all_files_and_directories() {
        let temp = crate::test_support::TempDir::new();
        std::fs::write(temp.0.join("a.txt"), b"a").unwrap();
        std::fs::create_dir(temp.0.join("sub")).unwrap();
        std::fs::write(temp.0.join("sub/b.txt"), b"b").unwrap();
        let mut found = std::collections::BTreeMap::new();
        walk_dir(
            temp.0.as_os_str().as_bytes(),
            true,
            &mut |path, directory| {
                found.insert(path.to_vec(), directory);
                Ok(())
            },
        )
        .unwrap();
        for (suffix, directory) in [
            ("", true),
            ("a.txt", false),
            ("sub", true),
            ("sub/b.txt", false),
        ] {
            let path = if suffix.is_empty() {
                temp.0.clone()
            } else {
                temp.0.join(suffix)
            };
            assert_eq!(found.get(path.as_os_str().as_bytes()), Some(&directory));
        }
        assert_eq!(found.len(), 4);
    }
    #[test]
    // source: tsc/internal/fswatch/walkdir_test.go:TestWalkDirCallback
    fn callback_distinguishes_root_subdirectories_and_files() {
        let temp = crate::test_support::TempDir::new();
        std::fs::create_dir(temp.0.join("sub")).unwrap();
        std::fs::write(temp.0.join("sub/f.txt"), b"f").unwrap();
        let (mut directories, mut files) = (Vec::new(), Vec::new());
        walk_dir(
            temp.0.as_os_str().as_bytes(),
            true,
            &mut |path, directory| {
                if directory {
                    directories.push(path.to_vec());
                } else {
                    files.push(path.to_vec());
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(directories.len(), 2);
        assert_eq!(
            files,
            [temp.0.join("sub/f.txt").as_os_str().as_bytes().to_vec()]
        );
    }
    #[test]
    // source: tsc/internal/fswatch/walkdir_test.go:TestWalkDirCallbackError
    fn callback_error_stops_before_visiting_descendants() {
        let temp = crate::test_support::TempDir::new();
        std::fs::create_dir(temp.0.join("sub")).unwrap();
        let mut visits = 0;
        let result = walk_dir(temp.0.as_os_str().as_bytes(), true, &mut |_, _| {
            visits += 1;
            Err(Error::Message("stop".into()))
        });
        assert_eq!(result, Err(Error::Message("stop".into())));
        assert_eq!(visits, 1);
    }
    #[test]
    // source: tsc/internal/fswatch/walkdir_test.go:TestWalkDirIgnoresUnreadableSubdir
    fn unreadable_descendants_are_skipped_when_permissions_are_enforced() {
        use std::os::unix::fs::PermissionsExt;
        let temp = crate::test_support::TempDir::new();
        let denied = temp.0.join("denied");
        std::fs::create_dir(&denied).unwrap();
        std::fs::write(denied.join("hidden"), b"x").unwrap();
        std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o0)).unwrap();
        let enforced = std::fs::read_dir(&denied).is_err();
        let mut found = Vec::new();
        let result = walk_dir(temp.0.as_os_str().as_bytes(), true, &mut |path, _| {
            found.push(path.to_vec());
            Ok(())
        });
        std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o700)).unwrap();
        result.unwrap();
        if enforced {
            assert_eq!(found, [temp.0.as_os_str().as_bytes().to_vec()]);
        }
    }
}
