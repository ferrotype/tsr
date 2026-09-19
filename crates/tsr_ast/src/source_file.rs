//! Source-file metadata lives below the same retention root as its nodes.
//! A factory may construct several logical files; services take their node ID.

use crate::{
    AstBuilder, AstStorageData, AstView, Diagnostic, Factory, JsString, NodeData, NodeId,
    NodeListId, SourceFileData, SyntaxKind,
};
use crate::{
    CommentSlice, CommentSliceRead, DiagnosticDirectiveSlice, DiagnosticDirectiveSliceRead,
    PragmaSlice, PragmaSliceRead, ReferenceSlice, ReferenceSliceRead, SourceNodeSlice,
    SourceNodeSliceRead, SourceTextSlice, SourceTextSliceRead,
};
use std::{collections::BTreeMap, ops::Deref, sync::OnceLock};
use ts_arena::Error;
use ts_core::{LanguageVariant, ScriptKind, TextRange, Tristate};
use ts_jsstring::{PositionMap, SourceText};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceFileParseOptions {
    pub file_name: JsString,
    pub path: JsString,
    pub external_module_indicator_options: ExternalModuleIndicatorOptions,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExternalModuleIndicatorOptions {
    pub jsx: bool,
    pub force: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommentRange {
    pub loc: TextRange,
    pub kind: crate::NodeKind,
    pub has_trailing_new_line: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileReference {
    pub loc: TextRange,
    pub file_name: JsString,
    pub resolution_mode: i64,
    pub preserve: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PragmaArgument {
    pub loc: TextRange,
    pub name: JsString,
    pub value: JsString,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pragma {
    pub range: CommentRange,
    pub name: JsString,
    pub args: BTreeMap<JsString, PragmaArgument>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CheckJsDirective {
    pub enabled: bool,
    pub range: CommentRange,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceHash {
    pub hi: u64,
    pub lo: u64,
}

/// Wire-facing span records. Mapping algorithms and validation are a later
/// spanmap slice; S06 serializes explicitly supplied pinned fixture metadata.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpanSegment {
    pub virtual_start: i32,
    pub virtual_end: i32,
    pub original_start: i32,
    pub original_end: i32,
    pub kind: i32,
    pub features: i32,
}
impl SpanSegment {
    pub const FEATURE_ALL: i32 = (1 << 20) - 1;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MappedDiagnosticDirective {
    pub original_range: TextRange,
    pub virtual_range: TextRange,
    pub policy: u8,
    pub unused_code: i32,
    pub unused_message_text: JsString,
    pub source: JsString,
}

#[derive(Clone, Debug, Default)]
pub struct ContentMapperSourceFileInfo {
    pub content_mapper: JsString,
    pub transform_identity: JsString,
    pub parse_options: SourceFileParseOptions,
    pub virtual_file_name: JsString,
    pub original_text: SourceText,
    pub span_map: Option<std::sync::Arc<[SpanSegment]>>,
    pub diagnostic_directives: DiagnosticDirectiveSlice,
    pub supplemental_source_files: SourceNodeSlice,
    pub canonical_source_file: Option<NodeId>,
}

/// The mutable parser frame becomes immutable with AST publication. Derived
/// maps are initialized only on demand and are not copied by factory cloning.
#[derive(Debug)]
pub struct SourceFileState {
    parse_options: SourceFileParseOptions,
    text: SourceText,
    pub language_variant: LanguageVariant,
    pub script_kind: ScriptKind,
    pub is_declaration_file: bool,
    pub uses_uri_style_node_core_modules: Tristate,
    pub identifier_count: i64,
    pub imports: SourceNodeSlice,
    pub module_augmentations: SourceNodeSlice,
    pub ambient_module_names: SourceTextSlice,
    pub comment_directives: CommentSlice,
    pub has_lazy_jsdoc: bool,
    pub reparsed_clones: Vec<NodeId>,
    pub pragmas: PragmaSlice,
    pub referenced_files: ReferenceSlice,
    pub type_reference_directives: ReferenceSlice,
    pub lib_reference_directives: ReferenceSlice,
    pub check_js_directive: Option<CheckJsDirective>,
    pub node_count: i64,
    pub text_count: i64,
    common_js_module_indicator: Option<NodeId>,
    pub external_module_indicator: Option<NodeId>,
    pub diagnostics: Vec<Diagnostic>,
    pub js_diagnostics: Vec<Diagnostic>,
    pub jsdoc_diagnostics: Vec<Diagnostic>,
    pub hash: SourceHash,
    content_mapper_info: Option<ContentMapperSourceFileInfo>,
    position_map: OnceLock<PositionMap>,
    ecma_line_map: OnceLock<Vec<i32>>,
    node_index: crate::source_cache::SourceNodeIndexCache,
    pub(crate) binding: crate::bind_result::BindCell,
}

impl SourceFileState {
    pub fn common_js_module_indicator(&self) -> Option<NodeId> {
        self.common_js_module_indicator
    }

    pub fn new(parse_options: SourceFileParseOptions, text: SourceText) -> Self {
        Self {
            parse_options,
            text,
            language_variant: LanguageVariant::default(),
            script_kind: ScriptKind::UNKNOWN,
            is_declaration_file: false,
            uses_uri_style_node_core_modules: Tristate::UNKNOWN,
            identifier_count: 0,
            imports: SourceNodeSlice::empty(),
            module_augmentations: SourceNodeSlice::empty(),
            ambient_module_names: SourceTextSlice::empty(),
            comment_directives: CommentSlice::empty(),
            has_lazy_jsdoc: false,
            reparsed_clones: Vec::new(),
            pragmas: PragmaSlice::empty(),
            referenced_files: ReferenceSlice::empty(),
            type_reference_directives: ReferenceSlice::empty(),
            lib_reference_directives: ReferenceSlice::empty(),
            check_js_directive: None,
            node_count: 0,
            text_count: 0,
            common_js_module_indicator: None,
            external_module_indicator: None,
            diagnostics: Vec::new(),
            js_diagnostics: Vec::new(),
            jsdoc_diagnostics: Vec::new(),
            hash: SourceHash::default(),
            content_mapper_info: None,
            position_map: OnceLock::new(),
            ecma_line_map: OnceLock::new(),
            node_index: crate::source_cache::SourceNodeIndexCache::default(),
            binding: crate::bind_result::BindCell::default(),
        }
    }

    /// port: tsc/internal/ast/ast.go:SourceFile.FileName
    pub fn file_name(&self) -> &[u8] {
        self.parse_options.file_name.as_bytes()
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.Path
    pub fn path(&self) -> &[u8] {
        self.parse_options.path.as_bytes()
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.GetPositionMap
    pub fn position_map(&self) -> &PositionMap {
        self.position_map
            .get_or_init(|| PositionMap::new(self.text.as_bytes()))
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.ECMALineMap
    pub fn ecma_line_map(&self) -> &[i32] {
        self.ecma_line_map
            .get_or_init(|| ts_jsstring::line_map::compute_ecma_line_starts(self.text.as_bytes()))
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.ParseOptions
    pub fn parse_options(&self) -> &SourceFileParseOptions {
        &self.parse_options
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.Text
    pub fn text(&self) -> &SourceText {
        &self.text
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.IsJS
    pub fn is_js(&self) -> bool {
        matches!(self.script_kind, ScriptKind::JS | ScriptKind::JSX)
    }

    pub fn content_mapper_info(&self) -> Option<&ContentMapperSourceFileInfo> {
        self.content_mapper_info.as_ref()
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.SetContentMapperInfo
    pub fn set_content_mapper_info(&mut self, info: ContentMapperSourceFileInfo) {
        assert!(
            self.content_mapper_info.is_none(),
            "content mapper source file info already set"
        );
        self.content_mapper_info = Some(info);
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.ContentMapper
    pub fn content_mapper(&self) -> &[u8] {
        self.content_mapper_info
            .as_ref()
            .map_or(&[], |info| info.content_mapper.as_bytes())
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.OriginalText
    pub fn original_text(&self) -> &[u8] {
        if self.content_mapper().is_empty() {
            self.text.as_bytes()
        } else {
            self.content_mapper_info
                .as_ref()
                .expect("nonempty mapper has metadata")
                .original_text
                .as_bytes()
        }
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.SpanMap
    pub fn span_map(&self) -> Option<&[SpanSegment]> {
        self.content_mapper_info
            .as_ref()
            .and_then(|info| info.span_map.as_deref())
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.IsContentMapperFailureStub
    pub fn is_content_mapper_failure_stub(&self) -> bool {
        !self.content_mapper().is_empty() && self.span_map().is_none()
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.VirtualFileName
    pub fn virtual_file_name(&self) -> &[u8] {
        self.content_mapper_info
            .as_ref()
            .map_or(&[], |info| info.virtual_file_name.as_bytes())
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.CanonicalSourceFile
    pub fn canonical_source_file(&self) -> Option<NodeId> {
        self.content_mapper_info
            .as_ref()
            .and_then(|info| info.canonical_source_file)
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.ContentMapperTransformIdentity
    pub fn content_mapper_transform_identity(&self) -> &[u8] {
        self.content_mapper_info
            .as_ref()
            .map_or(&[], |info| info.transform_identity.as_bytes())
    }
    /// A borrowed packet preserves the source value without reference-count work.
    /// port: tsc/internal/ast/ast.go:SourceFile.ContentMapperParseOptions
    pub fn content_mapper_parse_options(&self) -> std::borrow::Cow<'_, SourceFileParseOptions> {
        self.content_mapper_info.as_ref().map_or_else(
            || std::borrow::Cow::Owned(SourceFileParseOptions::default()),
            |info| std::borrow::Cow::Borrowed(&info.parse_options),
        )
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.IsContentMapperSupplemental
    pub fn is_content_mapper_supplemental(&self) -> bool {
        self.canonical_source_file().is_some()
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.Diagnostics
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.SetDiagnostics
    pub fn set_diagnostics(&mut self, diagnostics: Vec<Diagnostic>) {
        self.diagnostics = diagnostics;
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.JSDiagnostics
    pub fn js_diagnostics(&self) -> &[Diagnostic] {
        &self.js_diagnostics
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.SetJSDiagnostics
    pub fn set_js_diagnostics(&mut self, diagnostics: Vec<Diagnostic>) {
        self.js_diagnostics = diagnostics;
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.JSDocDiagnostics
    pub fn jsdoc_diagnostics(&self) -> &[Diagnostic] {
        &self.jsdoc_diagnostics
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.SetJSDocDiagnostics
    pub fn set_jsdoc_diagnostics(&mut self, diagnostics: Vec<Diagnostic>) {
        self.jsdoc_diagnostics = diagnostics;
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.SetHasLazyJSDoc
    pub fn set_has_lazy_jsdoc(&mut self, lazy: bool) {
        self.has_lazy_jsdoc = lazy;
    }
    pub(crate) fn validate_references(&self, view: AstView<'_>) -> Result<(), Error> {
        for slice in [
            self.imports,
            self.module_augmentations,
            self.content_mapper_info
                .as_ref()
                .map_or(SourceNodeSlice::empty(), |info| {
                    info.supplemental_source_files
                }),
        ] {
            for &id in view.source_nodes(slice)?.iter().flatten() {
                view.node(id)?;
            }
        }
        view.source_strings(self.ambient_module_names)?;
        view.source_comments(self.comment_directives)?;
        view.source_pragmas(self.pragmas)?;
        for slice in [
            self.referenced_files,
            self.type_reference_directives,
            self.lib_reference_directives,
        ] {
            view.source_references(slice)?;
        }
        if let Some(info) = &self.content_mapper_info {
            view.source_diagnostic_directives(info.diagnostic_directives)?;
        }
        for &id in self
            .reparsed_clones
            .iter()
            .chain(self.common_js_module_indicator.iter())
            .chain(self.external_module_indicator.iter())
            .chain(self.canonical_source_file().iter())
        {
            view.node(id)?;
        }
        self.node_index.validate_references(view)?;
        let mut diagnostics: Vec<&Diagnostic> = self
            .diagnostics
            .iter()
            .chain(&self.js_diagnostics)
            .chain(&self.jsdoc_diagnostics)
            .collect();
        while let Some(diagnostic) = diagnostics.pop() {
            if let Some(file) = diagnostic.file {
                view.node(file)?;
            }
            diagnostics.extend(diagnostic.message_chain.iter().map(AsRef::as_ref));
            diagnostics.extend(diagnostic.related_information.iter().map(AsRef::as_ref));
        }
        Ok(())
    }

    /// `SourceFile.copyFrom` deliberately omits counts, diagnostics, checkJs,
    /// lazy JSDoc, reparsed clones, hash and caches. This is not Rust Clone.
    fn factory_copy(&self) -> SourceFileCopy {
        SourceFileCopy {
            content_mapper_info: self.content_mapper_info.clone(),
            language_variant: self.language_variant,
            script_kind: self.script_kind,
            is_declaration_file: self.is_declaration_file,
            uses_uri_style_node_core_modules: self.uses_uri_style_node_core_modules,
            imports: self.imports,
            module_augmentations: self.module_augmentations,
            ambient_module_names: self.ambient_module_names,
            comment_directives: self.comment_directives,
            pragmas: self.pragmas,
            referenced_files: self.referenced_files,
            type_reference_directives: self.type_reference_directives,
            lib_reference_directives: self.lib_reference_directives,
            common_js_module_indicator: self.common_js_module_indicator,
            external_module_indicator: self.external_module_indicator,
        }
    }

    fn copy_parser_fields(&mut self, source: SourceFileCopy) {
        if let Some(info) = source.content_mapper_info {
            self.set_content_mapper_info(info);
        }
        self.language_variant = source.language_variant;
        self.script_kind = source.script_kind;
        self.is_declaration_file = source.is_declaration_file;
        self.uses_uri_style_node_core_modules = source.uses_uri_style_node_core_modules;
        self.imports = source.imports;
        self.module_augmentations = source.module_augmentations;
        self.ambient_module_names = source.ambient_module_names;
        self.comment_directives = source.comment_directives;
        self.pragmas = source.pragmas;
        self.referenced_files = source.referenced_files;
        self.type_reference_directives = source.type_reference_directives;
        self.lib_reference_directives = source.lib_reference_directives;
        self.common_js_module_indicator = source.common_js_module_indicator;
        self.external_module_indicator = source.external_module_indicator;
    }
}

/// A single staging move releases the original metadata borrow before mutation.
/// It contains only the fields selected by Go's copyFrom, not a second file.
struct SourceFileCopy {
    content_mapper_info: Option<ContentMapperSourceFileInfo>,
    language_variant: LanguageVariant,
    script_kind: ScriptKind,
    is_declaration_file: bool,
    uses_uri_style_node_core_modules: Tristate,
    imports: SourceNodeSlice,
    module_augmentations: SourceNodeSlice,
    ambient_module_names: SourceTextSlice,
    comment_directives: CommentSlice,
    pragmas: PragmaSlice,
    referenced_files: ReferenceSlice,
    type_reference_directives: ReferenceSlice,
    lib_reference_directives: ReferenceSlice,
    common_js_module_indicator: Option<NodeId>,
    external_module_indicator: Option<NodeId>,
}

pub struct OriginalFileName<'a>(SourceFileRead<'a>);
impl OriginalFileName<'_> {
    pub fn as_bytes(&self) -> &[u8] {
        self.0.file_name()
    }
}
impl Deref for OriginalFileName<'_> {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        self.as_bytes()
    }
}

pub struct SourceFileRead<'a> {
    record: crate::auxiliary::AuxRead<'a>,
    node: NodeId,
    view: AstView<'a>,
}
impl Deref for SourceFileRead<'_> {
    type Target = SourceFileState;
    fn deref(&self) -> &Self::Target {
        match self.record.full() {
            Some(AstStorageData::SourceFiles(files)) => &files[&self.node],
            _ => unreachable!("validated source-file frame"),
        }
    }
}
impl<'a> SourceFileRead<'a> {
    /// Borrow immutable parse metadata for the complete retained view lifetime.
    pub fn parse_options(&self) -> &'a SourceFileParseOptions {
        self.state_ref().parse_options()
    }
    /// Borrow the file name without retaining another owner or copying bytes.
    pub fn file_name(&self) -> &'a [u8] {
        self.state_ref().parse_options().file_name.as_bytes()
    }
    pub(crate) fn state_ref(&self) -> &'a SourceFileState {
        match self
            .record
            .full_borrowed()
            .expect("source-file states are published core records")
        {
            AstStorageData::SourceFiles(files) => &files[&self.node],
            _ => unreachable!("validated source-file frame"),
        }
    }
    pub fn common_js_module_indicator(&self) -> Option<NodeId> {
        self.view
            .1
            .filter(|result| result.source() == self.node)
            .map_or(self.state_ref().common_js_module_indicator, |result| {
                result.common_js_module_indicator
            })
    }
    pub fn bind_diagnostics(&self) -> &[Diagnostic] {
        self.view
            .1
            .filter(|result| result.source() == self.node)
            .map_or(&[], |result| result.diagnostics())
    }

    fn factory_copy(&self) -> SourceFileCopy {
        let mut copy = self.state_ref().factory_copy();
        copy.common_js_module_indicator = self.common_js_module_indicator();
        copy
    }

    /// port: tsc/internal/ast/ast.go:SourceFile.OriginalFileName
    pub fn original_file_name(&self) -> Result<OriginalFileName<'a>, Error> {
        let id = self.canonical_source_file().unwrap_or(self.node);
        self.view.source_file(id).map(OriginalFileName)
    }

    /// port: tsc/internal/ast/ast.go:SourceFile.Imports
    pub fn imports(&self) -> Result<SourceNodeSliceRead<'a>, Error> {
        self.view.source_nodes(self.imports)
    }
    pub fn module_augmentations(&self) -> Result<SourceNodeSliceRead<'a>, Error> {
        self.view.source_nodes(self.module_augmentations)
    }
    pub fn ambient_module_names(&self) -> Result<SourceTextSliceRead<'a>, Error> {
        self.view.source_strings(self.ambient_module_names)
    }
    pub fn comment_directives(&self) -> Result<CommentSliceRead<'a>, Error> {
        self.view.source_comments(self.comment_directives)
    }
    pub fn pragmas(&self) -> Result<PragmaSliceRead<'a>, Error> {
        self.view.source_pragmas(self.pragmas)
    }
    pub fn referenced_files(&self) -> Result<ReferenceSliceRead<'a>, Error> {
        self.view.source_references(self.referenced_files)
    }
    pub fn type_reference_directives(&self) -> Result<ReferenceSliceRead<'a>, Error> {
        self.view.source_references(self.type_reference_directives)
    }
    pub fn lib_reference_directives(&self) -> Result<ReferenceSliceRead<'a>, Error> {
        self.view.source_references(self.lib_reference_directives)
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.SupplementalSourceFiles
    pub fn supplemental_source_files(&self) -> Result<SourceNodeSliceRead<'a>, Error> {
        self.view.source_nodes(
            self.content_mapper_info
                .as_ref()
                .map_or(SourceNodeSlice::empty(), |info| {
                    info.supplemental_source_files
                }),
        )
    }
    /// port: tsc/internal/ast/ast.go:SourceFile.DiagnosticDirectives
    pub fn diagnostic_directives(&self) -> Result<DiagnosticDirectiveSliceRead<'a>, Error> {
        self.view.source_diagnostic_directives(
            self.content_mapper_info
                .as_ref()
                .map_or(DiagnosticDirectiveSlice::empty(), |info| {
                    info.diagnostic_directives
                }),
        )
    }

    /// Install a derived table only after its IDs resolve under this source's
    /// checked retention root. No caller-supplied foreign view can widen it.
    pub fn try_node_index_cache(
        &self,
        build: impl FnOnce() -> Result<crate::NodeIndexCache, Error>,
    ) -> Result<Option<&std::sync::Arc<crate::NodeIndexCache>>, Error> {
        self.node_index.get_or_try_init(self.view, build)
    }

    pub fn node_index_cache(
        &self,
        build: impl FnOnce() -> crate::NodeIndexCache,
    ) -> Option<&std::sync::Arc<crate::NodeIndexCache>> {
        self.try_node_index_cache(|| Ok(build()))
            .expect("cache builder must retain every node under its source owner")
    }
}

impl std::fmt::Debug for SourceFileRead<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.deref().fmt(f)
    }
}

impl<'a> AstView<'a> {
    pub fn source_file(self, node: NodeId) -> Result<SourceFileRead<'a>, Error> {
        let owner = self.for_node_owner(node)?;
        let owner = AstView(owner.0.owner_retention()?, self.1);
        let map = owner.file_info().source_files.ok_or(Error::InvalidGraph)?;
        let record = owner.auxiliary(map)?;
        match record
            .full_borrowed()
            .expect("source maps are published core records")
        {
            AstStorageData::SourceFiles(files) if files.contains_key(&node) => {
                let result = self.1.and_then(|active| {
                    if active.source() == node {
                        Some(active)
                    } else {
                        files[&node].binding.result()
                    }
                });
                Ok(SourceFileRead {
                    record,
                    node,
                    view: AstView(owner.0, result),
                })
            }
            _ => Err(Error::InvalidGraph),
        }
    }
}

impl AstBuilder {
    pub fn source_file_mut(&mut self, node: NodeId) -> Result<&mut SourceFileState, Error> {
        self.view().node(node)?;
        let map = self
            .view()
            .file_info()
            .source_files
            .ok_or(Error::InvalidGraph)?;
        match self.auxiliary_mut(map)? {
            AstStorageData::SourceFiles(files) => files.get_mut(&node).ok_or(Error::InvalidGraph),
            _ => Err(Error::InvalidGraph),
        }
    }

    /// The text is already loaded parser text and is never BOM-decoded here.
    /// port: tsc/internal/ast/ast.go:NodeFactory.NewSourceFile
    pub fn new_source_file(
        &mut self,
        options: SourceFileParseOptions,
        text: SourceText,
        statements: Option<NodeListId>,
        end_of_file_token: Option<NodeId>,
    ) -> NodeId {
        let name = options.file_name.as_bytes();
        assert!(
            ts_core::path::encoded_root_length(name) != 0
                && name == ts_core::path::normalize(name).as_ref(),
            "fileName should be normalized and absolute: {}",
            ts_jsstring::go_quote(name)
        );
        let state = SourceFileState::new(options, text);
        self.new_source_file_with_state(state, statements, end_of_file_token)
    }

    fn new_source_file_with_state(
        &mut self,
        state: SourceFileState,
        statements: Option<NodeListId>,
        end_of_file_token: Option<NodeId>,
    ) -> NodeId {
        let map = if let Some(map) = self.view().file_info().source_files {
            map
        } else {
            let map = self.push_auxiliary(AstStorageData::SourceFiles(BTreeMap::new()));
            self.frame_mut().source_files = Some(map);
            map
        };
        // The SourceFile state must exist before OnCreate can inspect it. Install
        // below via the shared factory pre-hook operation rather than afterward.
        let node = self.new_node_before_hook(
            SyntaxKind::SourceFile.into(),
            NodeData::SourceFile(SourceFileData {
                statements,
                end_of_file_token,
            }),
        );
        match self
            .auxiliary_mut(map)
            .expect("source-file map is mutable core storage")
        {
            AstStorageData::SourceFiles(files) => {
                files.insert(node, state);
            }
            _ => unreachable!("source-file frame kind"),
        }
        self.run_create_hook(node);
        node
    }

    /// port: tsc/internal/ast/ast.go:SourceFile.Clone
    pub fn clone_source_file(&mut self, original: NodeId) -> NodeId {
        let (options, text, data) = {
            let view = self
                .factory_view(original)
                .expect("original source file belongs to factory");
            let node = view
                .node(original)
                .expect("original source file belongs to factory");
            let crate::NodeDataRead::SourceFile(data) = node.data() else {
                panic!("source-file payload required");
            };
            let state = view
                .source_file(original)
                .expect("source file has metadata");
            (
                state.parse_options.clone(),
                state.text.clone(),
                data.to_owned(),
            )
        };
        let updated = self.new_source_file(options, text, data.statements, data.end_of_file_token);
        let state = self
            .factory_view(original)
            .expect("original source file belongs to factory")
            .source_file(original)
            .expect("original metadata after OnCreate")
            .factory_copy();
        self.source_file_mut(updated)
            .expect("new source metadata")
            .copy_parser_fields(state);
        self.finish_clone(updated, original)
    }

    /// port: tsc/internal/ast/ast.go:NodeFactory.UpdateSourceFile
    pub fn update_source_file(
        &mut self,
        original: NodeId,
        statements: Option<NodeListId>,
        end_of_file_token: Option<NodeId>,
    ) -> NodeId {
        let (options, text) = {
            let view = self
                .factory_view(original)
                .expect("original source file belongs to factory");
            let node = view
                .node(original)
                .expect("original source file belongs to factory");
            let crate::NodeDataRead::SourceFile(data) = node.data() else {
                panic!("source-file payload required");
            };
            if statements == data.statements() && end_of_file_token == data.end_of_file_token() {
                return original;
            }
            let state = view
                .source_file(original)
                .expect("source file has metadata");
            (state.parse_options.clone(), state.text.clone())
        };
        let updated = self.new_source_file(options, text, statements, end_of_file_token);
        let state = self
            .factory_view(original)
            .expect("original source file belongs to factory")
            .source_file(original)
            .expect("original metadata after OnCreate")
            .factory_copy();
        self.source_file_mut(updated)
            .expect("new source metadata")
            .copy_parser_fields(state);
        self.finish_update(updated, original)
    }
}

#[cfg(test)]
mod ecma_line_cache_tests {
    use super::*;

    #[test]
    fn cache_is_lazy_stable_and_factory_copies_start_empty() {
        let text = SourceText::from_loaded_bytes(b"a\r\nb\xe2\x80\xa8\xff".as_slice());
        let original = SourceFileState::new(SourceFileParseOptions::default(), text.clone());
        assert!(original.ecma_line_map.get().is_none());
        assert_eq!(original.ecma_line_map(), &[0, 3, 7]);
        assert!(std::ptr::eq(
            original.ecma_line_map(),
            original.ecma_line_map()
        ));
        let mut copy = SourceFileState::new(SourceFileParseOptions::default(), text);
        copy.copy_parser_fields(original.factory_copy());
        assert!(copy.ecma_line_map.get().is_none());
        assert_eq!(copy.ecma_line_map(), original.ecma_line_map());
        assert!(!std::ptr::eq(
            copy.ecma_line_map(),
            original.ecma_line_map()
        ));
        let empty = SourceFileState::new(SourceFileParseOptions::default(), SourceText::default());
        assert!(empty.ecma_line_map.get().is_none());
        assert_eq!(empty.ecma_line_map(), &[0]);
        assert!(std::ptr::eq(empty.ecma_line_map(), empty.ecma_line_map()));
    }
}
