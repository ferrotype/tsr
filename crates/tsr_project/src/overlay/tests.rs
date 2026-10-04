use super::*;
use tsr_lsproto::{
    Position, Range, TextDocumentContentChangePartial,
    TextDocumentContentChangePartialOrWholeDocument as Edit,
    TextDocumentContentChangeWholeDocument,
};

fn setup(encoding: PositionEncoding) -> OverlayFs {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", false);
    fs.insert_loaded(b"/test1.ts", &b"// existing content"[..]);
    fs.insert_loaded(b"/test2.ts", &b"// existing content"[..]);
    fs.insert_loaded(b"/script", &b"// extensionless content"[..]);
    OverlayFs::new(
        Arc::new(fs.finish()),
        JsString::from_bytes(&b"/"[..]),
        encoding,
    )
}
fn event(kind: K) -> FileChange {
    FileChange::new(kind, DocumentUri::from("file:///test1.ts"))
}
fn open(text: &[u8]) -> FileChange {
    FileChange {
        content: JsString::from_bytes(text),
        version: 1,
        language: LanguageKind("typescript".into()),
        ..event(K::Open)
    }
}
fn whole(text: &str, version: i32) -> FileChange {
    FileChange {
        version,
        changes: vec![Edit {
            whole_document: Some(Box::new(TextDocumentContentChangeWholeDocument {
                text: text.into(),
            })),
            ..Default::default()
        }],
        ..event(K::Change)
    }
}
// source: tsc/internal/project/overlayfs_test.go:TestProcessChanges
#[test]
fn watch_reduction_and_save_without_overlay() {
    for (events, changed, empty) in [
        (vec![K::WatchCreate, K::WatchDelete], false, true),
        (vec![K::WatchDelete, K::WatchCreate], true, false),
        (
            vec![K::WatchChange, K::WatchChange, K::WatchChange],
            true,
            false,
        ),
        (vec![K::Save], true, false),
    ] {
        let result = setup(PositionEncoding::Utf16)
            .process_changes(&events.into_iter().map(event).collect::<Vec<_>>())
            .unwrap();
        assert_eq!(result.is_empty(), empty);
        assert_eq!(result.changed.contains(&event(K::Change).uri), changed);
        assert!(result.created.is_empty() && result.deleted.is_empty());
    }
}
// source: tsc/internal/project/overlayfs_test.go:TestProcessChanges
#[test]
fn saves_and_watches_replace_handles_without_mutating_old_snapshots() {
    let mut fs = setup(PositionEncoding::Utf16);
    fs.process_changes(&[open(b"const x = 1;")]).unwrap();
    let first = fs.overlays().clone();
    let file = fs.get_file(b"/test1.ts").unwrap().unwrap();
    assert!(!file.matches_disk_text());
    assert!(fs.process_changes(&[event(K::Save)]).unwrap().is_empty());
    assert!(fs
        .get_file(b"/test1.ts")
        .unwrap()
        .unwrap()
        .matches_disk_text());
    assert!(!file.matches_disk_text());
    fs.process_changes(&[event(K::WatchChange)]).unwrap();
    assert!(!fs
        .get_file(b"/test1.ts")
        .unwrap()
        .unwrap()
        .matches_disk_text());
    assert!(Arc::ptr_eq(
        first.get(b"/test1.ts".as_slice()).unwrap(),
        &file
    ));
    fs.process_changes(&[event(K::Close)]).unwrap();
    assert!(fs.process_changes(&[event(K::Close)]).unwrap().is_empty());
    assert!(fs
        .process_changes(&[whole("ignored", 2)])
        .unwrap()
        .is_empty());
    assert_eq!(file.content().as_bytes(), b"const x = 1;");
}
// source: tsc/internal/project/overlayfs_test.go:TestProcessChanges
#[test]
fn close_open_batch_reports_change_or_reopen_and_unknown_languages_use_extensions() {
    let mut fs = setup(PositionEncoding::Utf16);
    fs.process_changes(&[open(b"one")]).unwrap();
    let result = fs
        .process_changes(&[event(K::Close), open(b"two")])
        .unwrap();
    assert!(result.opened.is_none() && result.reopened.is_none());
    assert!(result.changed.contains(&event(K::Open).uri));
    let result = fs
        .process_changes(&[event(K::Close), open(b"two")])
        .unwrap();
    assert_eq!(result.reopened, Some(event(K::Open).uri));
    for (uri, language, expected) in [
        ("file:///test.mts", "mts", ScriptKind::TS),
        ("file:///script", "plaintext", ScriptKind::UNKNOWN),
    ] {
        let mut change = open(b"text");
        change.uri = DocumentUri::from(uri);
        change.language = LanguageKind(language.into());
        fs.process_changes(&[change]).unwrap();
        assert_eq!(
            fs.get_file(DocumentUri::from(uri).file_name().as_bytes())
                .unwrap()
                .unwrap()
                .kind(),
            expected
        );
    }
    assert_eq!(
        setup(PositionEncoding::Utf16)
            .get_file(b"/script")
            .unwrap()
            .unwrap()
            .kind(),
        ScriptKind::UNKNOWN
    );
}
// source: tsc/internal/project/overlayfs_test.go:TestProcessChanges
#[test]
fn invalid_batch_order_panics_before_publication() {
    for changes in [
        vec![open(b"a"), {
            let mut c = open(b"b");
            c.uri = DocumentUri::from("file:///test2.ts");
            c
        }],
        vec![open(b"a"), whole("b", 2)],
        vec![event(K::Close), whole("b", 2)],
    ] {
        let mut fs = setup(PositionEncoding::Utf16);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || fs.process_changes(&changes)
        ))
        .is_err());
        assert!(fs.overlays().is_empty());
    }
}
#[test]
fn successive_edits_recompute_maps_in_both_encodings() {
    for (encoding, start, end) in [
        (PositionEncoding::Utf8, 1, 5),
        (PositionEncoding::Utf16, 1, 3),
    ] {
        let mut fs = setup(encoding);
        fs.process_changes(&[open("a😀e\u{301}\r\nnext".as_bytes())])
            .unwrap();
        let old = fs.get_file(b"/test1.ts").unwrap().unwrap();
        let mut change = event(K::Change);
        change.version = 2;
        change.changes = vec![
            Edit {
                partial: Some(Box::new(TextDocumentContentChangePartial {
                    range: Range {
                        start: Position {
                            line: 0,
                            character: start,
                        },
                        end: Position {
                            line: 0,
                            character: end,
                        },
                    },
                    text: "x\n".into(),
                    range_length: None,
                })),
                ..Default::default()
            },
            Edit {
                partial: Some(Box::new(TextDocumentContentChangePartial {
                    range: Range {
                        start: Position {
                            line: 2,
                            character: 0,
                        },
                        end: Position {
                            line: 2,
                            character: 4,
                        },
                    },
                    text: "last".into(),
                    range_length: None,
                })),
                ..Default::default()
            },
        ];
        fs.process_changes(&[change]).unwrap();
        let new = fs.get_file(b"/test1.ts").unwrap().unwrap();
        assert_eq!(new.content().as_bytes(), "ax\ne\u{301}\r\nlast".as_bytes());
        assert_eq!(old.content().as_bytes(), "a😀e\u{301}\r\nnext".as_bytes());
        assert_eq!(new.version(), 2);
        assert!(!new.matches_disk_text());
    }
}
