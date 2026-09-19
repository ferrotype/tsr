use crate::{FileCache, Program, ProgramOptions};
use std::sync::{Arc, Barrier};
use ts_arena::{Counters, Counts, Error, SymbolId};
use ts_ast::CompletedFile;
use ts_core::{CompilerOptions, Tristate};
use ts_jsstring::JsString;
use ts_tsoptions::ParsedCommandLine;
use ts_vfs::MemoryBuilder;

fn snapshot(text: &[u8], cache: &mut FileCache, counters: &Counters) -> Program {
    let mut host = MemoryBuilder::new(b"/src", true);
    host.insert_physical(b"/src/main.ts", text);
    Program::load(
        ProgramOptions {
            config: ParsedCommandLine::new(
                CompilerOptions {
                    no_lib: Tristate::TRUE,
                    ..CompilerOptions::default()
                },
                vec![JsString::from_bytes(b"/src/main.ts".as_slice())],
            ),
            host: Arc::new(host.finish()),
            current_directory: JsString::from_bytes(b"/src".as_slice()),
            default_library_path: JsString::from_bytes(b"bundled:///libs".as_slice()),
            skip_module_resolution: false,
        },
        cache,
        counters,
    )
    .unwrap()
}

fn local(file: &CompletedFile, name: &[u8]) -> SymbolId {
    let view = file.view();
    let table = view
        .node_binding(file.source())
        .unwrap()
        .unwrap()
        .locals
        .unwrap();
    view.result()
        .tables()
        .get(table)
        .unwrap()
        .get(name)
        .unwrap()
        .unwrap()
}

#[test]
fn shared_bound_file_retains_identity_until_the_final_response_drops() {
    let counters = Counters::new();
    let mut cache = FileCache::new();
    let first = snapshot(b"const value = 1;", &mut cache, &counters);
    let baseline = counters.snapshot();
    let second = snapshot(b"const value = 1;", &mut cache, &counters);
    assert!(Arc::ptr_eq(&first.files()[0], &second.files()[0]));
    assert_eq!(counters.snapshot(), baseline);
    let file = first.files()[0].bound();
    let symbol = local(file, b"value");
    let retained = file.retain_symbol(symbol).unwrap();
    assert_eq!(local(second.files()[0].bound(), b"value"), symbol);
    drop(first);
    assert_eq!(local(second.files()[0].bound(), b"value"), symbol);
    drop(second);
    cache.prune();
    assert_eq!(retained.symbol().name_bytes(), b"value");
    assert_eq!(local(retained.file(), b"value"), symbol);
    let declaration = retained.symbol().value_declaration().unwrap();
    assert!(retained.file().view().node(declaration).is_ok());
    assert_eq!(counters.snapshot(), baseline);
    drop(retained);
    cache.prune();
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn retained_snapshot_edit_answers_concurrently_and_rejects_old_new_id_crossing() {
    let counters = Counters::new();
    let mut cache = FileCache::new();
    let old = snapshot(b"const before = 1;", &mut cache, &counters);
    let old_file = old.files()[0].bound();
    let old_symbol = local(old_file, b"before");
    let response = old_file.retain_symbol(old_symbol).unwrap();
    let old_source = old_file.source();
    let ready = Barrier::new(2);
    let queried = Barrier::new(2);
    std::thread::scope(|scope| {
        let response = &response;
        let ready = &ready;
        let queried = &queried;
        let reader = scope.spawn(move || {
            ready.wait();
            assert_eq!(local(response.file(), b"before"), old_symbol);
            assert_eq!(
                response
                    .file()
                    .view()
                    .source_file()
                    .unwrap()
                    .text()
                    .as_bytes(),
                b"const before = 1;"
            );
            queried.wait();
            ready.wait();
            assert_eq!(response.symbol().name_bytes(), b"before");
        });
        let edited = snapshot(b"const after = 2;", &mut cache, &counters);
        let new_file = edited.files()[0].bound();
        let new_symbol = local(new_file, b"after");
        assert_eq!(old_source.slot(), new_file.source().slot());
        assert_eq!(old_symbol.slot(), new_symbol.slot());
        assert_ne!(old_source, new_file.source());
        assert_ne!(old_symbol, new_symbol);
        assert!(matches!(
            new_file.view().node(old_source),
            Err(Error::WrongOwner)
        ));
        assert!(matches!(
            old_file.view().node(new_file.source()),
            Err(Error::WrongOwner)
        ));
        assert!(matches!(
            new_file.view().symbol(old_symbol),
            Err(Error::WrongOwner)
        ));
        assert!(matches!(
            old_file.view().symbol(new_symbol),
            Err(Error::WrongOwner)
        ));
        ready.wait();
        queried.wait();
        drop(edited);
        ready.wait();
        reader.join().unwrap();
    });
    drop(old);
    cache.prune();
    assert_eq!(local(response.file(), b"before"), old_symbol);
    drop(response);
    cache.prune();
    assert_eq!(counters.snapshot(), Counts::default());
}
