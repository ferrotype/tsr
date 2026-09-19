//! Development/capture adapter, not part of the public embedding crate.
//! Reuses S08's actual query walker and diagnostic schedule over Session.
use serde_json::Value;
use std::sync::Arc;
use ts_arena::Counters;
use ts_checker::CheckerOwner;
use ts_embed::{FileCache, Program, ProgramOptions, Session};

#[path = "../../../s08/p5/baseline/mod.rs"]
mod baseline;
#[path = "../../../s08/p5/corpus.rs"]
mod corpus;
#[path = "../../../s08/p5/errors.rs"]
mod errors;
#[path = "../../../s08/p4/executor.rs"]
mod executor;
#[path = "../../../s08/p5/paths.rs"]
mod paths;

/// The external executable supplies the public API constructor explicitly.
pub type LoadSession =
    fn(ProgramOptions, &mut FileCache, &Counters) -> Result<Session, ts_compiler::Error>;

struct Embedding {
    load: LoadSession,
    session: Option<Session>,
}

impl executor::Hooks for Embedding {
    fn load_program(
        &mut self,
        options: ProgramOptions,
        cache: &mut FileCache,
        counters: &Counters,
    ) -> Result<Arc<Program>, ts_compiler::Error> {
        let session = (self.load)(options, cache, counters)?;
        let program = session.program().clone();
        self.session = Some(session);
        Ok(program)
    }

    fn create_checker(
        &mut self,
        program: Arc<Program>,
        _counters: &Counters,
    ) -> Result<Arc<CheckerOwner>, ts_checker::Error> {
        let session = self.session.as_ref().expect("loader created session");
        assert!(Arc::ptr_eq(session.program(), &program));
        session.checker().cloned()
    }
}

pub fn observe(request: &Value, load: LoadSession) -> Value {
    corpus::observe_with(
        request,
        &mut FileCache::new(),
        &mut Embedding {
            load,
            session: None,
        },
        false,
    )
}
