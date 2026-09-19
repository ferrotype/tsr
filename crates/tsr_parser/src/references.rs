use crate::{Parser, ParserFactory};
use std::ops::ControlFlow;
use ts_ast::{
    modifier_flags, node_flags, subtree_flags, AstBuilder, ChildVisitor, Factory, JsString, NodeId,
    NodeListId, NodeSlice, RuntimeFactory, SyntaxKind as K,
};
use ts_core::{ScriptKind, Tristate};

#[derive(Default)]
struct CollectedReferences {
    imports: Vec<Option<NodeId>>,
    augmentations: Vec<Option<NodeId>>,
    ambient_names: Vec<JsString>,
}

impl Parser<'_, AstBuilder> {
    /// port: tsc/internal/ast/parseoptions.go:SetExternalModuleIndicator
    pub(crate) fn set_external_module_indicator(&mut self, root: NodeId) {
        let indicator = self.get_external_module_indicator(root);
        self.factory
            .source_file_mut(root)
            .expect("source metadata")
            .external_module_indicator = indicator;
    }
    /// port: tsc/internal/ast/parseoptions.go:getExternalModuleIndicator
    fn get_external_module_indicator(&self, root: NodeId) -> Option<NodeId> {
        let file = self
            .factory
            .view()
            .source_file(root)
            .expect("source metadata");
        if file.script_kind == ScriptKind::JSON {
            return None;
        }
        if let Some(node) = self.is_file_probably_external_module(root) {
            return Some(node);
        }
        if file.is_declaration_file {
            return None;
        }
        let opts = file.parse_options().external_module_indicator_options;
        if opts.jsx {
            if let Some(node) = self.walk_tree_for_jsx_tags(root) {
                return Some(node);
            }
        }
        opts.force.then_some(root)
    }
    /// port: tsc/internal/ast/parseoptions.go:isFileProbablyExternalModule
    fn is_file_probably_external_module(&self, root: NodeId) -> Option<NodeId> {
        let statements = self.source_statements(root);
        for node in self.factory.read_nodes(statements).iter().flatten() {
            if self.is_an_external_module_indicator_node(node) {
                return Some(node);
            }
        }
        self.get_import_meta_if_necessary(root)
    }
    /// port: tsc/internal/ast/parseoptions.go:isAnExternalModuleIndicatorNode
    fn is_an_external_module_indicator_node(&self, node: NodeId) -> bool {
        if self.node_modifiers(node).is_some_and(|list| {
            self.factory.read_list(list).modifier_flags() & modifier_flags::EXPORT != 0
        }) {
            return true;
        }
        let node = self.factory.node(node);
        match node.kind().known() {
            Some(K::ImportEqualsDeclaration) => {
                let reference = node
                    .data_source()
                    .as_import_equals_declaration()
                    .expect("import-equals payload")
                    .module_reference()
                    .expect("parsed module reference");
                self.factory.node(reference).kind() == K::ExternalModuleReference
            }
            Some(K::ImportDeclaration | K::ExportAssignment | K::ExportDeclaration) => true,
            _ => false,
        }
    }
    /// port: tsc/internal/ast/parseoptions.go:getImportMetaIfNecessary
    fn get_import_meta_if_necessary(&self, root: NodeId) -> Option<NodeId> {
        if self.factory.node(root).flags() & node_flags::POSSIBLY_CONTAINS_IMPORT_META == 0 {
            return None;
        }
        self.find_child_node(root, &|p, id| p.is_import_meta(id))
    }
    /// port: tsc/internal/ast/parseoptions.go:walkTreeForJSXTags
    fn walk_tree_for_jsx_tags(&self, node: NodeId) -> Option<NodeId> {
        crate::recursion::guarded(|| {
            if self.factory.view().subtree_facts(node) & subtree_flags::JSX == 0 {
                return None;
            }
            if matches!(
                self.factory.node(node).kind().known(),
                Some(K::JsxOpeningElement | K::JsxSelfClosingElement | K::JsxFragment)
            ) {
                return Some(node);
            }
            let mut result = None;
            self.first_child_where(node, |child| {
                result = self.walk_tree_for_jsx_tags(child);
                result.is_some()
            });
            result
        })
    }
    /// port: tsc/internal/parser/references.go:collectExternalModuleReferences
    pub(crate) fn collect_external_module_references(&mut self, root: NodeId) {
        let mut collected = CollectedReferences::default();
        let statements = self.source_statements(root);
        for i in 0..statements.len() {
            let node = self
                .factory
                .read_nodes(statements)
                .at(i)
                .expect("source statement");
            self.collect_module_references(root, node, false, &mut collected);
        }
        let flags = self.factory.node(root).flags();
        if flags & (node_flags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT | node_flags::JAVA_SCRIPT_FILE)
            != 0
        {
            let javascript = flags & node_flags::JAVA_SCRIPT_FILE != 0;
            let mut start = 0;
            while let Some((index, size)) = find_import_or_require(self.source_text, start) {
                let node = self.get_node_at_position(root, index as i64, javascript);
                let argument = if javascript && self.is_require_call(node) {
                    self.first_call_argument(node)
                } else if self.is_import_call(node) {
                    self.first_call_argument(node)
                        .filter(|&id| self.is_string_literal_like(id))
                } else {
                    self.literal_import_type_argument(node)
                };
                if let Some(argument) = argument {
                    collected.imports.push(Some(argument));
                }
                start = index + size;
            }
        }
        let imports = if collected.imports.is_empty() {
            ts_ast::SourceNodeSlice::empty()
        } else {
            self.factory
                .source_nodes(collected.imports)
                .expect("owned module references")
        };
        let augmentations = if collected.augmentations.is_empty() {
            ts_ast::SourceNodeSlice::empty()
        } else {
            self.factory
                .source_nodes(collected.augmentations)
                .expect("owned augmentations")
        };
        let ambient_names = if collected.ambient_names.is_empty() {
            ts_ast::SourceTextSlice::empty()
        } else {
            self.factory
                .source_strings(collected.ambient_names)
                .expect("owned ambient names")
        };
        let file = self.factory.source_file_mut(root).expect("source metadata");
        file.imports = imports;
        file.module_augmentations = augmentations;
        file.ambient_module_names = ambient_names;
    }
    /// port: tsc/internal/parser/references.go:collectModuleReferences
    fn collect_module_references(
        &mut self,
        root: NodeId,
        node: NodeId,
        ambient: bool,
        collected: &mut CollectedReferences,
    ) {
        crate::recursion::guarded(|| {
            let kind = self.factory.node(node).kind();
            if matches!(
                kind.known(),
                Some(
                    K::ImportDeclaration
                        | K::ImportEqualsDeclaration
                        | K::JSImportDeclaration
                        | K::ExportDeclaration
                )
            ) {
                if let Some(module) = self.static_external_module_name(node) {
                    if self.factory.node(module).kind() == K::StringLiteral {
                        let name = self.reference_name_text(module);
                        if !name.is_empty()
                            && (!ambient || !is_external_module_name_relative(name.as_bytes()))
                        {
                            let file = self.factory.source_file_mut(root).expect("source metadata");
                            collected.imports.push(Some(module));
                            if file.uses_uri_style_node_core_modules != Tristate::TRUE
                                && !file.is_declaration_file
                            {
                                if name.as_bytes().starts_with(b"node:")
                                    && !exclusively_prefixed_node_core_module(name.as_bytes())
                                {
                                    file.uses_uri_style_node_core_modules = Tristate::TRUE;
                                } else if file.uses_uri_style_node_core_modules == Tristate::UNKNOWN
                                    && unprefixed_node_core_module(name.as_bytes())
                                {
                                    file.uses_uri_style_node_core_modules = Tristate::FALSE;
                                }
                            }
                        }
                    }
                }
                return;
            }
            if kind != K::ModuleDeclaration {
                return;
            }
            let (name, body, keyword, modifiers) = {
                let data = self.factory.node(node);
                let module = data
                    .data_source()
                    .as_module_declaration()
                    .expect("module payload");
                (
                    module.name().expect("parsed module name"),
                    module.body(),
                    module.keyword(),
                    module.modifiers(),
                )
            };
            if self.factory.node(name).kind() != K::StringLiteral && keyword != K::GlobalKeyword {
                return;
            }
            let ambient_modifier = modifiers.is_some_and(|list| {
                self.factory.read_list(list).modifier_flags() & modifier_flags::AMBIENT != 0
            });
            let (declaration, external) = {
                let file = self
                    .factory
                    .view()
                    .source_file(root)
                    .expect("source metadata");
                (
                    file.is_declaration_file,
                    file.external_module_indicator.is_some(),
                )
            };
            if !ambient && !ambient_modifier && !declaration {
                return;
            }
            let text = self.reference_name_text(name);
            if external || ambient && !is_external_module_name_relative(text.as_bytes()) {
                collected.augmentations.push(Some(name));
            } else if !ambient {
                collected.ambient_names.push(text);
                if let Some(body) = body {
                    let statements = self
                        .factory
                        .node(body)
                        .data_source()
                        .as_module_block()
                        .expect("ambient module body is a block")
                        .statements();
                    if let Some(statements) = statements {
                        let nodes = self.factory.read_list(statements).nodes();
                        for i in 0..nodes.len() {
                            let node = self
                                .factory
                                .read_nodes(nodes)
                                .at(i)
                                .expect("module statement");
                            self.collect_module_references(root, node, true, collected);
                        }
                    }
                }
            }
        });
    }
    fn source_statements(&self, root: NodeId) -> NodeSlice {
        let list = self
            .factory
            .node(root)
            .data_source()
            .as_source_file()
            .expect("source payload")
            .statements()
            .expect("source statements");
        self.factory.read_list(list).nodes()
    }
    /// port: tsc/internal/ast/utilities.go:GetNodeAtPosition
    fn get_node_at_position(&self, root: NodeId, position: i64, include_jsdoc: bool) -> NodeId {
        let mut current = root;
        loop {
            let child = if include_jsdoc {
                self.jsdoc_infos
                    .iter()
                    .rev()
                    .find(|info| info.parent == current)
                    .and_then(|info| {
                        info.js_docs
                            .iter()
                            .copied()
                            .find(|&node| self.node_contains_position(node, position))
                    })
            } else {
                None
            };
            let child = child.or_else(|| {
                self.first_child_where(current, |node| self.node_contains_position(node, position))
            });
            if child.is_none_or(|child| self.factory.node(child).kind() == K::MetaProperty) {
                return current;
            }
            current = child.expect("contained child was found");
        }
    }
}

impl<F: ParserFactory> Parser<'_, F> {
    fn first_child_where(
        &self,
        node: NodeId,
        mut predicate: impl FnMut(NodeId) -> bool,
    ) -> Option<NodeId> {
        struct Find<'a, F, P> {
            factory: &'a F,
            predicate: &'a mut P,
            found: Option<NodeId>,
        }
        impl<F: ParserFactory, P: FnMut(NodeId) -> bool> ChildVisitor for Find<'_, F, P> {
            fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
                if (self.predicate)(node) {
                    self.found = Some(node);
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            }
            fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
                self.visit_node_slice(self.factory.read_list(list).nodes())
            }
            fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
                for node in self.factory.read_nodes(nodes).iter().flatten() {
                    self.visit_node(node)?;
                }
                ControlFlow::Continue(())
            }
        }
        let mut finder = Find {
            factory: &self.factory,
            predicate: &mut predicate,
            found: None,
        };
        let _ = self.factory.node(node).for_each_child(&mut finder);
        finder.found
    }
    /// port: tsc/internal/ast/parseoptions.go:findChildNode
    fn find_child_node(
        &self,
        node: NodeId,
        check: &impl Fn(&Self, NodeId) -> bool,
    ) -> Option<NodeId> {
        crate::recursion::guarded(|| {
            if check(self, node) {
                return Some(node);
            }
            let mut result = None;
            self.first_child_where(node, |child| {
                result = self.find_child_node(child, check);
                result.is_some()
            });
            result
        })
    }
    /// port: tsc/internal/ast/utilities.go:nodeContainsPosition
    fn node_contains_position(&self, node: NodeId, position: i64) -> bool {
        let node = self.factory.node(node);
        let loc = node.range();
        node.kind().raw() >= K::FirstNode as i16
            && loc.pos() <= position
            && (position < loc.end() || position == loc.end() && node.kind() == K::EndOfFile)
    }
    /// port: tsc/internal/ast/utilities.go:IsImportMeta
    fn is_import_meta(&self, node: NodeId) -> bool {
        let node = self.factory.node(node);
        node.kind() == K::MetaProperty && {
            let meta = node.data_source().as_meta_property().expect("meta payload");
            meta.keyword_token() == K::ImportKeyword
                && self
                    .reference_name_text(meta.name().expect("meta name"))
                    .as_bytes()
                    == b"meta"
        }
    }
    fn reference_name_text(&self, node: NodeId) -> JsString {
        let node = self.factory.node(node);
        match node.kind().known() {
            Some(K::Identifier) => node
                .data_source()
                .as_identifier()
                .expect("identifier payload")
                .text_owned(),
            Some(K::StringLiteral) => node
                .data_source()
                .as_string_literal()
                .expect("string payload")
                .text_owned(),
            _ => panic!("module name is neither identifier nor string literal"),
        }
    }
    fn static_external_module_name(&self, node: NodeId) -> Option<NodeId> {
        let node = self.factory.node(node);
        match node.kind().known() {
            Some(K::ImportDeclaration | K::JSImportDeclaration) => node
                .data_source()
                .as_import_declaration()
                .expect("import payload")
                .module_specifier(),
            Some(K::ExportDeclaration) => node
                .data_source()
                .as_export_declaration()
                .expect("export payload")
                .module_specifier(),
            Some(K::ImportEqualsDeclaration) => {
                let reference = self.factory.node(
                    node.data_source()
                        .as_import_equals_declaration()
                        .expect("import-equals payload")
                        .module_reference()
                        .expect("module reference"),
                );
                if reference.kind() == K::ExternalModuleReference {
                    reference
                        .data_source()
                        .as_external_module_reference()
                        .expect("external reference payload")
                        .expression()
                } else {
                    None
                }
            }
            _ => panic!("static module-name helper called on unsupported node"),
        }
    }
    fn is_string_literal_like(&self, node: NodeId) -> bool {
        matches!(
            self.factory.node(node).kind().known(),
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
        )
    }
    fn first_call_argument(&self, node: NodeId) -> Option<NodeId> {
        let args = self
            .factory
            .node(node)
            .data_source()
            .as_call_expression()
            .expect("call payload")
            .arguments();
        args.and_then(|list| {
            self.factory
                .read_nodes(self.factory.read_list(list).nodes())
                .first()
                .flatten()
        })
    }
    fn is_require_call(&self, node: NodeId) -> bool {
        let node = self.factory.node(node);
        if node.kind() != K::CallExpression {
            return false;
        }
        let call = node
            .data_source()
            .as_call_expression()
            .expect("call payload");
        let expression = self.factory.node(call.expression().expect("call target"));
        if expression.kind() != K::Identifier
            || expression
                .data_source()
                .as_identifier()
                .expect("identifier payload")
                .text()
                != b"require"
        {
            return false;
        }
        let nodes = call.arguments().map_or(NodeSlice::empty(), |list| {
            self.factory.read_list(list).nodes()
        });
        nodes.len() == 1
            && self.is_string_literal_like(
                self.factory
                    .read_nodes(nodes)
                    .at(0)
                    .expect("parsed argument"),
            )
    }
    /// port: tsc/internal/ast/utilities.go:IsImportCall
    fn is_import_call(&self, node: NodeId) -> bool {
        let node = self.factory.node(node);
        if node.kind() != K::CallExpression {
            return false;
        }
        let expression = self.factory.node(
            node.data_source()
                .as_call_expression()
                .expect("call payload")
                .expression()
                .expect("call target"),
        );
        expression.kind() == K::ImportKeyword
            || expression.kind() == K::MetaProperty && {
                let meta = expression
                    .data_source()
                    .as_meta_property()
                    .expect("meta payload");
                meta.keyword_token() == K::ImportKeyword
                    && self
                        .reference_name_text(meta.name().expect("meta name"))
                        .as_bytes()
                        == b"defer"
            }
    }
    fn literal_import_type_argument(&self, node: NodeId) -> Option<NodeId> {
        let node = self.factory.node(node);
        if node.kind() != K::ImportType {
            return None;
        }
        let argument = node
            .data_source()
            .as_import_type_node()
            .expect("import type payload")
            .argument()?;
        let argument = self.factory.node(argument);
        if argument.kind() != K::LiteralType {
            return None;
        }
        argument
            .data_source()
            .as_literal_type_node()
            .expect("literal type payload")
            .literal()
            .filter(|&literal| self.factory.node(literal).kind() == K::StringLiteral)
    }
}

/// port: tsc/internal/ast/utilities.go:findImportOrRequire
fn find_import_or_require(text: &[u8], mut index: usize) -> Option<(usize, usize)> {
    while index < text.len() {
        index += text[index..]
            .iter()
            .position(|&byte| matches!(byte, b'i' | b'r'))?;
        let expected: &[u8] = if text[index] == b'i' {
            b"import"
        } else {
            b"require"
        };
        if text[index..].starts_with(expected) {
            return Some((index, expected.len()));
        }
        index += 1;
    }
    None
}
/// port: tsc/internal/tspath/path.go:IsExternalModuleNameRelative
fn is_external_module_name_relative(name: &[u8]) -> bool {
    matches!(name, b"." | b"..")
        || name.starts_with(b"./")
        || name.starts_with(b"../")
        || name.starts_with(b".\\")
        || name.starts_with(b"..\\")
        || ts_core::path::encoded_root_length(name) > 0
}

// Exact membership of core/nodemodules.go:UnprefixedNodeCoreModules at the source pin.
fn unprefixed_node_core_module(name: &[u8]) -> bool {
    matches!(
        name,
        b"assert"
            | b"assert/strict"
            | b"async_hooks"
            | b"buffer"
            | b"child_process"
            | b"cluster"
            | b"console"
            | b"constants"
            | b"crypto"
            | b"dgram"
            | b"diagnostics_channel"
            | b"dns"
            | b"dns/promises"
            | b"domain"
            | b"events"
            | b"fs"
            | b"fs/promises"
            | b"http"
            | b"http2"
            | b"https"
            | b"inspector"
            | b"inspector/promises"
            | b"module"
            | b"net"
            | b"os"
            | b"path"
            | b"path/posix"
            | b"path/win32"
            | b"perf_hooks"
            | b"process"
            | b"punycode"
            | b"querystring"
            | b"readline"
            | b"readline/promises"
            | b"repl"
            | b"stream"
            | b"stream/consumers"
            | b"stream/promises"
            | b"stream/web"
            | b"string_decoder"
            | b"sys"
            | b"timers"
            | b"timers/promises"
            | b"tls"
            | b"trace_events"
            | b"tty"
            | b"url"
            | b"util"
            | b"util/types"
            | b"v8"
            | b"vm"
            | b"wasi"
            | b"worker_threads"
            | b"zlib"
    )
}

// Exact membership of core/nodemodules.go:ExclusivelyPrefixedNodeCoreModules at the source pin.
fn exclusively_prefixed_node_core_module(name: &[u8]) -> bool {
    matches!(
        name,
        b"node:quic" | b"node:sea" | b"node:sqlite" | b"node:test" | b"node:test/reporters"
    )
}
