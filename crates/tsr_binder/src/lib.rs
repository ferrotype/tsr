//! File binding and hook-driven name/reference resolution at the pinned source.
mod backend;
mod local;
#[cfg(test)]
mod local_tests;
mod state;
mod symbol_access;
mod table_access;
mod target;
pub use state::ContainerFlags;
pub(crate) use state::{ActiveLabel, Binder};

pub(crate) fn need<T>(value: Option<T>) -> T {
    value.expect("pinned binder requires a nonnil value")
}

// Existing shared helper modules keep their source domains. This internal import
// surface avoids binder-local transcriptions of already ported AST operations.
pub(crate) mod ast {
    pub use ts_ast::utilities::*;
    pub use ts_ast::utilities_middle::*;
    pub use ts_ast::*;
}
pub(crate) fn checked<T>(result: Result<T, ts_arena::Error>) -> T {
    result.expect("binder graph belongs to its retained source")
}

mod binary_trampoline;
mod bindings;
mod container_classification;
mod containers;
mod declarations;
mod diagnostics;
mod dispatch;
mod expando;
mod expressions;
mod flow;
mod flow_access;
mod modules;
pub mod name_resolver;
mod recursion;
pub mod reference_resolver;
mod statements;
pub use containers::get_container_flags;
pub use declarations::get_symbol_name_for_private_identifier;
pub use diagnostics::find_use_strict_prologue;

/// Bind one logical source file once, retaining its completed graph explicitly.
/// First binding runs on the reserved native parser stack before entering its
/// publication guard. Cache hits perform no worker dispatch.
// port: tsc/internal/binder/binder.go:BindSourceFile
pub fn bind_source_file(
    file: &ts_ast::AstFile,
    source: ts_ast::NodeId,
) -> Result<ts_ast::BoundFile, ts_ast::BindError> {
    if !file.is_bound(source)? {
        ts_parser::on_parser_worker(|| bind_source_file_worker(file, source))?;
    }
    file.retain_bound(source)
}

// port: tsc/internal/binder/binder.go:bindSourceFile
fn bind_source_file_worker(
    file: &ts_ast::AstFile,
    source: ts_ast::NodeId,
) -> Result<(), ts_ast::BindError> {
    file.bind_with(source, |builder| {
        initialize_binding(builder);
        Ok(())
    })?;
    Ok(())
}

/// Bind the exclusive parser result before publishing its completed syntax.
/// Unusual constructed owners select the existing shared-publication backend.
pub fn bind_parsed_file(
    parsed: ts_ast::ParsedFile,
) -> Result<ts_ast::CompletedFile, ts_ast::BindError> {
    ts_parser::on_parser_worker(|| {
        parsed.bind_and_publish(|builder| {
            initialize_binding(builder);
            Ok(())
        })
    })
}

fn initialize_binding(builder: &mut ts_ast::BindBuilder<'_>) {
    if builder
        .with_local_scope(|local| {
            run_binding(Binder::from_backend(backend::Backend::Local(local)));
        })
        .is_none()
    {
        run_binding(Binder::new(builder));
    }
}

fn run_binding(mut binder: Binder<'_, '_, '_>) {
    let source = binder.file;
    binder.unreachable_flow = Some(binder.new_flow_node(ts_ast::flow_flags::UNREACHABLE));
    binder.bind(Some(source));
    binder.bind_deferred_expando_assignments();
    binder.builder.set_symbol_count(binder.symbol_count);
}

#[cfg(test)]
mod recursion_tests;

#[cfg(test)]
mod bound_factory_tests;

#[cfg(test)]
mod exclusive_tests;
