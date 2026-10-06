//! Module and reference-path completion, using the program's retained filesystem.
use crate::{completion_items, syntax::Syntax, CompletionOptions, LanguageService, Result};
use std::collections::{BTreeMap, HashSet};
use tsr_ast::{span_map::FEATURE_COMPLETION, NodeId, SyntaxKind as K};
use tsr_checker::{ModuleSpecifierEnding as Ending, Operation};
use tsr_core::{ResolutionMode, TextRange};
use tsr_lsproto as lsp;
use tsr_tspath as path;
#[path = "completion_mappings.rs"]
mod mappings;
use mappings::MappingKind;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Directory,
    File,
    Module,
}
struct Entry {
    name: Vec<u8>,
    kind: Kind,
    extension: Vec<u8>,
}
#[derive(Default)]
struct Entries(BTreeMap<Vec<u8>, Entry>);
impl Entries {
    // port: tsc/internal/ls/string_completions.go:moduleCompletionNameAndKindSet.add
    fn add(&mut self, name: Vec<u8>, kind: Kind, extension: Vec<u8>) {
        if self.0.get(&name).is_none_or(|e| e.kind < kind) {
            self.0.insert(
                name.clone(),
                Entry {
                    name,
                    kind,
                    extension,
                },
            );
        }
    }
}
#[derive(Clone)]
struct PathOptions {
    extensions: Vec<Vec<u8>>,
    endings: Vec<Ending>,
    reference: bool,
}

pub(crate) fn module_literal(syntax: &Syntax<'_>, literal: NodeId) -> Result<bool> {
    let mut parent = syntax.view.node(literal)?.parent();
    while let Some(id) = parent {
        let read = syntax.view.node(id)?;
        match read.kind().known() {
            Some(K::ParenthesizedExpression | K::ParenthesizedType | K::LiteralType) => {
                parent = read.parent();
            }
            Some(
                K::ImportType
                | K::ImportDeclaration
                | K::ExportDeclaration
                | K::ExternalModuleReference
                | K::JSDocImportTag,
            ) => return Ok(true),
            Some(K::CallExpression) => {
                let Some(expression) = read.expression() else {
                    return Ok(false);
                };
                let expression = syntax.view.node(expression)?;
                return Ok(expression.kind() == K::ImportKeyword
                    || expression.kind() == K::Identifier
                        && syntax.view.node_text(expression.id())?.as_bytes() == b"require");
            }
            _ => return Ok(false),
        }
    }
    Ok(false)
}
// port: tsc/internal/ls/string_completions.go:getDirectoryFragmentRange
fn fragment_range(text: &[u8], start: i64) -> Option<TextRange> {
    let offset = text
        .iter()
        .rposition(|c| matches!(c, b'/' | b'\\'))
        .map_or(0, |p| p + 1);
    (offset < text.len()).then(|| TextRange::new(start + offset as i64, start + text.len() as i64))
}
fn directory_fragment(text: &[u8]) -> Vec<u8> {
    if text.ends_with(b"/") {
        text.to_vec()
    } else {
        path::directory(text)
    }
}
fn relative(path: &[u8]) -> bool {
    path.starts_with(b"./") || path.starts_with(b"../")
}

impl LanguageService<'_> {
    /// The pin's Snapshot.ReadDirectory/GetDirectories read the underlying host,
    /// not SnapshotFS.GetAccessibleEntries' retained file inventory. This host is
    /// used only for completion discovery; semantic reads still use the program.
    pub fn set_completion_file_system(&mut self, host: std::sync::Arc<dyn tsr_vfs::FileSystem>) {
        self.completion_host = Some(host);
    }
    fn completion_file_system(&self) -> &dyn tsr_vfs::FileSystem {
        self.completion_host
            .as_deref()
            .unwrap_or_else(|| self.program.host())
    }

    fn path_options(
        &self,
        checker: &mut Operation<'_>,
        syntax: &Syntax<'_>,
        mode: ResolutionMode,
        reference: bool,
        options: &CompletionOptions,
    ) -> Result<PathOptions> {
        let compiler = self.program.options();
        let mut extensions = Vec::new();
        if !reference {
            for symbol in checker.get_ambient_modules()? {
                let name = checker.symbol(symbol)?.name_bytes();
                let name = name
                    .strip_prefix(b"\"")
                    .and_then(|s| s.strip_suffix(b"\""))
                    .unwrap_or(name);
                if name.starts_with(b"*.") && !name.contains(&b'/') {
                    extensions.push(name[1..].to_vec());
                }
            }
        }
        extensions.extend(
            tsr_tsoptions::supported_extensions(
                compiler,
                &self.program.content_mapper_extensions(),
            )
            .into_iter()
            .flatten()
            .map(|s| s.as_bytes().to_vec()),
        );
        if node_modules_mode(compiler) && compiler.resolve_json_module() {
            extensions.push(b".json".to_vec());
        }
        Ok(PathOptions {
            extensions,
            endings: checker.import_ending_preferences(
                syntax.source,
                mode,
                options.auto_import.ending.as_deref(),
            )?,
            reference,
        })
    }
    // port: tsc/internal/ls/string_completions.go:LanguageService.getStringLiteralCompletionsFromModuleNames
    pub(crate) fn module_path_completions(
        &mut self,
        checker: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        literal: NodeId,
        position: i64,
        options: &CompletionOptions,
    ) -> Result<lsp::CompletionList> {
        let text = syntax.view.node_text(literal)?.into_js_string();
        let replacement = fragment_range(text.as_bytes(), syntax.start(literal)? + 1);
        let fragment = path::normalize_slashes(text.as_bytes());
        let mode = checker.import_usage_resolution_mode(syntax.source, literal)?;
        let path_options = self.path_options(checker, syntax, mode, false, options)?;
        let directory = path::directory(syntax.file.path());
        let mut entries = Entries::default();
        if relative(&fragment)
            || self
                .program
                .options()
                .paths
                .as_ref()
                .is_none_or(tsr_core::collections::OrderedMap::is_empty)
                && path::encoded_root_length(&fragment) != 0
        {
            let mut directories = root_directories(self.program, &directory);
            if directories.is_empty() {
                directories.push(directory);
            }
            for directory in directories {
                self.directory_path_entries(
                    &fragment,
                    &directory,
                    syntax.file.path(),
                    &path_options,
                    &mut entries,
                )?;
            }
        } else {
            self.nonrelative_path_entries(
                checker,
                &fragment,
                &directory,
                &path_options,
                mode,
                &mut entries,
            )?;
        }
        self.path_completion_list(syntax, entries, replacement, position, options)
    }
    fn directory_path_entries(
        &self,
        fragment: &[u8],
        base: &[u8],
        exclude: &[u8],
        options: &PathOptions,
        result: &mut Entries,
    ) -> Result<()> {
        self.check_canceled()?;
        let fragment = directory_fragment(fragment);
        let directory = path::resolve(base, &[if fragment.is_empty() { b"." } else { &fragment }]);
        // ResolvePath already returns the directory spelling used by the pin.
        // Removing its separator turns the filesystem root `/` into an empty path.
        let directory = directory.as_slice();
        let fs = self.completion_file_system();
        if !fs
            .directory_exists(directory)
            .map_err(tsr_compiler::Error::from)?
        {
            return Ok(());
        }
        let entries = fs.entries(directory).map_err(tsr_compiler::Error::from)?;
        // The pinned ReadDirectory groups matches by extension group. Preserve
        // the first file when a .ts and .js spelling collapse to one label.
        let mut files = entries.files.unwrap_or_default();
        files.sort_by_key(|name| {
            options
                .extensions
                .iter()
                .position(|ext| name.as_bytes().ends_with(ext))
                .unwrap_or(usize::MAX)
        });
        for name in files {
            // The pin's `./*` include pattern never matches a dot-file.
            if name.as_bytes().starts_with(b".")
                || !options
                    .extensions
                    .iter()
                    .any(|ext| name.as_bytes().ends_with(ext))
            {
                continue;
            }
            let file = path::resolve(directory, &[name.as_bytes()]);
            if path::compare_paths(
                &file,
                exclude,
                self.program.current_directory(),
                self.program.use_case_sensitive_file_names(),
            )
            .is_eq()
            {
                continue;
            }
            let (name, extension) = file_name(name.as_bytes(), self.program.options(), options);
            result.add(name, Kind::File, extension);
        }
        for name in entries.directories.into_iter().flatten() {
            if name.as_bytes() != b"@types" {
                result.add(name.as_bytes().to_vec(), Kind::Directory, Vec::new());
            }
        }
        Ok(())
    }
    fn nonrelative_path_entries(
        &self,
        checker: &mut Operation<'_>,
        fragment: &[u8],
        directory: &[u8],
        options: &PathOptions,
        mode: ResolutionMode,
        result: &mut Entries,
    ) -> Result<()> {
        let fragment_directory = directory_fragment(fragment);
        for symbol in checker.get_ambient_modules()? {
            let name = checker.symbol(symbol)?.name_bytes();
            let name = name
                .strip_prefix(b"\"")
                .and_then(|s| s.strip_suffix(b"\""))
                .unwrap_or(name);
            if name.contains(&b'*') || !name.starts_with(fragment) {
                continue;
            }
            if fragment_directory.is_empty() {
                result.add(name.to_vec(), Kind::Module, Vec::new());
            } else if let Some(name) = name
                .strip_prefix(fragment_directory.as_slice())
                .map(|s| s.strip_prefix(b"/").unwrap_or(s))
            {
                result.add(name.to_vec(), Kind::Module, Vec::new());
            }
        }
        self.typings_path_entries(&fragment_directory, directory, options, result)?;
        if let Some(paths) = &self.program.options().paths {
            self.mapping_entries(
                paths,
                fragment,
                self.program
                    .options()
                    .paths_base_path(self.program.current_directory()),
                options,
                MappingKind::Paths,
                result,
            )?;
        }
        if !node_modules_mode(self.program.options()) {
            return Ok(());
        }
        let mut found = false;
        if fragment_directory.is_empty() {
            let mut ancestor = directory.to_vec();
            loop {
                let json = path::resolve(&ancestor, &[b"package.json"]);
                if let Some(package) = checker.import_package_json(&json)? {
                    package.contents.range_dependencies(|name, _, _| {
                        if !name.starts_with("@types/") && !result.0.contains_key(name.as_bytes()) {
                            found = true;
                            result.add(name.as_bytes().to_vec(), Kind::Module, Vec::new());
                        }
                        true
                    });
                }
                let parent = path::directory(&ancestor);
                if parent == ancestor {
                    break;
                }
                ancestor = parent;
            }
        }
        if !found {
            self.package_path_entries(checker, fragment, directory, options, mode, result)?;
        }
        Ok(())
    }
    fn typings_path_entries(
        &self,
        fragment: &[u8],
        directory: &[u8],
        options: &PathOptions,
        result: &mut Entries,
    ) -> Result<()> {
        let mut roots: Vec<Vec<u8>> = self
            .program
            .options()
            .type_roots
            .iter()
            .flatten()
            .map(|r| path::resolve(self.program.current_directory(), &[r.as_bytes()]))
            .collect();
        let mut ancestor = directory.to_vec();
        loop {
            roots.push(path::resolve(&ancestor, &[b"node_modules/@types"]));
            let parent = path::directory(&ancestor);
            if parent == ancestor {
                break;
            }
            ancestor = parent;
        }
        let mut seen = HashSet::new();
        for root in roots {
            if !self
                .completion_file_system()
                .directory_exists(&root)
                .map_err(tsr_compiler::Error::from)?
            {
                continue;
            }
            for name in self
                .completion_file_system()
                .entries(&root)
                .map_err(tsr_compiler::Error::from)?
                .directories
                .into_iter()
                .flatten()
            {
                let package = tsr_module::unmangle_scoped_package_name(name.as_bytes());
                if self.program.options().types.as_ref().is_some_and(|types| {
                    !types.is_empty() && !types.iter().any(|t| t.as_bytes() == package)
                }) {
                    continue;
                }
                if fragment.is_empty() {
                    if seen.insert(package.clone()) {
                        result.add(package, Kind::Module, Vec::new());
                    }
                } else if let Some(rest) = fragment.strip_prefix(package.as_slice()) {
                    let remaining = rest.strip_prefix(b"/").unwrap_or(rest);
                    self.directory_path_entries(
                        &[remaining, b"/"].concat(),
                        &path::resolve(&root, &[name.as_bytes()]),
                        b"",
                        options,
                        result,
                    )?;
                }
            }
        }
        Ok(())
    }
    // port: tsc/internal/ls/string_completions.go:LanguageService.convertPathCompletions
    fn path_completion_list(
        &mut self,
        syntax: &Syntax<'_>,
        entries: Entries,
        replacement: Option<TextRange>,
        position: i64,
        options: &CompletionOptions,
    ) -> Result<lsp::CompletionList> {
        let mut list = lsp::CompletionList::default();
        let replacement = replacement
            .map(|range| self.range(syntax.source, range, FEATURE_COMPLETION))
            .transpose()?
            .filter(|(_, f)| f.is_exact())
            .map(|(r, _)| r);
        for (_, entry) in entries.0 {
            let name = String::from_utf8_lossy(&entry.name).into_owned();
            let mut detail = name.clone();
            if !entry.name.ends_with(&entry.extension) {
                detail.push_str(&String::from_utf8_lossy(&entry.extension));
            }
            list.items.push(Some(Box::new(lsp::CompletionItem {
                label: name.clone(),
                kind: Some(Box::new(match entry.kind {
                    Kind::Directory => lsp::CompletionItemKind::FOLDER,
                    Kind::File => lsp::CompletionItemKind::FILE,
                    Kind::Module => lsp::CompletionItemKind::MODULE,
                })),
                sort_text: Some(Box::new("11".into())),
                detail: Some(Box::new(detail)),
                text_edit: replacement.as_ref().map(|range| {
                    Box::new(lsp::TextEditOrInsertReplaceEdit {
                        text_edit: Some(Box::new(lsp::TextEdit {
                            range: range.clone(),
                            new_text: name,
                        })),
                        ..Default::default()
                    })
                }),
                ..Default::default()
            })));
        }
        let (cursor, _) = self.range(
            syntax.source,
            TextRange::new(position, position),
            FEATURE_COMPLETION,
        )?;
        completion_items::defaults(&mut list, options, &cursor.start, None, &[]);
        self.completion_data(syntax.source, position, &mut list)?;
        Ok(list)
    }
}
// port: tsc/internal/ls/string_completions.go:getFilenameWithExtensionOption
fn file_name(
    name: &[u8],
    compiler: &tsr_core::CompilerOptions,
    options: &PathOptions,
) -> (Vec<u8>, Vec<u8>) {
    if let Some(stem) = name.strip_suffix(b".ts") {
        if let Some(pos) = stem.windows(3).rposition(|w| w == b".d.") {
            let name = [&stem[..pos], &stem[pos + 2..]].concat();
            let extension =
                name[name.iter().rposition(|c| *c == b'.').unwrap_or(name.len())..].to_vec();
            return (name, extension);
        }
    }
    let extension = path::try_get_extension_from_path(name).to_vec();
    if options.reference {
        return (name.to_vec(), extension);
    }
    if options.endings.first() == Some(&Ending::Ts)
        && matches!(extension.as_slice(), b".ts" | b".tsx" | b".mts" | b".cts")
    {
        return (name.to_vec(), extension);
    }
    if matches!(
        options.endings.first(),
        Some(Ending::Minimal | Ending::Index)
    ) && matches!(
        extension.as_slice(),
        b".js" | b".jsx" | b".ts" | b".tsx" | b".d.ts"
    ) {
        return (path::remove_file_extension(name).to_vec(), extension);
    }
    let js = tsr_module::js_extension_for_file(name, compiler);
    if js.is_empty() {
        (name.to_vec(), extension)
    } else {
        (path::change_extension(name, js), js.to_vec())
    }
}
// port: tsc/internal/ls/string_completions.go:getBaseDirectoriesFromRootDirs
fn root_directories(program: &tsr_compiler::Program, directory: &[u8]) -> Vec<Vec<u8>> {
    let options = program.options();
    let Some(roots) = &options.root_dirs else {
        return Vec::new();
    };
    let base = if options.project.is_empty() {
        program.current_directory()
    } else {
        options.project.as_bytes()
    };
    let roots: Vec<_> = roots
        .iter()
        .map(|r| path::resolve(base, &[r.as_bytes()]))
        .collect();
    let relative = roots
        .iter()
        .find(|r| path::contains_path(r, directory, base, program.use_case_sensitive_file_names()))
        .map_or(&b""[..], |r| {
            directory
                .get(r.len()..)
                .unwrap_or_default()
                .strip_prefix(b"/")
                .unwrap_or_else(|| directory.get(r.len()..).unwrap_or_default())
        });
    let mut result = Vec::new();
    for root in roots
        .iter()
        .map(|r| path::resolve(r, &[relative]))
        .chain(std::iter::once(directory.to_vec()))
    {
        if !result.contains(&root) {
            result.push(root);
        }
    }
    result
}

fn trim_left(mut text: &[u8]) -> &[u8] {
    while !text.is_empty() {
        let (ch, n) = tsr_jsstring::wtf8::decode_utf8(text);
        if !tsr_jsstring::classify::is_white_space_like(ch) {
            break;
        }
        text = &text[n..];
    }
    text
}
// port: tsc/internal/ls/string_completions.go:parseTripleSlashDirectiveFragment
fn triple_slash_fragment(text: &[u8]) -> Option<(usize, bool, &[u8])> {
    let mut rest = trim_left(text.strip_prefix(b"///")?);
    rest = rest.strip_prefix(b"<reference")?;
    if rest
        .first()
        .is_none_or(|b| !tsr_jsstring::classify::is_white_space_like(i32::from(*b)))
    {
        return None;
    }
    rest = trim_left(rest);
    let reference = if let Some(t) = rest.strip_prefix(b"path") {
        rest = t;
        true
    } else {
        rest = rest.strip_prefix(b"types")?;
        false
    };
    rest = trim_left(rest).strip_prefix(b"=")?;
    rest = trim_left(rest);
    if !matches!(rest.first(), Some(b'\'' | b'"')) {
        return None;
    }
    rest = &rest[1..];
    if rest.iter().any(|b| matches!(b, b'\'' | b'"')) {
        return None;
    }
    Some((text.len() - rest.len(), reference, rest))
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/string_completions.go:LanguageService.getTripleSlashReferenceCompletions
    pub(crate) fn reference_path_completions(
        &mut self,
        checker: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        position: i64,
        options: &CompletionOptions,
    ) -> Result<Option<lsp::CompletionList>> {
        let token = syntax.nav().get_token_at_position(position)?;
        let mut ranges = tsr_scanner::get_leading_comment_ranges(
            syntax.file.text().as_bytes(),
            i64::from(syntax.view.node(token)?.pos()),
        );
        let Some(range) = ranges.find(|r| r.loc.pos() <= position && position <= r.loc.end())
        else {
            return Ok(None);
        };
        let text = &syntax.file.text().as_bytes()[range.loc.pos() as usize..position as usize];
        let Some((prefix, reference, fragment)) = triple_slash_fragment(text) else {
            return Ok(None);
        };
        let replacement = fragment_range(fragment, range.loc.pos() + prefix as i64);
        let path_options =
            self.path_options(checker, syntax, ResolutionMode::NONE, reference, options)?;
        let mut entries = Entries::default();
        let directory = path::directory(syntax.file.path());
        if reference {
            self.directory_path_entries(
                fragment,
                &directory,
                syntax.file.path(),
                &path_options,
                &mut entries,
            )?;
        } else {
            self.typings_path_entries(
                &directory_fragment(fragment),
                &directory,
                &path_options,
                &mut entries,
            )?;
        }
        self.path_completion_list(syntax, entries, replacement, position, options)
            .map(Some)
    }
}
fn node_modules_mode(options: &tsr_core::CompilerOptions) -> bool {
    let kind = options.module_resolution_kind();
    kind >= tsr_core::ModuleResolutionKind::NODE16
        && kind <= tsr_core::ModuleResolutionKind::NODE_NEXT
        || kind == tsr_core::ModuleResolutionKind::BUNDLER
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reference_fragment_rejects_closed_quotes_and_preserves_offsets() {
        let text = b"/// <reference path=\"./folder/na";
        assert_eq!(
            triple_slash_fragment(text),
            Some((21, true, &b"./folder/na"[..]))
        );
        for text in [
            &b"// <reference path=\"x"[..],
            b"/// <referencepath=\"x",
            b"/// <reference lib=\"x",
            b"/// <reference types=\"x\"",
            b"/// <reference path='x\"y",
        ] {
            assert!(triple_slash_fragment(text).is_none());
        }
        assert_eq!(
            fragment_range(b"./dir/name", 10),
            Some(TextRange::new(16, 20))
        );
        assert_eq!(fragment_range(b"./dir/", 10), None);
    }
}
