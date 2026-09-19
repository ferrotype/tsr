use crate::ParserFactory;
use std::collections::HashSet;
use ts_ast::{
    node_flags, Diagnostic, JsString, NodeId, NodeListId, NodeSlice, SourceFileParseOptions,
    SyntaxKind,
};
use ts_core::{LanguageVariant, ScriptKind, TextRange};
use ts_jsstring::SourceText;
use ts_scanner::{Checkpoint, CommentRange, Scanner};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum ParsingContext {
    SourceElements,
    BlockStatements,
    SwitchClauses,
    SwitchClauseStatements,
    TypeMembers,
    ClassMembers,
    EnumMembers,
    HeritageClauseElement,
    VariableDeclarations,
    ObjectBindingElements,
    ArrayBindingElements,
    ArgumentExpressions,
    ObjectLiteralMembers,
    JsxAttributes,
    JsxChildren,
    ArrayLiteralMembers,
    Parameters,
    JSDocParameters,
    RestProperties,
    TypeParameters,
    TypeArguments,
    TupleElementTypes,
    HeritageClauses,
    ImportOrExportSpecifiers,
    ImportAttributes,
    JSDocComment,
}

pub(crate) struct JSDocInfo {
    pub(crate) parent: NodeId,
    pub(crate) js_docs: Vec<NodeId>,
}

pub(crate) struct Parser<'src, F: ParserFactory> {
    pub(crate) scanner: Scanner<'src>,
    pub(crate) factory: F,
    pub(crate) opts: SourceFileParseOptions,
    pub(crate) source_owner: &'src SourceText,
    pub(crate) source_text: &'src [u8],
    pub(crate) script_kind: ScriptKind,
    pub(crate) language_variant: LanguageVariant,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) js_diagnostics: Vec<Diagnostic>,
    pub(crate) jsdoc_diagnostics: Vec<Diagnostic>,
    pub(crate) token: SyntaxKind,
    pub(crate) source_flags: u32,
    pub(crate) context_flags: u32,
    pub(crate) parsing_contexts: u32,
    pub(crate) statement_has_await_identifier: bool,
    pub(crate) has_deprecated_tag: bool,
    pub(crate) has_parse_error: bool,
    pub(crate) identifier_count: i64,
    pub(crate) not_parenthesized_arrow: HashSet<i64>,
    pub(crate) jsdoc_infos: Vec<JSDocInfo>,
    pub(crate) possible_await_spans: Vec<usize>,
    pub(crate) jsdoc_comments_space: Vec<JsString>,
    pub(crate) jsdoc_comment_ranges_space: Vec<CommentRange>,
    pub(crate) jsdoc_tag_comments_space: Vec<JsString>,
    pub(crate) jsdoc_tag_comments_parts_space: Vec<NodeId>,
    pub(crate) reparse_list: Vec<NodeId>,
    pub(crate) reparsed_clones: Vec<NodeId>,
    parent_scratch: Vec<NodeId>,
}

/// Only fields in the pinned ParserState participate in speculation. Counts,
/// allocations, identifier count and source flags deliberately survive rewind.
pub(crate) struct ParserState<'src> {
    scanner_state: Checkpoint<'src>,
    pub(crate) context_flags: u32,
    pub(crate) diagnostics_len: usize,
    js_diagnostics_len: usize,
    jsdoc_infos_len: usize,
    reparsed_clones_len: usize,
    statement_has_await_identifier: bool,
    has_parse_error: bool,
}

impl<'src, F: ParserFactory> Parser<'src, F> {
    /// port: tsc/internal/parser/parser.go:Parser.initializeState
    pub(crate) fn new(
        opts: SourceFileParseOptions,
        source: &'src SourceText,
        script_kind: ScriptKind,
        factory: F,
    ) -> Self {
        if script_kind == ScriptKind::UNKNOWN {
            let mut message = b"ScriptKind must be specified when parsing source file: ".to_vec();
            message.extend_from_slice(opts.file_name.as_bytes());
            match String::from_utf8(message) {
                Ok(message) => std::panic::panic_any(message),
                // Go panic strings preserve malformed filename bytes. The byte
                // payload remains available to a caller that catches the unwind.
                Err(message) => std::panic::panic_any(message.into_bytes()),
            }
        }
        let language_variant = get_language_variant(script_kind);
        let context_flags = match script_kind {
            ScriptKind::JS | ScriptKind::JSX => node_flags::JAVA_SCRIPT_FILE,
            ScriptKind::JSON => node_flags::JAVA_SCRIPT_FILE | node_flags::JSON_FILE,
            _ => 0,
        };
        let mut scanner = Scanner::new();
        scanner.set_text(source.as_bytes());
        scanner.buffer_diagnostics();
        scanner.set_language_variant(language_variant);
        Self {
            scanner,
            factory,
            opts,
            source_owner: source,
            source_text: source.as_bytes(),
            script_kind,
            language_variant,
            diagnostics: Vec::new(),
            js_diagnostics: Vec::new(),
            jsdoc_diagnostics: Vec::new(),
            token: SyntaxKind::Unknown,
            source_flags: 0,
            context_flags,
            parsing_contexts: 0,
            statement_has_await_identifier: false,
            has_deprecated_tag: false,
            has_parse_error: false,
            identifier_count: 0,
            not_parenthesized_arrow: HashSet::new(),
            jsdoc_infos: Vec::new(),
            possible_await_spans: Vec::new(),
            jsdoc_comments_space: Vec::new(),
            jsdoc_comment_ranges_space: Vec::new(),
            jsdoc_tag_comments_space: Vec::new(),
            jsdoc_tag_comments_parts_space: Vec::new(),
            reparse_list: Vec::new(),
            reparsed_clones: Vec::new(),
            parent_scratch: Vec::new(),
        }
    }

    /// port: tsc/internal/parser/parser.go:Parser.mark
    pub(crate) fn mark(&mut self) -> ParserState<'src> {
        ParserState {
            scanner_state: self.scanner.mark(),
            context_flags: self.context_flags,
            diagnostics_len: self.diagnostics.len(),
            js_diagnostics_len: self.js_diagnostics.len(),
            jsdoc_infos_len: self.jsdoc_infos.len(),
            reparsed_clones_len: self.reparsed_clones.len(),
            statement_has_await_identifier: self.statement_has_await_identifier,
            has_parse_error: self.has_parse_error,
        }
    }

    /// port: tsc/internal/parser/parser.go:Parser.rewind
    pub(crate) fn rewind(&mut self, state: ParserState<'src>) {
        self.scanner.rewind(state.scanner_state);
        self.token = self.scanner.token();
        self.context_flags = state.context_flags;
        self.diagnostics.truncate(state.diagnostics_len);
        self.js_diagnostics.truncate(state.js_diagnostics_len);
        self.jsdoc_infos.truncate(state.jsdoc_infos_len);
        self.reparsed_clones.truncate(state.reparsed_clones_len);
        self.statement_has_await_identifier = state.statement_has_await_identifier;
        self.has_parse_error = state.has_parse_error;
    }

    pub(crate) fn commit(&mut self, state: ParserState<'src>) {
        self.scanner.commit(state.scanner_state);
    }

    /// port: tsc/internal/parser/parser.go:Parser.lookAhead
    pub(crate) fn look_ahead(&mut self, callback: impl FnOnce(&mut Self) -> bool) -> bool {
        let state = self.mark();
        let result = callback(self);
        self.rewind(state);
        result
    }

    /// port: tsc/internal/parser/parser.go:Parser.isJavaScript
    pub(crate) fn is_java_script(&self) -> bool {
        matches!(self.script_kind, ScriptKind::JS | ScriptKind::JSX)
    }
    /// port: tsc/internal/parser/parser.go:Parser.nodePos
    pub(crate) fn node_pos(&self) -> i64 {
        self.scanner.token_full_start()
    }
    /// port: tsc/internal/parser/parser.go:Parser.hasPrecedingLineBreak
    pub(crate) fn has_preceding_line_break(&self) -> bool {
        self.scanner.has_preceding_line_break()
    }
    /// port: tsc/internal/parser/parser.go:Parser.jsdocScannerInfo
    pub(crate) fn jsdoc_scanner_info(&self) -> u8 {
        if !self.scanner.has_preceding_jsdoc_comment() {
            return 0;
        }
        1 | (u8::from(self.scanner.has_preceding_jsdoc_with_deprecated_tag()) * 2)
            | (u8::from(self.scanner.has_preceding_jsdoc_with_see_or_link()) * 4)
    }
    /// port: tsc/internal/parser/parser.go:Parser.setContextFlags
    pub(crate) fn set_context_flags(&mut self, flags: u32, value: bool) {
        if value {
            self.context_flags |= flags;
        } else {
            self.context_flags &= !flags;
        }
    }
    /// port: tsc/internal/parser/parser.go:doInContext
    pub(crate) fn do_in_context<T>(
        &mut self,
        flags: u32,
        value: bool,
        callback: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let saved = self.context_flags;
        self.set_context_flags(flags, value);
        let result = callback(self);
        self.context_flags = saved;
        result
    }
    /// port: tsc/internal/parser/parser.go:Parser.inYieldContext
    pub(crate) fn in_yield_context(&self) -> bool {
        self.context_flags & node_flags::YIELD_CONTEXT != 0
    }
    /// port: tsc/internal/parser/parser.go:Parser.inAwaitContext
    pub(crate) fn in_await_context(&self) -> bool {
        self.context_flags & node_flags::AWAIT_CONTEXT != 0
    }
    /// port: tsc/internal/parser/parser.go:Parser.inDisallowInContext
    pub(crate) fn in_disallow_in_context(&self) -> bool {
        self.context_flags & node_flags::DISALLOW_IN_CONTEXT != 0
    }
    /// port: tsc/internal/parser/parser.go:Parser.inDisallowConditionalTypesContext
    pub(crate) fn in_disallow_conditional_types_context(&self) -> bool {
        self.context_flags & node_flags::DISALLOW_CONDITIONAL_TYPES_CONTEXT != 0
    }
    /// port: tsc/internal/parser/parser.go:Parser.inDecoratorContext
    pub(crate) fn in_decorator_context(&self) -> bool {
        self.context_flags & node_flags::DECORATOR_CONTEXT != 0
    }
    /// port: tsc/internal/parser/parser.go:Parser.skipRangeTrivia
    pub(crate) fn skip_range_trivia(&self, loc: TextRange) -> TextRange {
        TextRange::new(
            ts_scanner::skip_trivia(self.source_text, loc.pos()),
            loc.end(),
        )
    }

    pub(crate) fn new_parsed_node_list(
        &mut self,
        loc: TextRange,
        nodes: crate::list_buffer::ListBuffer,
    ) -> NodeListId {
        let nodes = self.factory.finish_list_buffer(nodes);
        self.factory.alloc_list(loc, nodes)
    }
    /// port: tsc/internal/parser/parser.go:Parser.newNodeList
    pub(crate) fn new_node_list(&mut self, loc: TextRange, nodes: Vec<NodeId>) -> NodeListId {
        let nodes = if nodes.is_empty() {
            NodeSlice::empty()
        } else {
            self.factory
                .alloc_nodes(nodes.into_iter().map(Some).collect())
        };
        self.factory.alloc_list(loc, nodes)
    }
    /// port: tsc/internal/parser/parser.go:Parser.newModifierList
    pub(crate) fn new_modifier_list(&mut self, loc: TextRange, nodes: Vec<NodeId>) -> NodeListId {
        let nodes = if nodes.is_empty() {
            NodeSlice::empty()
        } else {
            self.factory
                .alloc_nodes(nodes.into_iter().map(Some).collect())
        };
        let list = self.factory.new_modifier_list(nodes);
        self.factory.set_list_location(list, loc);
        list
    }
    /// port: tsc/internal/parser/parser.go:modifierListHasAsync
    pub(crate) fn modifier_list_has_async(&self, list: Option<NodeListId>) -> bool {
        list.is_some_and(|list| {
            let nodes = self.factory.read_list(list).nodes();
            self.factory
                .read_nodes(nodes)
                .iter()
                .flatten()
                .any(|id| self.factory.node(id).kind() == SyntaxKind::AsyncKeyword)
        })
    }
    /// port: tsc/internal/parser/parser.go:Parser.finishNode
    pub(crate) fn finish_node(&mut self, node: NodeId, pos: i64) -> NodeId {
        self.finish_node_with_end(node, pos, self.node_pos())
    }
    /// port: tsc/internal/parser/parser.go:Parser.finishNodeWithEnd
    pub(crate) fn finish_node_with_end(&mut self, node: NodeId, pos: i64, end: i64) -> NodeId {
        self.factory.finish_node(
            node,
            TextRange::new(pos, end),
            self.context_flags
                | if self.has_parse_error {
                    node_flags::THIS_NODE_HAS_ERROR
                } else {
                    0
                },
        );
        self.has_parse_error = false;
        self.override_parent_in_immediate_children(node);
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.overrideParentInImmediateChildren
    pub(crate) fn override_parent_in_immediate_children(&mut self, node: NodeId) {
        self.factory
            .override_parent_in_immediate_children(node, &mut self.parent_scratch);
    }
}

/// port: tsc/internal/parser/utilities.go:getLanguageVariant
pub(crate) fn get_language_variant(kind: ScriptKind) -> LanguageVariant {
    if matches!(
        kind,
        ScriptKind::JS | ScriptKind::JSX | ScriptKind::TSX | ScriptKind::JSON
    ) {
        LanguageVariant::JSX
    } else {
        LanguageVariant::STANDARD
    }
}

pub(crate) use ts_ast::modifier_to_flag;

impl<F: ParserFactory> Parser<'_, F> {
    /// The source forwards through payload interfaces, independently of header kind.
    /// port: tsc/internal/ast/ast.go:Node.Modifiers
    pub(crate) fn node_modifiers(&self, node: NodeId) -> Option<NodeListId> {
        match self.factory.node(node).data() {
            ts_ast::NodeDataRead::VariableStatement(data) => data.modifiers(),
            ts_ast::NodeDataRead::ParameterDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::MissingDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::FunctionDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::ClassDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::ClassExpression(data) => data.modifiers(),
            ts_ast::NodeDataRead::InterfaceDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::TypeAliasDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::EnumMember(data) => data.modifiers(),
            ts_ast::NodeDataRead::EnumDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::ImportDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::ExportAssignment(data) => data.modifiers(),
            ts_ast::NodeDataRead::NamespaceExportDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::ConstructorDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::GetAccessorDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::SetAccessorDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::IndexSignatureDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::MethodSignatureDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::MethodDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::PropertySignatureDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::PropertyDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::ClassStaticBlockDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::BinaryExpression(data) => data.modifiers(),
            ts_ast::NodeDataRead::ArrowFunction(data) => data.modifiers(),
            ts_ast::NodeDataRead::FunctionExpression(data) => data.modifiers(),
            ts_ast::NodeDataRead::PropertyAssignment(data) => data.modifiers(),
            ts_ast::NodeDataRead::ShorthandPropertyAssignment(data) => data.modifiers(),
            ts_ast::NodeDataRead::FunctionTypeNode(data) => data.modifiers(),
            ts_ast::NodeDataRead::ConstructorTypeNode(data) => data.modifiers(),
            ts_ast::NodeDataRead::ModuleDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::ImportEqualsDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::ExportDeclaration(data) => data.modifiers(),
            ts_ast::NodeDataRead::TypeParameterDeclaration(data) => data.modifiers(),
            _ => None,
        }
    }
}
