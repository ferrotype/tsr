use super::state::Projection;
use crate::wire::{raw, Json};
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_core::CompilerOptions;
use tsr_lsp::Server;
use tsr_project::{
    parse_cache::{ContentMappedParseCache, ParseCache},
    ref_count_cache::RefCountCacheOptions,
    session::{Session, SessionOptions},
};
use tsr_vfs::FileSystem;

pub(super) enum Action {
    Initialize {
        options: SessionOptions,
        compiler: CompilerOptions,
        host: Arc<dyn FileSystem>,
    },
    Options(CompilerOptions),
    Notification {
        method: String,
        params: Option<tsr_json::RawValue>,
    },
    State,
    Reset,
}
pub(super) struct Worker {
    server: Option<Server>,
    cache: Arc<ParseCache>,
    mapped_cache: Arc<ContentMappedParseCache>,
    counters: Counters,
    projection: Projection,
    notification_error: Option<String>,
}
impl Worker {
    pub(super) fn poison(&mut self) {
        self.notification_error = Some("project operation panicked; reset the test session".into());
    }
    pub(super) fn new() -> Self {
        Self {
            server: None,
            cache: Arc::new(ParseCache::new(RefCountCacheOptions {
                disable_deletion: true,
            })),
            mapped_cache: Arc::new(ContentMappedParseCache::new(RefCountCacheOptions {
                disable_deletion: true,
            })),
            counters: Counters::new(),
            projection: Projection::default(),
            notification_error: None,
        }
    }
    pub(super) fn run(
        &mut self,
        action: Action,
        request_host: Arc<dyn FileSystem>,
    ) -> Result<Json, String> {
        if matches!(action, Action::Reset) {
            self.reset();
            return Ok(raw(&serde_json::json!({"reset":true})));
        }
        if let Some(error) = &self.notification_error {
            return Err(error.clone());
        }
        match action {
            Action::Initialize {
                options,
                compiler,
                host,
            } => {
                if self.server.is_some() {
                    return Err("project session already initialized".into());
                }
                let session = Session::with_caches(
                    options,
                    host,
                    &self.counters,
                    self.cache.clone(),
                    self.mapped_cache.clone(),
                );
                session
                    .apply_inferred_options(compiler, request_host)
                    .map_err(|e| e.to_string())?;
                self.server = Some(Server::new(session));
                Ok(raw(&()))
            }
            Action::Options(options) => {
                self.server
                    .as_ref()
                    .ok_or("project session is not initialized")?
                    .session()
                    .apply_inferred_options(options, request_host)
                    .map_err(|e| e.to_string())?;
                Ok(raw(&()))
            }
            Action::Notification { method, params } => {
                let result = self
                    .server
                    .as_ref()
                    .ok_or("project session is not initialized")?
                    .notification(&method, params.as_ref(), request_host)
                    .map_err(|e| e.message);
                if let Err(error) = &result {
                    self.notification_error = Some(error.clone());
                }
                result.map(|()| raw(&()))
            }
            Action::State => {
                let snapshot = self
                    .server
                    .as_ref()
                    .ok_or("project session is not initialized")?
                    .snapshot(request_host)
                    .map_err(|e| e.message)?;
                self.projection.read(&snapshot).map(|v| raw(&v))
            }
            Action::Reset => unreachable!(),
        }
    }
    fn reset(&mut self) {
        if let Some(server) = self.server.take() {
            server.close();
        }
        self.projection = Projection::default();
        self.notification_error = None;
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tsr_core::{ModuleDetectionKind, Tristate};
    use tsr_jsstring::PositionEncoding;
    use tsr_vfs::MemoryBuilder;

    fn host(library: &str) -> Arc<dyn FileSystem> {
        let mut fs = MemoryBuilder::new(b"/", true);
        fs.insert_loaded(b"/shared.d.ts", library.as_bytes());
        Arc::new(fs.finish())
    }
    fn initialize(
        worker: &mut Worker,
        host: &Arc<dyn FileSystem>,
        encoding: PositionEncoding,
        strict: bool,
        force: bool,
    ) {
        worker
            .run(
                Action::Initialize {
                    options: SessionOptions {
                        position_encoding: encoding,
                        ..Default::default()
                    },
                    compiler: CompilerOptions {
                        no_lib: Tristate::TRUE,
                        strict: Tristate::from(strict),
                        module_detection: if force {
                            ModuleDetectionKind::FORCE
                        } else {
                            ModuleDetectionKind::LEGACY
                        },
                        ..Default::default()
                    },
                    host: host.clone(),
                },
                host.clone(),
            )
            .unwrap();
    }
    fn notification(
        worker: &mut Worker,
        host: &Arc<dyn FileSystem>,
        method: &str,
        params: &serde_json::Value,
    ) {
        worker
            .run(
                Action::Notification {
                    method: method.into(),
                    params: Some(tsr_json::RawValue(serde_json::to_vec(params).unwrap())),
                },
                host.clone(),
            )
            .unwrap();
    }
    fn open(worker: &mut Worker, host: &Arc<dyn FileSystem>, name: &str) {
        notification(
            worker,
            host,
            "textDocument/didOpen",
            &json!({"textDocument":{"uri":format!("file://{name}"),"version":1,"languageId":"typescript","text":"//💩\n/// <reference path=\"/shared.d.ts\" />\nconst x = shared;"}}),
        );
    }

    // The real batch worker retains only parses. Reset must destroy each
    // session even when its worker/cache stays alive for the next test.
    #[test]
    fn batch_reset_reuses_library_parses_without_leaking_session_state() {
        let mut worker = Worker::new();
        let counters = worker.counters.clone();
        let baseline = counters.snapshot();
        let fs = host("declare const shared: number;");
        initialize(&mut worker, &fs, PositionEncoding::Utf16, true, false);
        open(&mut worker, &fs, "/first.ts");
        let session = worker.server.as_ref().unwrap().session();
        let old_session = Arc::downgrade(session);
        let first = session.snapshot().unwrap();
        let old_fs = Arc::downgrade(first.filesystem().unwrap());
        let lib = first
            .project()
            .program()
            .unwrap()
            .source_file(b"/shared.d.ts")
            .unwrap();
        let library_id = lib.source();
        let cached_library = Arc::downgrade(
            first
                .project()
                .program()
                .unwrap()
                .files()
                .iter()
                .find(|f| f.source() == library_id)
                .unwrap(),
        );
        assert!(first
            .project()
            .program()
            .unwrap()
            .options()
            .strict
            .is_true());
        drop(first);
        worker.run(Action::Reset, fs.clone()).unwrap();
        assert!(old_session.upgrade().is_none());
        assert!(old_fs.upgrade().is_none());
        assert!(
            cached_library.upgrade().is_some(),
            "only the explicit test cache retains syntax"
        );
        initialize(&mut worker, &fs, PositionEncoding::Utf8, false, false);
        assert!(worker
            .server
            .as_ref()
            .unwrap()
            .session()
            .snapshot()
            .unwrap()
            .projects()
            .is_empty());
        open(&mut worker, &fs, "/second.ts");
        notification(
            &mut worker,
            &fs,
            "textDocument/didChange",
            &json!({"textDocument":{"uri":"file:///second.ts","version":2},"contentChanges":[{"range":{"start":{"line":0,"character":6},"end":{"line":0,"character":6}},"text":"!"}]}),
        );
        worker.run(Action::State, fs.clone()).unwrap();
        let second = worker
            .server
            .as_ref()
            .unwrap()
            .session()
            .snapshot()
            .unwrap();
        let program = second.project().program().unwrap();
        assert!(!program.options().strict.is_true());
        assert!(program.source_file(b"/first.ts").is_none());
        assert_eq!(
            program.source_file(b"/shared.d.ts").unwrap().source(),
            library_id
        );
        let text = program
            .source_file(b"/second.ts")
            .unwrap()
            .bound()
            .view()
            .source_file()
            .unwrap()
            .text()
            .clone();
        assert!(
            text.as_bytes().starts_with("//💩!\n".as_bytes()),
            "UTF-8 positions from the second initialization must be used"
        );
        drop(second);
        worker.run(Action::Reset, fs.clone()).unwrap();
        let changed = host("declare const shared: string;");
        initialize(&mut worker, &changed, PositionEncoding::Utf16, false, false);
        open(&mut worker, &changed, "/third.ts");
        let third = worker
            .server
            .as_ref()
            .unwrap()
            .session()
            .snapshot()
            .unwrap();
        assert_ne!(
            third
                .project()
                .program()
                .unwrap()
                .source_file(b"/shared.d.ts")
                .unwrap()
                .source(),
            library_id,
            "changed content must not reuse the cached library"
        );
        drop(third);
        worker.run(Action::Reset, fs.clone()).unwrap();
        initialize(&mut worker, &fs, PositionEncoding::Utf16, false, true);
        open(&mut worker, &fs, "/fourth.ts");
        let fourth = worker
            .server
            .as_ref()
            .unwrap()
            .session()
            .snapshot()
            .unwrap();
        // force-module applies only to non-declaration files. Compare the same
        // source name and bytes in another session with different parse options.
        let forced = fourth
            .project()
            .program()
            .unwrap()
            .source_file(b"/fourth.ts")
            .unwrap()
            .source();
        drop(fourth);
        worker.run(Action::Reset, fs.clone()).unwrap();
        initialize(&mut worker, &fs, PositionEncoding::Utf16, false, false);
        open(&mut worker, &fs, "/fourth.ts");
        let plain = worker
            .server
            .as_ref()
            .unwrap()
            .session()
            .snapshot()
            .unwrap();
        assert_ne!(
            plain
                .project()
                .program()
                .unwrap()
                .source_file(b"/fourth.ts")
                .unwrap()
                .source(),
            forced
        );
        drop(plain);
        drop(worker);
        assert!(cached_library.upgrade().is_none());
        assert_eq!(counters.snapshot(), baseline);
    }
}
