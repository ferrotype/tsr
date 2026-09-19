//! Declaration diagnostics execute the native transformation and checker emit
//! resolver; no output file is written and semantic skip/noEmit rules do not
//! substitute for this phase's own source selection.
use crate::{Error, Program, ProgramFile};
use ts_ast::{AstBuilder, Diagnostic};
use ts_checker::Operation;
use ts_core::ScriptKind;
use ts_printer::EmitContext;
use ts_transformers::declarations::{transform_declarations, DeclarationOptions};

impl Program {
    // port: tsc/internal/compiler/program.go:Program.getDeclarationDiagnosticsForFile
    // port: tsc/internal/compiler/emitter.go:getDeclarationDiagnostics
    pub fn declaration_diagnostics_with_checker(
        &self,
        operation: &mut Operation<'_>,
        file: &ProgramFile,
    ) -> Result<Vec<Diagnostic>, Error> {
        let source = file.bound().view().source_file()?;
        let retained = self
            .file(source.parse_options().path.as_bytes())
            .ok_or(ts_arena::Error::WrongOwner)?;
        if retained.source() != file.source() {
            return Err(ts_arena::Error::WrongOwner.into());
        }
        if source.is_declaration_file {
            return Ok(Vec::new());
        }
        // As in native LoadOrStore, the cache lock never surrounds a checker or
        // transformer callback. Only complete successful results enter the map.
        if let Some(cached) = self
            .declaration_diagnostics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&file.source())
            .cloned()
        {
            return Ok(cached);
        }
        let diagnostics = if source.script_kind == ScriptKind::JSON
            || !crate::output_paths::may_emit(file, self)?
        {
            Vec::new()
        } else {
            let counters = ts_arena::Counters::new();
            let mut emit = EmitContext::new();
            let mut output = AstBuilder::with_hooks(
                ts_jsstring::SourceText::from_loaded_bytes(&b""[..]),
                &counters,
                emit.factory_hooks(),
            );
            let options = DeclarationOptions {
                isolated_declarations: self.options().isolated_declarations.is_true(),
                strip_internal: self.options().strip_internal.is_true(),
            };
            let transformed =
                transform_declarations(operation, &mut output, &mut emit, file.source(), options)?;
            // Completion validates generated and retained graph edges even though
            // this query discards syntax after collecting its diagnostics.
            let _completed = output.complete(transformed.root)?;
            transformed.diagnostics
        };
        let mut cache = self
            .declaration_diagnostics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(cache.entry(file.source()).or_insert(diagnostics).clone())
    }
}
