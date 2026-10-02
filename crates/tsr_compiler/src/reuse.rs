//! Single-source reuse of a completed file graph. Immutable file owners are
//! shared; derived include locations and checker state belong to the new program.
use super::*;
use tsr_ast::{AstView, FileReference, SourceFileRead, SyntaxKind};

pub struct ProgramReuse {
    pub program: Option<Program>,
    /// Retain the speculative parse through a full fallback load so FileCache
    /// can return that exact owner instead of parsing the changed file twice.
    pub file: Option<Arc<ProgramFile>>,
}
impl Program {
    pub fn reuse_program(
        &self,
        changed_path: &[u8],
        host: Arc<dyn FileSystem>,
        cache: &mut FileCache,
        counters: &Counters,
    ) -> Result<ProgramReuse, Error> {
        let Some(&index) = self.by_path.get(changed_path) else {
            return Ok(ProgramReuse {
                program: None,
                file: None,
            });
        };
        let old = &self.files[index];
        let old_source = old.bound().view().source_file()?;
        // Compiler watch cycles force a full transform for mapped edits. Other
        // callers also get a full-build decision until a mapped replacement can
        // retain and validate its complete supplemental owner set.
        if !old_source.content_mapper().is_empty() {
            return Ok(ProgramReuse {
                program: None,
                file: None,
            });
        }
        let Some(content) = host.read_file(old_source.file_name())? else {
            return Ok(ProgramReuse {
                program: None,
                file: None,
            });
        };
        let file = cache.acquire(
            content.text,
            old_source.script_kind,
            old_source.parse_options().clone(),
            counters,
            self.tracing.as_ref(),
        )?;
        let failure = || ProgramReuse {
            program: None,
            file: Some(file.clone()),
        };
        if self.redirect_paths.contains_key(changed_path)
            || self
                .redirect_paths
                .values()
                .any(|path| path.as_bytes() == changed_path)
            || !self.can_replace_file(old, &file)?
        {
            return Ok(failure());
        }
        let options = self.options_for_file(changed_path, old_source.file_name());
        if synthetic_imports(old, options)? || synthetic_imports(&file, options)? {
            return Ok(failure());
        }
        // Verification and loader diagnostics may own source syntax rather than
        // include indexes. Such diagnostics need a full rebuild before replacing
        // their source owner; never leave a dangling diagnostic node in a clone.
        if self
            .option_verification
            .diagnostics
            .iter()
            .chain(&self.loader_diagnostics)
            .chain(&self.content_mapper_diagnostics)
            .chain(&self.content_mapper_option_diagnostics)
            .any(|diagnostic| references_source(diagnostic, old.source()))
        {
            return Ok(failure());
        }
        let mut files = self.files.clone();
        files[index] = file.clone();
        let program = Self {
            tracing: self.tracing.clone(),
            owners: crate::resolver_host::OwnerIndex::from_files(&files),
            include_reasons: self
                .include_reasons
                .iter()
                .map(|(path, reasons)| {
                    (
                        path.clone(),
                        reasons
                            .iter()
                            .map(|reason| Arc::new(reason.fresh_for_program()))
                            .collect(),
                    )
                })
                .collect(),
            references: self.references.fork_published(),
            output_file_to_project_reference_source: self
                .output_file_to_project_reference_source
                .clone(),
            redirect_paths: self.redirect_paths.clone(),
            redirect_file_names: self.redirect_file_names.clone(),
            redirect_order: self.redirect_order.clone(),
            package_resolver: std::sync::Mutex::new(
                self.package_resolver
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .fork_for_host(host.clone()),
            ),
            include_explanations: IncludeExplanations::default(),
            diagnostic_snapshot: crate::program_diagnostics::ProgramDiagnostics::default(),
            declaration_diagnostics: std::sync::Mutex::default(),
            option_verification: self.option_verification.clone(),
            config: self.config.clone(),
            cwd: self.cwd.clone(),
            default_library_path: self.default_library_path.clone(),
            external_paths: self.external_paths.clone(),
            options: self.options.clone(),
            host,
            files,
            by_path: self.by_path.clone(),
            metadata: self.metadata.clone(),
            libs: self.libs.clone(),
            default_lib_files: self.default_lib_files.clone(),
            missing: self.missing.clone(),
            resolutions: self.resolutions.clone(),
            type_resolutions: self.type_resolutions.clone(),
            loader_diagnostics: self.loader_diagnostics.clone(),
            processing_diagnostics: self
                .processing_diagnostics
                .iter()
                .map(ProcessingDiagnostic::fresh_for_program)
                .collect(),
            include_diagnostics: OnceLock::new(),
            trace: Vec::new(),
            single_threaded: self.single_threaded,
            content_mapper_project: self.content_mapper_project.clone(),
            content_mapper_diagnostics: self.content_mapper_diagnostics.clone(),
            content_mapper_option_diagnostics: self.content_mapper_option_diagnostics.clone(),
        };
        Ok(ProgramReuse {
            program: Some(program),
            file: Some(file),
        })
    }
    fn can_replace_file(&self, old: &ProgramFile, new: &ProgramFile) -> Result<bool, Error> {
        let old_view = old.bound().view().ast();
        let new_view = new.bound().view().ast();
        let a = old_view.source_file(old.source())?;
        let b = new_view.source_file(new.source())?;
        if a.parse_options() != b.parse_options()
            || a.script_kind != b.script_kind
            || external(&a) != external(&b)
            || a.uses_uri_style_node_core_modules != b.uses_uri_style_node_core_modules
            || a.check_js_directive.as_ref().map(|d| d.enabled)
                != b.check_js_directive.as_ref().map(|d| d.enabled)
        {
            return Ok(false);
        }
        let options = self.options_for_file(a.parse_options().path.as_bytes(), a.file_name());
        let metadata = self
            .metadata
            .get(a.parse_options().path.as_bytes())
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        let imports_a = a.imports()?;
        let imports_b = b.imports()?;
        if imports_a.len() != imports_b.len() {
            return Ok(false);
        }
        for (&left, &right) in imports_a.iter().zip(imports_b.iter()) {
            let left = left.ok_or(tsr_arena::Error::InvalidGraph)?;
            let right = right.ok_or(tsr_arena::Error::InvalidGraph)?;
            if !equal_names(old_view, left, new_view, right, false)?
                || metadata::usage_mode(old_view, a.file_name(), metadata, left, options)?
                    != metadata::usage_mode(new_view, b.file_name(), metadata, right, options)?
            {
                return Ok(false);
            }
        }
        let augment_a = a.module_augmentations()?;
        let augment_b = b.module_augmentations()?;
        if augment_a.len() != augment_b.len() {
            return Ok(false);
        }
        for (&left, &right) in augment_a.iter().zip(augment_b.iter()) {
            if !equal_names(
                old_view,
                left.ok_or(tsr_arena::Error::InvalidGraph)?,
                new_view,
                right.ok_or(tsr_arena::Error::InvalidGraph)?,
                true,
            )? {
                return Ok(false);
            }
        }
        if !a
            .ambient_module_names()?
            .iter()
            .eq(b.ambient_module_names()?.iter())
        {
            return Ok(false);
        }
        Ok(
            equal_references(a.referenced_files()?.iter(), b.referenced_files()?.iter())
                && equal_references(
                    a.type_reference_directives()?.iter(),
                    b.type_reference_directives()?.iter(),
                )
                && equal_references(
                    a.lib_reference_directives()?.iter(),
                    b.lib_reference_directives()?.iter(),
                ),
        )
    }
}
fn external(source: &SourceFileRead<'_>) -> bool {
    source.external_module_indicator.is_some() || source.common_js_module_indicator().is_some()
}
fn equal_names(
    a: AstView<'_>,
    left: NodeId,
    b: AstView<'_>,
    right: NodeId,
    always_text: bool,
) -> Result<bool, Error> {
    let left_node = a.node(left)?;
    let right_node = b.node(right)?;
    Ok(left_node.kind() == right_node.kind()
        && (!(always_text || left_node.kind() == SyntaxKind::StringLiteral)
            || a.node_text(left)?.as_bytes() == b.node_text(right)?.as_bytes()))
}
fn equal_references<'a>(
    a: impl Iterator<Item = &'a FileReference>,
    b: impl Iterator<Item = &'a FileReference>,
) -> bool {
    let mut a = a;
    let mut b = b;
    loop {
        match (a.next(), b.next()) {
            (None, None) => return true,
            (Some(a), Some(b))
                if a.file_name == b.file_name
                    && a.resolution_mode == b.resolution_mode
                    && a.preserve == b.preserve => {}
            _ => return false,
        }
    }
}
fn synthetic_imports(file: &ProgramFile, options: &CompilerOptions) -> Result<bool, Error> {
    let view = file.bound().view().ast();
    let source = view.source_file(file.source())?;
    let javascript = matches!(source.script_kind, ScriptKind::JS | ScriptKind::JSX);
    if options.import_helpers.is_true()
        && (javascript
            || !source.is_declaration_file
                && (options.isolated_modules() || source.external_module_indicator.is_some()))
    {
        return Ok(true);
    }
    Ok((javascript || source.script_kind == ScriptKind::TSX)
        && !metadata::jsx_runtime_import(
            metadata::jsx_implicit_import_base(view, file.source(), options)?.as_bytes(),
            options,
        )
        .is_empty())
}
fn references_source(diagnostic: &Diagnostic, source: NodeId) -> bool {
    diagnostic.file == Some(source)
        || diagnostic
            .related_information
            .iter()
            .any(|d| references_source(d, source))
        || diagnostic
            .message_chain
            .iter()
            .any(|d| references_source(d, source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tsr_binder::name_resolver::ResolverHost;
    use tsr_vfs::MemoryBuilder;

    fn host(text: &[u8]) -> Arc<dyn FileSystem> {
        let mut host = MemoryBuilder::new(b"/src", true);
        host.insert_physical(b"/src/main.ts", text);
        host.insert_physical(b"/src/dep.ts", b"export const dep = 1;".as_slice());
        Arc::new(host.finish())
    }
    fn load(
        text: &[u8],
        options: CompilerOptions,
        cache: &mut FileCache,
        counters: &Counters,
    ) -> Program {
        Program::load(
            ProgramOptions {
                config: ParsedCommandLine::new(
                    CompilerOptions {
                        no_lib: Tristate::TRUE,
                        ..options
                    },
                    vec![JsString::from_bytes(b"/src/main.ts".as_slice())],
                ),
                host: host(text),
                current_directory: JsString::from_bytes(b"/src".as_slice()),
                default_library_path: JsString::from_bytes(b"/lib".as_slice()),
                skip_module_resolution: false,
                single_threaded: Tristate::TRUE,
            },
            cache,
            counters,
        )
        .unwrap()
    }

    #[test]
    fn single_edit_reuses_graph_and_retains_old_and_new_bound_owners() {
        let counters = Counters::new();
        let mut cache = FileCache::new();
        let old = load(
            b"import {dep} from './dep'; export const value = dep;",
            CompilerOptions::default(),
            &mut cache,
            &counters,
        );
        let old_file = old.files[old.by_path[b"/src/main.ts".as_slice()]].clone();
        let dependency = old.files[old.by_path[b"/src/dep.ts".as_slice()]].clone();
        cache.evict(b"/src/main.ts");
        let reused = old
            .reuse_program(
                b"/src/main.ts",
                host(b"import {dep} from './dep'; export const value = dep + 1;"),
                &mut cache,
                &counters,
            )
            .unwrap();
        let new = reused.program.expect("same import graph is reusable");
        let new_file = &new.files[new.by_path[b"/src/main.ts".as_slice()]];
        assert!(!Arc::ptr_eq(&old_file, new_file));
        assert!(Arc::ptr_eq(
            &dependency,
            &new.files[new.by_path[b"/src/dep.ts".as_slice()]]
        ));
        assert_eq!(old.resolutions.len(), new.resolutions.len());
        assert!(old.resolver_host(&counters).ast(new_file.source()).is_err());
        assert!(new.resolver_host(&counters).ast(old_file.source()).is_err());
        drop(old);
        assert_eq!(
            old_file
                .bound()
                .view()
                .source_file()
                .unwrap()
                .text()
                .as_bytes(),
            b"import {dep} from './dep'; export const value = dep;"
        );
        assert!(new.resolver_host(&counters).ast(new_file.source()).is_ok());
        drop(reused.file);
        drop(new);
        drop(old_file);
        drop(dependency);
        cache.prune();
        assert_eq!(counters.snapshot(), tsr_arena::Counts::default());
    }

    #[test]
    fn structural_edits_and_synthetic_imports_require_full_loading() {
        for (before, after, options) in [
            (
                "import './dep';",
                "import './other';",
                CompilerOptions::default(),
            ),
            (
                "const value = 1;",
                "export const value = 1;",
                CompilerOptions::default(),
            ),
            (
                "/// <reference path='./dep.ts' />\nconst value = 1;",
                "/// <reference path='./other.ts' />\nconst value = 1;",
                CompilerOptions::default(),
            ),
            (
                "// @ts-check\nexport {};",
                "// @ts-nocheck\nexport {};",
                CompilerOptions::default(),
            ),
            (
                "declare module 'a' {}",
                "declare module 'b' {}",
                CompilerOptions::default(),
            ),
            (
                "export const value = 1;",
                "export const value = 2;",
                CompilerOptions {
                    import_helpers: Tristate::TRUE,
                    ..Default::default()
                },
            ),
        ] {
            let counters = Counters::new();
            let mut cache = FileCache::new();
            let old = load(before.as_bytes(), options, &mut cache, &counters);
            let reused = old
                .reuse_program(
                    b"/src/main.ts",
                    host(after.as_bytes()),
                    &mut cache,
                    &counters,
                )
                .unwrap();
            assert!(
                reused.program.is_none(),
                "must reload: {before:?} -> {after:?}"
            );
            assert!(
                reused.file.is_some(),
                "fallback retains its speculative parse"
            );
        }
    }

    #[test]
    fn reuse_recomputes_cached_include_diagnostic_positions() {
        let counters = Counters::new();
        let mut cache = FileCache::new();
        let before = b"import './dep'; export {};";
        let after = b"\n\nimport './dep'; export {};";
        let old = load(before, CompilerOptions::default(), &mut cache, &counters);
        let explain = |program: &Program| {
            program
                .explain_file_include(
                    b"/src/dep.ts",
                    tsr_diagnostics::File_0_not_found,
                    vec![JsString::from_bytes(b"dep".as_slice())],
                )
                .unwrap()
        };
        let old_diagnostic = explain(&old);
        let new = old
            .reuse_program(b"/src/main.ts", host(after), &mut cache, &counters)
            .unwrap()
            .program
            .expect("import text did not change");
        let new_diagnostic = explain(&new);
        assert_eq!(new_diagnostic.loc.pos(), old_diagnostic.loc.pos() + 2);
        assert_ne!(new_diagnostic.file, old_diagnostic.file);
        assert_eq!(
            old_diagnostic.file,
            Some(old.source_file(b"/src/main.ts").unwrap().source())
        );
        assert_eq!(
            new_diagnostic.file,
            Some(new.source_file(b"/src/main.ts").unwrap().source())
        );
    }
}
