use std::sync::{Arc, Barrier};
use tsr_jsstring::JsString;
use tsr_module::{InfoCache, InfoCacheEntry, PackageJson};

fn entry(label: &[u8], present: bool) -> Arc<InfoCacheEntry> {
    Arc::new(InfoCacheEntry {
        package_directory: JsString::from_bytes(label),
        directory_exists: true,
        contents: present.then(|| Arc::new(PackageJson::parse(label, b"{}"))),
    })
}

#[test]
fn concurrent_stores_retain_one_identity_and_range_can_reenter() {
    let cache = Arc::new(InfoCache::new(b"/root", false));
    let start = Arc::new(Barrier::new(8));
    let winners = std::thread::scope(|scope| {
        (0..8)
            .map(|i| {
                let cache = cache.clone();
                let start = start.clone();
                scope.spawn(move || {
                    let item = entry(format!("/entry{i}").as_bytes(), i % 2 == 0);
                    start.wait();
                    cache.set(b"PKG/../pkg/package.json", item)
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect::<Vec<_>>()
    });
    let first = &winners[0];
    assert!(winners.iter().all(|winner| Arc::ptr_eq(winner, first)));
    assert!(Arc::ptr_eq(
        &cache.get(b"/ROOT/pkg/package.json").unwrap(),
        first
    ));
    let mut visits = 0;
    cache.range(|key, winner| {
        visits += 1;
        assert!(Arc::ptr_eq(&cache.get(key.as_bytes()).unwrap(), winner));
        cache.set(b"other/package.json", entry(b"other", false));
        false
    });
    assert_eq!(visits, 1);
    assert!(cache.get(b"other/package.json").is_some());
}

#[test]
fn negative_entries_and_directory_views_preserve_cache_contract() {
    let cache = InfoCache::new(b"/root", true);
    assert!(cache.get(b"pkg/package.json").is_none());
    let absent = entry(b"/root/pkg", false);
    cache.set(b"pkg/package.json", absent.clone());
    let winner = cache.set(b"pkg/package.json", entry(b"/root/pkg", true));
    assert!(Arc::ptr_eq(&absent, &winner));
    assert!(!winner.exists());
    assert!(cache.get(b"PKG/package.json").is_none());
    let original = entry(b"/root/pkg", true);
    assert!(Arc::ptr_eq(
        &original.with_package_directory(b"/root/pkg"),
        &original
    ));
    let alias = original.with_package_directory(b"/root/pkg/");
    assert!(!Arc::ptr_eq(&alias, &original));
    assert!(Arc::ptr_eq(
        alias.contents.as_ref().unwrap(),
        original.contents.as_ref().unwrap()
    ));
    assert_eq!(alias.package_directory.as_bytes(), b"/root/pkg/");
}
