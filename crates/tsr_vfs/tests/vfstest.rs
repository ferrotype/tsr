//! The test filesystem's contracts that are easy to get wrong.
use std::{collections::BTreeMap, sync::Arc};
use tsr_vfs::{
    iofs::{self, IoError},
    vfstest::{from_map, symlink, InputFile, TestFs},
};

fn build(files: &[(&str, InputFile)], case_sensitive: bool) -> TestFs {
    let map: BTreeMap<Vec<u8>, InputFile> = files
        .iter()
        .map(|(path, file)| (path.as_bytes().to_vec(), file.clone()))
        .collect();
    from_map(&map, case_sensitive)
}

#[test]
fn links_resolve_to_the_spelled_path_and_a_dangling_one_is_not_missing() {
    let fs = build(
        &[
            ("/Real/File.ts", InputFile::Text(b"x".to_vec())),
            ("/link", InputFile::File(symlink(b"/Real"))),
            ("/dangling", InputFile::File(symlink(b"/nowhere"))),
        ],
        false,
    );
    assert_eq!(fs.realpath(b"LINK/file.TS").unwrap(), b"Real/File.ts");
    let broken = fs.realpath(b"dangling").unwrap_err();
    assert!(broken.is_broken_symlink() && !broken.is_not_exist());
    assert_eq!(fs.realpath(b"absent").unwrap_err(), IoError::NotExist);
}

#[test]
fn a_held_entry_never_changes_under_its_holder() {
    let fs = build(&[("/a.ts", InputFile::Text(b"one".to_vec()))], true);
    let held = fs.get_file_info(b"/a.ts").unwrap();
    fs.write_file(b"a.ts", b"two", 0o666).unwrap();
    fs.chtimes(b"a.ts", iofs::Time::ZERO, iofs::Time::from_unix(1, 0))
        .unwrap();
    assert_eq!(&held.data[..], b"one");
    assert_eq!(&fs.get_file_info(b"/a.ts").unwrap().data[..], b"two");
    assert_eq!(fs.get_mod_time(b"/a.ts"), iofs::Time::from_unix(1, 0));
}

#[test]
fn writes_create_nothing_above_them_and_directories_list_in_name_order() {
    let fs = Arc::new(build(
        &[
            ("/d/b.ts", InputFile::Text(Vec::new())),
            ("/d/a.ts", InputFile::Text(Vec::new())),
        ],
        true,
    ));
    assert!(fs
        .write_file(b"missing/new.ts", b"", 0o666)
        .unwrap_err()
        .is_not_exist());
    let names: Vec<Vec<u8>> = iofs::read_dir(&*fs, b"d")
        .unwrap()
        .iter()
        .map(|entry| entry.name.clone())
        .collect();
    assert_eq!(names, [b"a.ts".to_vec(), b"b.ts".to_vec()]);
    fs.mkdir_all(b"d/sub/deep", 0o777).unwrap();
    assert!(iofs::stat(&*fs, b"d/sub/deep").unwrap().is_dir());
}
