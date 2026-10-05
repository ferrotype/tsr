//! File moves update paths in the retained configuration and every loaded
//! source file, using the same module-specifier generator as auto imports.
use crate::{
    change::{self, Tracker},
    change_nodes::NodeTracker,
    reference_helpers as h,
    syntax::Syntax,
    LanguageService, Result,
};
use tsr_ast::{AstView, FactoryMethods, NodeId, SyntaxKind as K};
use tsr_checker::Operation;
use tsr_core::TextRange;
use tsr_lsproto as lsp;
use tsr_tspath as path;

struct PathUpdater<'a> {
    old: &'a [u8],
    new: &'a [u8],
    case_sensitive: bool,
}
impl PathUpdater<'_> {
    // port: tsc/internal/ls/file_rename.go:LanguageService.createPathUpdater
    fn update(&self, file: &[u8]) -> Option<Vec<u8>> {
        if path::compare_paths(file, self.old, b"", self.case_sensitive).is_eq() {
            return Some(self.new.to_vec());
        }
        let old = path::remove_trailing_directory_separator(self.old);
        let suffix = path::trim_file_path_prefix(file, old, self.case_sensitive)?;
        (suffix.starts_with(b"/") || suffix.starts_with(b"\\")).then(|| [self.new, suffix].concat())
    }
    fn relative(&self, from: &[u8], to: &[u8]) -> Vec<u8> {
        path::relative_from_directory(from, to, b"", self.case_sensitive)
    }
    // port: tsc/internal/ls/file_rename.go:LanguageService.updateRelativePath
    fn update_relative(&self, old_from: &[u8], new_from: &[u8], specifier: &[u8]) -> Vec<u8> {
        let old =
            path::normalize(&path::combine(&path::directory(old_from), &[specifier])).into_owned();
        let new = self.update(&old).unwrap_or(old);
        path::ensure_path_is_non_module_name(&self.relative(&path::directory(new_from), &new))
            .into_owned()
    }
}
pub(crate) fn rename_file(
    old_uri: lsp::DocumentUri,
    new_uri: lsp::DocumentUri,
) -> lsp::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile {
    lsp::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile {
        rename_file: Some(Box::new(lsp::RenameFile {
            old_uri,
            new_uri,
            ..Default::default()
        })),
        ..Default::default()
    }
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/file_rename.go:LanguageService.GetEditsForFileRename
    pub fn file_rename(
        &mut self,
        c: &mut Operation<'_>,
        old: &lsp::DocumentUri,
        new: &lsp::DocumentUri,
        preferences: &tsr_autoimport::Preferences,
        format: &tsr_format::FormatCodeSettings,
    ) -> Result<Vec<lsp::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile>> {
        self.check_canceled()?;
        let excluded = |specifier: &[u8]| preferences.excludes(specifier);
        let specifier_preferences = tsr_checker::ModuleSpecifierPreferences {
            relative: preferences.module_specifier.as_deref(),
            ending: preferences.ending.as_deref(),
            excluded: &excluded,
        };
        let old = old.file_name();
        let new = new.file_name();
        let updater = PathUpdater {
            old: old.as_bytes(),
            new: new.as_bytes(),
            case_sensitive: self.program.use_case_sensitive_file_names(),
        };
        let mut tracker = NodeTracker::new(self.program, format);
        self.update_config_paths(&mut tracker, &updater)?;
        self.update_import_paths(c, &mut tracker.raw, &updater, specifier_preferences)?;
        let mut result = Vec::new();
        if path::is_declaration_file_name(old.as_bytes())
            && path::is_declaration_file_name(new.as_bytes())
        {
            // The pin derives both extension sets from oldPath.
            for ext in path::possible_original_input_extensions(old.as_bytes()) {
                let original = path::change_full_extension(old.as_bytes(), &ext);
                if self
                    .program
                    .host()
                    .file_exists(&original)
                    .map_err(tsr_compiler::Error::from)?
                {
                    result.push(rename_file(
                        lsp::DocumentUri::from_file_name(&original),
                        lsp::DocumentUri::from_file_name(&path::change_full_extension(
                            new.as_bytes(),
                            &ext,
                        )),
                    ));
                }
            }
        }
        result.extend(change::document_edits(tracker.finish(self)?));
        Ok(result)
    }
    // port: tsc/internal/ls/file_rename.go:LanguageService.updateImportsForFileRename
    fn update_import_paths(
        &self,
        c: &mut Operation<'_>,
        tracker: &mut Tracker,
        updater: &PathUpdater<'_>,
        specifier_preferences: tsr_checker::ModuleSpecifierPreferences<'_>,
    ) -> Result<()> {
        let mut moved = Vec::new();
        for file in self.program.files() {
            let source = self.source(file.source())?;
            if let Some(new) = updater.update(source.original_file_name()?.as_bytes()) {
                moved.push((file.source(), new));
            }
        }
        for file in self.program.files() {
            self.check_canceled()?;
            let source_id = file.source();
            let view = file.bound().view().ast();
            let source = view.source_file(source_id)?;
            let old_name = source.original_file_name()?;
            let updated = updater.update(old_name.as_bytes());
            let new_from = updated.as_deref().unwrap_or(old_name.as_bytes());
            for reference in source.referenced_files()?.iter() {
                if !path::is_external_module_name_relative(reference.file_name.as_bytes()) {
                    continue;
                }
                let new = updater.update_relative(
                    old_name.as_bytes(),
                    new_from,
                    reference.file_name.as_bytes(),
                );
                if new != reference.file_name.as_bytes() {
                    tracker.replace_text(
                        source_id,
                        reference.loc,
                        String::from_utf8_lossy(&new).into_owned(),
                    );
                }
            }
            let mut syntax = Syntax::new(view, source_id)?;
            for import in source.imports()?.iter().flatten() {
                let symbol = c.get_symbol_at_location(*import)?;
                let mut ambient = false;
                if let Some(symbol) = symbol {
                    for decl in h::declarations(c, symbol)? {
                        let v = self.view(decl)?;
                        if v.node(decl)?.kind() == K::ModuleDeclaration
                            && v.node(decl)?.name().is_some_and(|n| {
                                v.node(n).is_ok_and(|n| n.kind() == K::StringLiteral)
                            })
                        {
                            ambient = true;
                            break;
                        }
                    }
                }
                if ambient {
                    continue;
                }
                let old_spec = view.node_text(*import)?.into_js_string();
                let resolved = c.resolved_import_file(source_id, *import)?;
                let mut new_spec = None;
                if let Some(target) = resolved {
                    let new_target = updater.update(target.as_bytes());
                    if new_target.is_none()
                        && !(updated.is_some()
                            && path::is_external_module_name_relative(old_spec.as_bytes()))
                    {
                        continue;
                    }
                    new_spec = Some(
                        c.update_module_specifier(
                            source_id,
                            new_from,
                            *import,
                            new_target.as_deref().unwrap_or(target.as_bytes()),
                            specifier_preferences,
                        )?
                        .as_bytes()
                        .to_vec(),
                    );
                } else {
                    for (candidate, new_name) in &moved {
                        let old_name = self.source(*candidate)?;
                        let old = c.update_module_specifier(
                            source_id,
                            new_from,
                            *import,
                            old_name.file_name(),
                            specifier_preferences,
                        )?;
                        if old != old_spec {
                            continue;
                        }
                        let updated = c.update_module_specifier(
                            source_id,
                            new_from,
                            *import,
                            new_name,
                            specifier_preferences,
                        )?;
                        if !updated.as_bytes().is_empty() && updated != old_spec {
                            new_spec = Some(updated.as_bytes().to_vec());
                        }
                        break;
                    }
                    if new_spec.is_none()
                        && path::is_external_module_name_relative(old_spec.as_bytes())
                    {
                        new_spec = Some(updater.update_relative(
                            source.file_name(),
                            new_from,
                            old_spec.as_bytes(),
                        ));
                    }
                }
                if let Some(new) =
                    new_spec.filter(|new| !new.is_empty() && new != old_spec.as_bytes())
                {
                    tracker.replace_text(
                        source_id,
                        TextRange::new(
                            syntax.start(*import)? + 1,
                            i64::from(view.node(*import)?.end()) - 1,
                        ),
                        String::from_utf8_lossy(&new).into_owned(),
                    );
                }
            }
        }
        Ok(())
    }
    // port: tsc/internal/ls/file_rename.go:LanguageService.updateTsconfigFiles
    fn update_config_paths(
        &self,
        tracker: &mut NodeTracker<'_>,
        updater: &PathUpdater<'_>,
    ) -> Result<()> {
        let command = self.program.config();
        let Some(config) = &command.config_file else {
            return Ok(());
        };
        let Some(object) = config.object() else {
            return Ok(());
        };
        let view = config.file.view();
        let mut syntax = Syntax::new(view, config.root)?;
        let dir = path::directory(syntax.file.file_name());
        for (name, value) in properties(view, object)? {
            match name.as_slice() {
                b"files" | b"include" | b"exclude" => {
                    let mut found = false;
                    for element in path_elements(view, value)? {
                        found |= update_config_string(
                            &mut syntax,
                            element,
                            &dir,
                            &mut tracker.raw,
                            updater,
                        )?;
                    }
                    if found
                        || name != b"include"
                        || view.node(value)?.kind() != K::ArrayLiteralExpression
                    {
                        continue;
                    }
                    let (old_spec, default) = command.matched_include_spec(updater.old);
                    if !old_spec.is_empty()
                        && !default
                        && command.matched_include_spec(updater.new).0.is_empty()
                    {
                        if let Some(&last) = path_elements(view, value)?.last() {
                            insert_config_path(
                                &mut syntax,
                                last,
                                &updater.relative(&dir, updater.new),
                                tracker,
                            )?;
                        }
                    }
                }
                b"compilerOptions" if view.node(value)?.kind() == K::ObjectLiteralExpression => {
                    for (name, value) in properties(view, value)? {
                        if tsr_tsoptions::option_declaration(&name, false).is_some_and(|o| {
                            o.is_file_path || o.element.is_some_and(|e| e.is_file_path)
                        }) {
                            for element in path_elements(view, value)? {
                                update_config_string(
                                    &mut syntax,
                                    element,
                                    &dir,
                                    &mut tracker.raw,
                                    updater,
                                )?;
                            }
                        } else if name == b"paths"
                            && view.node(value)?.kind() == K::ObjectLiteralExpression
                        {
                            for (_, value) in properties(view, value)? {
                                if view.node(value)?.kind() == K::ArrayLiteralExpression {
                                    for element in path_elements(view, value)? {
                                        update_config_string(
                                            &mut syntax,
                                            element,
                                            &dir,
                                            &mut tracker.raw,
                                            updater,
                                        )?;
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}
fn path_elements(view: AstView<'_>, value: NodeId) -> Result<Vec<NodeId>> {
    if view.node(value)?.kind() == K::ArrayLiteralExpression {
        Ok(view
            .node_slice(view.node(value)?.elements(view)?)?
            .iter()
            .flatten()
            .collect())
    } else {
        Ok(vec![value])
    }
}
// port: tsc/internal/ls/file_rename.go:forEachObjectProperty
fn properties(view: AstView<'_>, object: NodeId) -> Result<Vec<(Vec<u8>, NodeId)>> {
    let mut result = Vec::new();
    for property in view
        .node_slice(view.node(object)?.properties(view)?)?
        .iter()
        .flatten()
    {
        let read = view.node(property)?;
        if read.kind() != K::PropertyAssignment {
            continue;
        }
        let (Some(name), Some(value)) = (read.name(), read.initializer()) else {
            continue;
        };
        if let Some(name) = tsr_ast::utilities_targets::try_get_text_of_property_name(view, name)? {
            result.push((name, value));
        }
    }
    Ok(result)
}
// port: tsc/internal/ls/file_rename.go:tryUpdateConfigString
fn update_config_string(
    syntax: &mut Syntax<'_>,
    element: NodeId,
    dir: &[u8],
    tracker: &mut Tracker,
    updater: &PathUpdater<'_>,
) -> Result<bool> {
    let node = syntax.view.node(element)?;
    if node.kind() != K::StringLiteral {
        return Ok(false);
    }
    let combined = path::combine(dir, &[syntax.view.node_text(element)?.as_bytes()]);
    let file = path::normalize(&combined);
    let Some(new) = updater.update(&file) else {
        return Ok(false);
    };
    tracker.replace_text(
        syntax.source,
        TextRange::new(syntax.start(element)? + 1, i64::from(node.end()) - 1),
        String::from_utf8_lossy(&updater.relative(dir, &new)).into_owned(),
    );
    Ok(true)
}
fn insert_config_path(
    syntax: &mut Syntax<'_>,
    last: NodeId,
    path: &[u8],
    tracker: &mut NodeTracker<'_>,
) -> Result<()> {
    let node = tracker
        .ast
        .new_string_literal(tsr_ast::JsString::from_bytes(path), 0);
    tracker.insert_after(syntax.source, last, node)
}

#[cfg(test)]
mod tests {
    use super::*;
    // source: tsc/internal/ls/file_rename_test.go:TestCreatePathUpdaterCaseFoldingShrinksOldPath
    #[test]
    #[allow(
        clippy::unicode_not_nfc,
        reason = "The native regression requires Kelvin signs whose byte width shrinks under case folding"
    )]
    fn case_folding_shrinks_the_directory_prefix() {
        let updater = PathUpdater {
            old: "/a/KKKK".as_bytes(),
            new: b"/a/new",
            case_sensitive: false,
        };
        assert_eq!(
            updater.update(b"/a/kkkk/x.ts"),
            Some(b"/a/new/x.ts".to_vec())
        );
    }
}
