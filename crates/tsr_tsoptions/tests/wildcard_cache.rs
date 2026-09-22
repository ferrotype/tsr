use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;
use tsr_tsoptions::{ConfigFileSpecs, ParsedCommandLine};
fn specs() -> ConfigFileSpecs {
    ConfigFileSpecs {
        validated_includes: vec![JsString::from_bytes(b"Source/**/*.ts".as_slice())],
        ..ConfigFileSpecs::default()
    }
}
#[test]
fn wildcard_cache_is_lazy_concurrent_and_invalidated_by_context_replacement() {
    let mut parsed = ParsedCommandLine::new(CompilerOptions::default(), vec![]);
    parsed.set_config_specs(
        Some(specs()),
        JsString::from_bytes(b"/old".as_slice()),
        true,
    );
    // Changes before the first read must affect the lazy calculation.
    parsed.set_config_specs(
        Some(specs()),
        JsString::from_bytes(b"/project".as_slice()),
        true,
    );
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        let threads = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    let result = parsed.wildcard_directories().unwrap();
                    assert_eq!(result.get(b"/project/Source".as_slice()), Some(&true));
                    std::ptr::from_ref(result) as usize
                })
            })
            .collect::<Vec<_>>();
        let addresses = threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect::<Vec<_>>();
        assert!(addresses.iter().all(|address| *address == addresses[0]));
    });
    let original = parsed.wildcard_directories().unwrap();
    let mut clone = parsed.clone();
    assert!(!std::ptr::eq(
        original,
        clone.wildcard_directories().unwrap()
    ));
    let mut changed = specs();
    changed.validated_excludes = vec![JsString::from_bytes(b"source".as_slice())];
    clone.set_config_specs(
        Some(changed),
        JsString::from_bytes(b"/project".as_slice()),
        false,
    );
    assert!(clone.wildcard_directories().unwrap().is_empty());
    assert_eq!(parsed.wildcard_directories().unwrap().len(), 1);
    clone.set_config_specs(
        Some(specs()),
        JsString::from_bytes(b"/new".as_slice()),
        true,
    );
    assert_eq!(
        clone
            .wildcard_directories()
            .unwrap()
            .get(b"/new/Source".as_slice()),
        Some(&true)
    );
    clone.set_config_specs(None, JsString::default(), true);
    assert!(clone.wildcard_directories().is_none());
}
