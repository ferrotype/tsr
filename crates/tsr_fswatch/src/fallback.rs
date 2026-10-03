#![forbid(unsafe_code)]
use crate::{Error, WatchDirectoryRequest};
/// Retry only unsupported filesystems, preserving the primary owner's batch
/// fast path. Secondary requests use the public inotify owner, so its directory
/// table, native backend, and debounce worker are shared with direct watches.
// port: tsc/internal/fswatch/watcher.go:fallbackWatcher.WatchDirectories
pub(crate) fn watch_directories<T>(
    requests: &[WatchDirectoryRequest],
    mut primary: impl FnMut(&[WatchDirectoryRequest]) -> Result<Vec<T>, Error>,
    mut secondary: impl FnMut(&WatchDirectoryRequest) -> Result<T, Error>,
) -> Result<Vec<T>, Error> {
    match primary(requests) {
        Ok(watches) => return Ok(watches),
        Err(error) if error.is_filesystem_unsupported() => {}
        Err(error) => return Err(error),
    }
    let mut watches = Vec::with_capacity(requests.len());
    for request in requests {
        let result = match primary(std::slice::from_ref(request)) {
            Ok(mut watch) => Ok(watch.remove(0)),
            Err(error) if error.is_filesystem_unsupported() => secondary(request),
            Err(error) => Err(error),
        };
        match result {
            Ok(watch) => watches.push(watch),
            Err(error) => {
                // Closing in reverse creation order also tears down newly
                // initialized backends when an otherwise-empty batch fails.
                while watches.pop().is_some() {}
                return Err(error.context_prefix(format!(
                    "fswatch: failed to watch directory {:?}",
                    String::from_utf8_lossy(&request.dir)
                )));
            }
        }
    }
    Ok(watches)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock;
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct Fake {
        failure: Option<(Vec<u8>, Error)>,
        active: Mutex<Vec<Vec<u8>>>,
        closed: Mutex<Vec<Vec<u8>>>,
    }
    struct Subscription {
        owner: Arc<Fake>,
        path: Vec<u8>,
    }
    impl Drop for Subscription {
        fn drop(&mut self) {
            lock(&self.owner.active).retain(|path| *path != self.path);
            lock(&self.owner.closed).push(self.path.clone());
        }
    }
    impl Fake {
        fn watch_many(
            self: &Arc<Self>,
            requests: &[WatchDirectoryRequest],
        ) -> Result<Vec<Subscription>, Error> {
            if let Some((prefix, error)) = &self.failure {
                if requests.iter().any(|r| r.dir.starts_with(prefix)) {
                    return Err(error.clone());
                }
            }
            lock(&self.active).extend(requests.iter().map(|r| r.dir.clone()));
            Ok(requests
                .iter()
                .map(|r| Subscription {
                    owner: self.clone(),
                    path: r.dir.clone(),
                })
                .collect())
        }
    }
    fn requests(dirs: &[&[u8]]) -> Vec<WatchDirectoryRequest> {
        dirs.iter()
            .map(|dir| WatchDirectoryRequest {
                dir: dir.to_vec(),
                callback: Arc::new(|_, _| {}),
                options: Default::default(),
            })
            .collect()
    }
    fn fallback(
        primary: &Arc<Fake>,
        secondary: &Arc<Fake>,
        dirs: &[&[u8]],
    ) -> Result<Vec<Subscription>, Error> {
        watch_directories(
            &requests(dirs),
            |r| primary.watch_many(r),
            |r| {
                secondary
                    .watch_many(std::slice::from_ref(r))
                    .map(|mut watches| watches.remove(0))
            },
        )
    }
    #[test]
    // source: tsc/internal/fswatch/fallback_test.go:TestFallbackWatcherRoutesUnsupportedDirectories
    fn routes_only_filesystem_unsupported_watches() {
        let primary = Arc::new(Fake {
            failure: Some((b"/mnt/fuse".to_vec(), Error::FilesystemUnsupported)),
            ..Fake::default()
        });
        let secondary = Arc::new(Fake::default());
        let watches = fallback(
            &primary,
            &secondary,
            &[
                b"/project",
                b"/project/src",
                b"/mnt/fuse/deps",
                b"/mnt/fuse/deps/a",
            ],
        )
        .unwrap();
        assert_eq!(
            *lock(&primary.active),
            [b"/project".to_vec(), b"/project/src".to_vec()]
        );
        assert_eq!(
            *lock(&secondary.active),
            [b"/mnt/fuse/deps".to_vec(), b"/mnt/fuse/deps/a".to_vec()]
        );
        drop(watches);
        assert!(lock(&primary.active).is_empty());
        assert!(lock(&secondary.active).is_empty());
    }
    #[test]
    // source: tsc/internal/fswatch/fallback_test.go:TestFallbackWatcherDoesNotFallbackForUnrelatedError
    fn unrelated_errors_do_not_fallback() {
        let primary = Arc::new(Fake {
            failure: Some((Vec::new(), Error::Unavailable)),
            ..Fake::default()
        });
        let secondary = Arc::new(Fake::default());
        assert!(matches!(
            fallback(&primary, &secondary, &[b"/project"]),
            Err(Error::Unavailable)
        ));
        assert!(lock(&secondary.active).is_empty());
    }
    #[test]
    // source: tsc/internal/fswatch/fallback_test.go:TestFallbackWatcherDoesNotUseSecondaryOnHappyPath
    fn supported_filesystem_does_not_initialize_secondary() {
        let primary = Arc::new(Fake::default());
        let watches = watch_directories(
            &requests(&[b"/project", b"/project/src"]),
            |r| primary.watch_many(r),
            |_| panic!("secondary should stay idle"),
        )
        .unwrap();
        assert_eq!(lock(&primary.active).len(), 2);
        drop(watches);
    }
    #[test]
    // source: tsc/internal/fswatch/fallback_test.go:TestFallbackWatcherRollsBackRoutedWatchesOnFailure
    fn rollback_closes_primary_and_routed_watches() {
        let primary = Arc::new(Fake {
            failure: Some((b"/mnt".to_vec(), Error::FilesystemUnsupported)),
            ..Fake::default()
        });
        let secondary = Arc::new(Fake {
            failure: Some((b"/mnt/broken".to_vec(), Error::Unavailable)),
            ..Fake::default()
        });
        let error = fallback(
            &primary,
            &secondary,
            &[b"/project", b"/mnt/fuse", b"/mnt/broken"],
        )
        .err()
        .expect("routed unavailable directory must fail");
        assert!(matches!(error, Error::Prefix { source, .. } if *source == Error::Unavailable));
        assert_eq!(*lock(&primary.closed), [b"/project".to_vec()]);
        assert_eq!(*lock(&secondary.closed), [b"/mnt/fuse".to_vec()]);
        assert!(lock(&primary.active).is_empty());
        assert!(lock(&secondary.active).is_empty());
    }
    #[test]
    fn secondary_initialization_failure_also_rolls_back() {
        let primary = Arc::new(Fake {
            failure: Some((b"/mnt".to_vec(), Error::FilesystemUnsupported)),
            ..Fake::default()
        });
        let result = watch_directories(
            &requests(&[b"/project", b"/mnt/fuse"]),
            |r| primary.watch_many(r),
            |_| Err(Error::Unavailable),
        );
        assert!(result.is_err());
        assert_eq!(*lock(&primary.closed), [b"/project".to_vec()]);
        assert!(lock(&primary.active).is_empty());
    }
}
