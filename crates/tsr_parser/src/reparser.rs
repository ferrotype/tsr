//! JavaScript JSDoc elaboration through the production AST factory.
use crate::{JSDocInfo, Parser, ParserFactory, ParsingContext};
use ts_ast::SyntaxKind as K;
use ts_ast::{node_flags, FactoryMethods, JsString, NodeData, NodeDataRead, NodeId, NodeListId};
use ts_core::TextRange;
use ts_diagnostics as diagnostics;

struct ClassLikeFields {
    heritage: Option<NodeListId>,
}

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/reparser.go:Parser.finishReparsedNode
    fn finish_reparsed_node(&mut self, node: NodeId, location: NodeId) {
        let loc = self.factory.node(location).range();
        self.factory
            .set_node_flags(node, self.context_flags | node_flags::REPARSED);
        self.factory.set_node_range(node, loc);
        self.override_parent_in_immediate_children(node);
    }
    /// port: tsc/internal/parser/reparser.go:Parser.finishMutatedNode
    fn finish_mutated_node(&mut self, node: NodeId) {
        self.override_parent_in_immediate_children(node);
    }
    /// port: tsc/internal/parser/reparser.go:Parser.addDeepCloneReparse
    fn add_deep_clone_reparse(&mut self, node: Option<NodeId>) -> Option<NodeId> {
        let cloned = ts_ast::deep_clone_reparse(&mut self.factory, node);
        if let Some(node) = cloned {
            self.reparsed_clones.push(node);
        }
        cloned
    }
    /// port: tsc/internal/parser/reparser.go:Parser.addTransformedReparse
    fn add_transformed_reparse(&mut self, node: NodeId, old: NodeId) -> NodeId {
        self.finish_reparsed_node(node, old);
        self.factory
            .add_node_flags(node, node_flags::REPARSER_TRANSFORMED_LITERAL);
        self.reparsed_clones.push(node);
        node
    }
    /// port: tsc/internal/parser/reparser.go:Parser.checkNonIdentifierName
    fn check_non_identifier_name(&mut self, name: Option<NodeId>) -> Option<NodeId> {
        let name = name?;
        if self.factory.node(name).kind() == K::Identifier
            && !ts_scanner::is_valid_identifier(self.jsdoc_text(name).as_bytes())
        {
            let loc = self.factory.node(name).range();
            let loc = if loc.is_empty() {
                TextRange::new(loc.pos() - 1, loc.pos())
            } else {
                loc
            };
            self.parse_error_at_range(loc, diagnostics::Identifier_expected, vec![]);
        }
        Some(name)
    }
    /// port: tsc/internal/parser/reparser.go:Parser.reparseTags
    pub(crate) fn reparse_tags(&mut self, parent: NodeId, docs: &[NodeId]) {
        for &doc in docs {
            let is_last = Some(&doc) == docs.last();
            let tags = self
                .factory
                .node(doc)
                .data_source()
                .as_js_doc()
                .expect("JSDoc payload")
                .tags();
            let Some(tags) = tags else {
                continue;
            };
            let nodes = self.factory.read_list(tags).nodes();
            for i in 0..nodes.len() {
                let tag = self.factory.read_nodes(nodes).at(i).expect("JSDoc tag");
                self.reparse_unhosted(tag, parent, doc);
                if is_last {
                    self.reparse_hosted(tag, parent, doc);
                }
            }
        }
    }
    /// port: tsc/internal/parser/reparser.go:Parser.reparseUnhosted
    fn reparse_unhosted(&mut self, tag: NodeId, parent: NodeId, doc: NodeId) {
        match self.factory.node(tag).kind().known() {
            Some(K::JSDocTypedefTag | K::JSDocCallbackTag) => {
                let Some(expression) = self.jsdoc_type(tag) else {
                    return;
                };
                let name = self.jsdoc_name(tag);
                let modifiers = name
                    .filter(|&id| self.factory.node(id).kind() == K::ModuleDeclaration)
                    .map(|_| self.create_export_modifier(tag));
                let typedef = self.factory.node(tag).kind() == K::JSDocTypedefTag;
                let alias = if typedef {
                    let name = self.get_innermost_name_of_jsdoc_namespace(name);
                    let name = self.check_non_identifier_name(name);
                    let name = self.add_deep_clone_reparse(name);
                    self.factory
                        .new_js_type_alias_declaration(modifiers, name, None, None)
                } else {
                    let function = self.reparse_jsdoc_signature(expression, tag, doc, tag, None);
                    let name = self.get_innermost_name_of_jsdoc_namespace(name);
                    let name = self.add_deep_clone_reparse(name);
                    self.factory.new_js_type_alias_declaration(
                        modifiers,
                        name,
                        None,
                        Some(function),
                    )
                };
                let params = self.gather_type_parameters(doc, true);
                self.set_reparse_type_parameters(alias, params);
                if typedef {
                    let ty = match self.factory.node(expression).kind().known() {
                        Some(K::JSDocTypeExpression) => {
                            self.add_deep_clone_reparse(self.jsdoc_type(expression))
                        }
                        Some(K::JSDocTypeLiteral) => {
                            self.reparse_jsdoc_type_literal(Some(expression))
                        }
                        _ => panic!(
                            "typedef tag type expression should be a name reference or a type expression{}",
                            self.factory.node(expression).kind()
                        ),
                    };
                    self.set_reparse_type(alias, ty);
                }
                self.finish_reparsed_node(alias, tag);
                self.jsdoc_infos.push(JSDocInfo {
                    parent: alias,
                    js_docs: vec![doc],
                });
                self.factory.add_node_flags(alias, node_flags::HAS_JS_DOC);
                let result = self.wrap_in_jsdoc_namespace(name, alias, false);
                self.reparse_list.push(result);
            }
            Some(K::JSDocImportTag) => {
                let (clause, specifier, attributes) = {
                    let n = self.factory.node(tag);
                    let d = n.data_source().as_js_doc_import_tag().expect("import tag");
                    (d.import_clause(), d.module_specifier(), d.attributes())
                };
                if clause.is_none() {
                    return;
                }
                let clause = self
                    .add_deep_clone_reparse(clause)
                    .expect("cloned import clause");
                if let NodeData::ImportClause(d) = self.factory.node_mut(clause).data_mut() {
                    d.phase_modifier = K::TypeKeyword.into();
                } else {
                    panic!("import clause");
                }
                let modifiers = self.node_modifiers(tag);
                let modifiers = ts_ast::deep_clone_reparse_modifiers(&mut self.factory, modifiers);
                let specifier = self.add_deep_clone_reparse(specifier);
                let attributes = self.add_deep_clone_reparse(attributes);
                let node = self.factory.new_js_import_declaration(
                    modifiers,
                    Some(clause),
                    specifier,
                    attributes,
                );
                self.finish_reparsed_node(node, tag);
                self.reparse_list.push(node);
            }
            Some(K::JSDocOverloadTag)
                if matches!(
                    self.factory.node(parent).kind().known(),
                    Some(K::FunctionDeclaration | K::MethodDeclaration | K::Constructor)
                ) && self.parsing_contexts
                    & (1 << ParsingContext::ObjectLiteralMembers as u32)
                    == 0 =>
            {
                let signature = self.reparse_jsdoc_signature(
                    self.jsdoc_type(tag).expect("overload signature"),
                    parent,
                    doc,
                    tag,
                    self.node_modifiers(parent),
                );
                self.reparse_list.push(signature);
            }
            _ => {}
        }
    }
    /// port: tsc/internal/parser/reparser.go:Parser.reparseJSDocSignature
    fn reparse_jsdoc_signature(
        &mut self,
        js_signature: NodeId,
        fun: NodeId,
        doc: NodeId,
        tag: NodeId,
        modifiers: Option<NodeListId>,
    ) -> NodeId {
        let modifiers = ts_ast::deep_clone_reparse_modifiers(&mut self.factory, modifiers);
        let signature = match self.factory.node(fun).kind().known() {
            Some(K::FunctionDeclaration | K::MethodDeclaration) => {
                let name = self.check_non_identifier_name(self.jsdoc_name(fun));
                let name = ts_ast::deep_clone_reparse(&mut self.factory, name);
                if self.factory.node(fun).kind() == K::FunctionDeclaration {
                    self.factory.new_function_declaration(
                        modifiers, None, name, None, None, None, None, None,
                    )
                } else {
                    self.factory.new_method_declaration(
                        modifiers, None, name, None, None, None, None, None, None,
                    )
                }
            }
            Some(K::Constructor) => self
                .factory
                .new_constructor_declaration(modifiers, None, None, None, None, None),
            Some(K::JSDocCallbackTag) => {
                let any = self.factory.new_keyword_type_node(K::AnyKeyword.into());
                self.factory.new_function_type_node(None, None, Some(any))
            }
            _ => panic!("Unexpected kind {}", self.factory.node(fun).kind()),
        };
        if self.factory.node(tag).kind() != K::JSDocCallbackTag {
            let params = self.gather_type_parameters(doc, false);
            self.set_reparse_type_parameters(signature, params);
        }
        let list = self
            .reparse_function_fields(js_signature)
            .1
            .expect("JSDoc signature parameters");
        let nodes = self.factory.read_list(list).nodes();
        let mut parameters = Vec::new();
        for i in 0..nodes.len() {
            let param = self
                .factory
                .read_nodes(nodes)
                .at(i)
                .expect("JSDoc parameter");
            let parameter = match self.factory.node(param).kind().known() {
                Some(K::JSDocThisTag) => {
                    let name = self
                        .factory
                        .new_identifier(JsString::from_bytes(b"this".as_slice()));
                    self.finish_reparsed_node(name, param);
                    let node = self.factory.new_parameter_declaration(
                        None,
                        None,
                        Some(name),
                        None,
                        None,
                        None,
                    );
                    if let Some(ty) = self.jsdoc_type(param) {
                        let ty = self.add_deep_clone_reparse(self.jsdoc_type(ty));
                        self.set_reparse_type(node, ty);
                    }
                    node
                }
                Some(K::JSDocParameterTag | K::JSDocPropertyTag) => {
                    let name = self.jsdoc_name(param).expect("JSDoc parameter name");
                    if self.factory.node(name).kind() == K::QualifiedName {
                        continue;
                    }
                    let mut rest = None;
                    let mut ty = None;
                    if let Some(expr) = self.jsdoc_type(param) {
                        let inner = self.jsdoc_type(expr).expect("JSDoc parameter type");
                        if self.factory.node(inner).kind() == K::JSDocVariadicType {
                            let token = self.factory.new_token(K::DotDotDotToken.into());
                            self.finish_reparsed_node(token, param);
                            rest = Some(token);
                            ty = self.reparse_jsdoc_type_literal(self.jsdoc_type(inner));
                        } else {
                            ty = self.reparse_jsdoc_type_literal(Some(inner));
                        }
                    }
                    let name = if self.factory.node(name).kind() == K::Identifier
                        && !ts_scanner::is_valid_identifier(self.jsdoc_text(name).as_bytes())
                    {
                        let text = self.jsdoc_text(name);
                        let mut result = Vec::new();
                        let mut pos = 0;
                        while pos < text.len() {
                            let (ch, len) = ts_jsstring::wtf8::decode_utf8(&text.as_bytes()[pos..]);
                            let valid = if pos == 0 {
                                ts_scanner::is_identifier_start(ch)
                            } else {
                                ts_scanner::is_identifier_part(ch)
                            };
                            if valid {
                                let c = char::from_u32(ch as u32).expect("valid identifier rune");
                                let mut bytes = [0; 4];
                                result.extend_from_slice(c.encode_utf8(&mut bytes).as_bytes());
                            } else {
                                result.push(b'_');
                            }
                            pos += len;
                        }
                        if result.is_empty() {
                            result.push(b'_');
                            result.extend_from_slice(i.to_string().as_bytes());
                        }
                        let new = self.factory.new_identifier(JsString::from_bytes(result));
                        self.add_transformed_reparse(new, name)
                    } else {
                        self.add_deep_clone_reparse(Some(name))
                            .expect("cloned name")
                    };
                    let question = self.make_question_if_optional(param);
                    self.factory.new_parameter_declaration(
                        None,
                        rest,
                        Some(name),
                        question,
                        ty,
                        None,
                    )
                }
                _ => panic!("nil reparsed parameter"),
            };
            self.finish_reparsed_node(parameter, param);
            parameters.push(parameter);
            self.reparse_jsdoc_comment(parameter, param);
        }
        let loc = self.factory.read_list(list).loc();
        let params = self.new_node_list(loc, parameters);
        self.set_reparse_parameters(signature, Some(params));
        if let Some(return_tag) = self.jsdoc_type(js_signature) {
            if let Some(expr) = self.jsdoc_type(return_tag) {
                let ty = self.add_deep_clone_reparse(self.jsdoc_type(expr));
                self.set_reparse_type(signature, ty);
            }
        }
        let loc = if self.factory.node(tag).kind() == K::JSDocOverloadTag {
            self.reparse_tag_name(tag)
        } else {
            js_signature
        };
        self.finish_reparsed_node(signature, loc);
        signature
    }
    /// port: tsc/internal/parser/reparser.go:Parser.reparseJSDocTypeLiteral
    fn reparse_jsdoc_type_literal(&mut self, ty: Option<NodeId>) -> Option<NodeId> {
        crate::recursion::guarded(|| {
            let ty = ty?;
            if self.factory.node(ty).kind() != K::JSDocTypeLiteral {
                return self.add_deep_clone_reparse(Some(ty));
            }
            let (nodes, array) = {
                let n = self.factory.node(ty);
                let d = n
                    .data_source()
                    .as_js_doc_type_literal()
                    .expect("JSDoc type literal");
                (d.js_doc_property_tags(), d.is_array_type())
            };
            let mut properties = Vec::new();
            for i in 0..nodes.len() {
                let prop = self
                    .factory
                    .read_nodes(nodes)
                    .at(i)
                    .expect("JSDoc property");
                if !matches!(
                    self.factory.node(prop).kind().known(),
                    Some(K::JSDocPropertyTag | K::JSDocParameterTag)
                ) {
                    continue;
                }
                let mut name = self.jsdoc_name(prop).expect("JSDoc property name");
                if self.factory.node(name).kind() == K::QualifiedName {
                    name = self.jsdoc_qualified_name(name).1;
                }
                name = if self.factory.node(name).kind() == K::Identifier
                    && !ts_scanner::is_valid_identifier(self.jsdoc_text(name).as_bytes())
                {
                    let new = self.factory.new_string_literal(self.jsdoc_text(name), 0);
                    self.add_transformed_reparse(new, name)
                } else {
                    self.add_deep_clone_reparse(Some(name))
                        .expect("cloned property name")
                };
                let question = self.make_question_if_optional(prop);
                let property = self.factory.new_property_signature_declaration(
                    None,
                    Some(name),
                    question,
                    None,
                    None,
                );
                if let Some(expr) = self.jsdoc_type(prop) {
                    let t = self.reparse_jsdoc_type_literal(self.jsdoc_type(expr));
                    self.set_reparse_type(property, t);
                }
                self.finish_reparsed_node(property, prop);
                properties.push(property);
                self.reparse_jsdoc_comment(property, prop);
            }
            let list = self.new_node_list(self.factory.node(ty).range(), properties);
            let mut node = self.factory.new_type_literal_node(Some(list));
            if array {
                self.finish_reparsed_node(node, ty);
                node = self.factory.new_array_type_node(Some(node));
            }
            self.finish_reparsed_node(node, ty);
            Some(node)
        })
    }
    /// port: tsc/internal/parser/reparser.go:Parser.reparseJSDocComment
    fn reparse_jsdoc_comment(&mut self, node: NodeId, tag: NodeId) {
        let comment = match self.factory.node(tag).data() {
            NodeDataRead::JSDocParameterOrPropertyTag(d) => d.comment(),
            NodeDataRead::JSDocThisTag(d) => d.comment(),
            _ => None,
        };
        let Some(comment) = comment else {
            return;
        };
        let list = self.factory.read_list(comment);
        let loc = list.loc();
        let nodes = list.nodes();
        drop(list);
        let mut comments = Vec::with_capacity(nodes.len());
        for i in 0..nodes.len() {
            let id = self.factory.read_nodes(nodes).at(i);
            comments.push(
                ts_ast::deep_clone_reparse(&mut self.factory, id).expect("cloned JSDoc comment"),
            );
        }
        let list = self.new_node_list(loc, comments);
        let doc = self.factory.new_js_doc(Some(list), None);
        self.finish_reparsed_node(doc, tag);
        self.factory.set_node_parent(doc, Some(node));
        self.jsdoc_infos.push(JSDocInfo {
            parent: node,
            js_docs: vec![doc],
        });
        self.factory.add_node_flags(node, node_flags::HAS_JS_DOC);
    }
    /// port: tsc/internal/parser/reparser.go:Parser.gatherTypeParameters
    fn gather_type_parameters(&mut self, doc: NodeId, typedef: bool) -> Option<NodeListId> {
        let tags = self
            .factory
            .node(doc)
            .data_source()
            .as_js_doc()
            .expect("JSDoc payload")
            .tags()
            .expect("JSDoc tags");
        let tags = self.factory.read_list(tags).nodes();
        let mut result = Vec::new();
        let mut pos = -1;
        let mut end = -1;
        let mut first_template = true;
        for i in 0..tags.len() {
            let tag = self.factory.read_nodes(tags).at(i).expect("JSDoc tag");
            if !typedef
                && matches!(
                    self.factory.node(tag).kind().known(),
                    Some(K::JSDocTypedefTag | K::JSDocCallbackTag)
                )
            {
                return None;
            }
            if self.factory.node(tag).kind() != K::JSDocTemplateTag {
                continue;
            }
            if first_template {
                pos = self.factory.node(tag).range().pos();
                first_template = false;
            }
            end = self.factory.node(tag).range().end();
            let (constraint, params) = {
                let n = self.factory.node(tag);
                let d = n
                    .data_source()
                    .as_js_doc_template_tag()
                    .expect("template tag");
                (
                    d.constraint(),
                    d.type_parameters().expect("template parameters"),
                )
            };
            let params = self.factory.read_list(params).nodes();
            for i in 0..params.len() {
                let param = self
                    .factory
                    .read_nodes(params)
                    .at(i)
                    .expect("type parameter");
                let node = if let Some(constraint) = constraint.filter(|_| i == 0) {
                    let mods = self.node_modifiers(param);
                    let mods = ts_ast::deep_clone_reparse_modifiers(&mut self.factory, mods);
                    let name = self.check_non_identifier_name(self.jsdoc_name(param));
                    let name = self.add_deep_clone_reparse(name);
                    let constraint = self.add_deep_clone_reparse(self.jsdoc_type(constraint));
                    let default = self
                        .factory
                        .node(param)
                        .data_source()
                        .as_type_parameter_declaration()
                        .expect("type parameter")
                        .default_type();
                    let default = self.add_deep_clone_reparse(default);
                    let node = self
                        .factory
                        .new_type_parameter_declaration(mods, name, constraint, None, default);
                    self.finish_reparsed_node(node, param);
                    node
                } else {
                    self.add_deep_clone_reparse(Some(param))
                        .expect("cloned type parameter")
                };
                result.push(node);
            }
        }
        (!result.is_empty()).then(|| self.new_node_list(TextRange::new(pos, end), result))
    }
    /// port: tsc/internal/parser/reparser.go:Parser.makeQuestionIfOptional
    fn make_question_if_optional(&mut self, tag: NodeId) -> Option<NodeId> {
        let (bracketed, expression) = {
            let n = self.factory.node(tag);
            let d = n
                .data_source()
                .as_js_doc_parameter_or_property_tag()
                .expect("parameter tag");
            (d.is_bracketed(), d.type_expression())
        };
        if bracketed
            || expression.is_some_and(|expr| {
                self.factory
                    .node(self.jsdoc_type(expr).expect("parameter type"))
                    .kind()
                    == K::JSDocOptionalType
            })
        {
            let node = self.factory.new_token(K::QuestionToken.into());
            self.finish_reparsed_node(node, tag);
            Some(node)
        } else {
            None
        }
    }
    /// port: tsc/internal/parser/reparser.go:Parser.makeNewCast
    fn make_new_cast(&mut self, ty: Option<NodeId>, expr: NodeId, assertion: bool) -> NodeId {
        let node = if assertion {
            self.factory.new_as_expression(Some(expr), ty)
        } else {
            self.factory.new_satisfies_expression(Some(expr), ty)
        };
        let loc = self.factory.node(expr).range();
        self.finish_node_with_end(node, loc.pos(), loc.end())
    }
    /// port: tsc/internal/parser/reparser.go:Parser.createExportModifier
    fn create_export_modifier(&mut self, location: NodeId) -> NodeListId {
        let node = self.factory.new_modifier(K::ExportKeyword.into());
        self.finish_reparsed_node(node, location);
        self.new_modifier_list(self.factory.node(location).range(), vec![node])
    }
    /// port: tsc/internal/parser/reparser.go:Parser.getInnermostNameOfJSDocNamespace
    fn get_innermost_name_of_jsdoc_namespace(&self, name: Option<NodeId>) -> Option<NodeId> {
        let mut name = name?;
        while self.factory.node(name).kind() == K::ModuleDeclaration {
            let n = self.factory.node(name);
            let d = n.data_source().as_module_declaration().expect("namespace");
            let Some(body) = d.body() else {
                return d.name();
            };
            name = body;
        }
        Some(name)
    }
    /// port: tsc/internal/parser/reparser.go:Parser.wrapInJSDocNamespace
    fn wrap_in_jsdoc_namespace(
        &mut self,
        name: Option<NodeId>,
        statement: NodeId,
        nested: bool,
    ) -> NodeId {
        crate::recursion::guarded(|| {
            let Some(name) =
                name.filter(|&id| self.factory.node(id).kind() == K::ModuleDeclaration)
            else {
                return statement;
            };
            let body = self
                .factory
                .node(name)
                .data_source()
                .as_module_declaration()
                .expect("namespace")
                .body();
            let wrapped = self.wrap_in_jsdoc_namespace(body, statement, true);
            let list = self.new_node_list(self.factory.node(name).range(), vec![wrapped]);
            let block = self.factory.new_module_block(Some(list));
            self.finish_reparsed_node(block, name);
            let modifiers = nested.then(|| self.create_export_modifier(name));
            let cloned_name = self.add_deep_clone_reparse(self.jsdoc_name(name));
            let node = self.factory.new_module_declaration(
                modifiers,
                K::NamespaceKeyword.into(),
                cloned_name,
                None,
                Some(block),
            );
            self.finish_reparsed_node(node, name);
            self.reparsed_clones.push(node);
            node
        })
    }
}

macro_rules! function_field {
    ($data:expr, $field:ident) => {
        match $data {
            NodeDataRead::FunctionDeclaration(d) => d.$field(),
            NodeDataRead::FunctionExpression(d) => d.$field(),
            NodeDataRead::ArrowFunction(d) => d.$field(),
            NodeDataRead::MethodDeclaration(d) => d.$field(),
            NodeDataRead::ConstructorDeclaration(d) => d.$field(),
            NodeDataRead::GetAccessorDeclaration(d) => d.$field(),
            NodeDataRead::SetAccessorDeclaration(d) => d.$field(),
            NodeDataRead::CallSignatureDeclaration(d) => d.$field(),
            NodeDataRead::ConstructSignatureDeclaration(d) => d.$field(),
            NodeDataRead::IndexSignatureDeclaration(d) => d.$field(),
            NodeDataRead::MethodSignatureDeclaration(d) => d.$field(),
            NodeDataRead::FunctionTypeNode(d) => d.$field(),
            NodeDataRead::ConstructorTypeNode(d) => d.$field(),
            NodeDataRead::JSDocSignature(d) => d.$field(),
            _ => None,
        }
    };
}
macro_rules! set_function_field {
    ($data:expr, $field:ident, $value:expr) => {
        match $data {
            NodeData::FunctionDeclaration(d) => d.$field = $value,
            NodeData::FunctionExpression(d) => d.$field = $value,
            NodeData::ArrowFunction(d) => d.$field = $value,
            NodeData::MethodDeclaration(d) => d.$field = $value,
            NodeData::ConstructorDeclaration(d) => d.$field = $value,
            NodeData::GetAccessorDeclaration(d) => d.$field = $value,
            NodeData::SetAccessorDeclaration(d) => d.$field = $value,
            NodeData::CallSignatureDeclaration(d) => d.$field = $value,
            NodeData::ConstructSignatureDeclaration(d) => d.$field = $value,
            NodeData::IndexSignatureDeclaration(d) => d.$field = $value,
            NodeData::MethodSignatureDeclaration(d) => d.$field = $value,
            NodeData::FunctionTypeNode(d) => d.$field = $value,
            NodeData::ConstructorTypeNode(d) => d.$field = $value,
            NodeData::JSDocSignature(d) => d.$field = $value,
            _ => panic!("node has no function-like data"),
        }
    };
}
impl<F: ParserFactory> Parser<'_, F> {
    fn reparse_function_fields(
        &self,
        node: NodeId,
    ) -> (Option<NodeListId>, Option<NodeListId>, Option<NodeId>) {
        let n = self.factory.node(node);
        (
            function_field!(n.data(), type_parameters),
            function_field!(n.data(), parameters),
            function_field!(n.data(), full_signature),
        )
    }
    fn set_reparse_type_parameters(&mut self, node: NodeId, list: Option<NodeListId>) {
        match self.factory.node_mut(node).data_mut() {
            NodeData::TypeAliasDeclaration(d) => d.type_parameters = list,
            NodeData::ClassDeclaration(d) => d.type_parameters = list,
            NodeData::ClassExpression(d) => d.type_parameters = list,
            data => set_function_field!(data, type_parameters, list),
        }
    }
    fn set_reparse_parameters(&mut self, node: NodeId, list: Option<NodeListId>) {
        set_function_field!(self.factory.node_mut(node).data_mut(), parameters, list);
    }
    fn set_reparse_type(&mut self, node: NodeId, ty: Option<NodeId>) {
        match self.factory.node_mut(node).data_mut() {
            NodeData::TypeAliasDeclaration(d) => d.r#type = ty,
            NodeData::ParameterDeclaration(d) => d.r#type = ty,
            NodeData::PropertySignatureDeclaration(d) => d.r#type = ty,
            NodeData::PropertyDeclaration(d) => d.r#type = ty,
            NodeData::PropertyAssignment(d) => d.r#type = ty,
            NodeData::ShorthandPropertyAssignment(d) => d.r#type = ty,
            NodeData::VariableDeclaration(d) => d.r#type = ty,
            NodeData::ExportAssignment(d) => d.r#type = ty,
            NodeData::BinaryExpression(d) => d.r#type = ty,
            data => set_function_field!(data, r#type, ty),
        }
    }
    fn reparse_expression(&self, node: NodeId) -> Option<NodeId> {
        match self.factory.node(node).data() {
            NodeDataRead::ExpressionStatement(d) => d.expression(),
            NodeDataRead::ExportAssignment(d) => d.expression(),
            NodeDataRead::ReturnStatement(d) => d.expression(),
            NodeDataRead::ParenthesizedExpression(d) => d.expression(),
            NodeDataRead::SatisfiesExpression(d) => d.expression(),
            NodeDataRead::PropertyAccessExpression(d) => d.expression(),
            NodeDataRead::ElementAccessExpression(d) => d.expression(),
            _ => None,
        }
    }
    fn set_reparse_expression(&mut self, node: NodeId, expression: Option<NodeId>) {
        match self.factory.node_mut(node).data_mut() {
            NodeData::ReturnStatement(d) => d.expression = expression,
            NodeData::ParenthesizedExpression(d) => d.expression = expression,
            NodeData::ExportAssignment(d) => d.expression = expression,
            _ => panic!("unsupported expression setter"),
        }
    }
    fn reparse_initializer(&self, node: NodeId) -> Option<NodeId> {
        match self.factory.node(node).data() {
            NodeDataRead::VariableDeclaration(d) => d.initializer(),
            NodeDataRead::PropertyDeclaration(d) => d.initializer(),
            NodeDataRead::PropertyAssignment(d) => d.initializer(),
            NodeDataRead::ShorthandPropertyAssignment(d) => d.object_assignment_initializer(),
            _ => None,
        }
    }
    fn set_reparse_initializer(&mut self, node: NodeId, initializer: Option<NodeId>) {
        match self.factory.node_mut(node).data_mut() {
            NodeData::VariableDeclaration(d) => d.initializer = initializer,
            NodeData::PropertyDeclaration(d) => d.initializer = initializer,
            NodeData::PropertyAssignment(d) => d.initializer = initializer,
            NodeData::ShorthandPropertyAssignment(d) => {
                d.object_assignment_initializer = initializer;
            }
            _ => panic!("unsupported initializer setter"),
        }
    }
    fn reparse_tag_name(&self, tag: NodeId) -> NodeId {
        match self.factory.node(tag).data() {
            NodeDataRead::JSDocOverloadTag(d) => d.tag_name(),
            NodeDataRead::JSDocThisTag(d) => d.tag_name(),
            _ => panic!("tag name unavailable"),
        }
        .expect("tag name")
    }
    fn reparse_list_nodes(&self, list: Option<NodeListId>) -> ts_ast::NodeSlice {
        list.map_or_else(Default::default, |id| self.factory.read_list(id).nodes())
    }
    fn reparse_declarations(&self, node: NodeId) -> ts_ast::NodeSlice {
        let list = self
            .factory
            .node(node)
            .data_source()
            .as_variable_statement()
            .expect("variable statement")
            .declaration_list();
        list.map_or_else(Default::default, |list| {
            let list = self
                .factory
                .node(list)
                .data_source()
                .as_variable_declaration_list()
                .expect("declaration list")
                .declarations();
            self.reparse_list_nodes(list)
        })
    }
    /// port: tsc/internal/parser/reparser.go:skipSatisfiesExpressions
    fn skip_satisfies_expressions(&self, mut node: Option<NodeId>) -> Option<NodeId> {
        while node.is_some_and(|id| self.factory.node(id).kind() == K::SatisfiesExpression) {
            node = self.reparse_expression(node.expect("checked expression"));
        }
        node
    }
    /// port: tsc/internal/parser/reparser.go:getFunctionLikeHost
    fn get_function_like_host(&self, host: NodeId) -> Option<NodeId> {
        let mut fun = Some(host);
        match self.factory.node(host).kind().known() {
            Some(K::VariableStatement) => {
                // Source dereferences DeclarationList even when absent.
                self.factory
                    .node(host)
                    .data_source()
                    .as_variable_statement()
                    .expect("variable statement")
                    .declaration_list()
                    .expect("variable declaration list");
                let nodes = self.reparse_declarations(host);
                if !nodes.is_empty() {
                    fun = self.reparse_initializer(
                        self.factory
                            .read_nodes(nodes)
                            .at(0)
                            .expect("variable declaration"),
                    );
                }
            }
            Some(K::PropertyAssignment | K::PropertyDeclaration) => {
                fun = self.reparse_initializer(host);
            }
            Some(K::ExportAssignment | K::ReturnStatement) => fun = self.reparse_expression(host),
            Some(K::ExpressionStatement) => {
                fun = Some(self.get_right_most_assigned_expression(self.reparse_expression(host)));
            }
            _ => {}
        }
        self.skip_satisfies_expressions(fun)
            .filter(|&node| is_function_like_kind(self.factory.node(node).kind()))
    }
    /// port: tsc/internal/parser/reparser.go:findMatchingParameter
    fn find_matching_parameter(
        &self,
        fun: NodeId,
        parameter_tag: NodeId,
        doc: NodeId,
    ) -> Option<NodeId> {
        let tags = self
            .factory
            .node(doc)
            .data_source()
            .as_js_doc()
            .expect("JSDoc")
            .tags();
        let tags = self.reparse_list_nodes(tags);
        let mut index = -1;
        let mut count = -1;
        for tag in self.factory.read_nodes(tags).iter().flatten() {
            if self.factory.node(tag).kind() == K::JSDocParameterTag {
                count += 1;
                if tag == parameter_tag {
                    index = count;
                    break;
                }
            }
        }
        let parameters = self.reparse_list_nodes(self.reparse_function_fields(fun).1);
        let tag_name = self.jsdoc_name(parameter_tag).expect("parameter tag name");
        for (i, param) in self.factory.read_nodes(parameters).iter().enumerate() {
            let param = param.expect("parameter");
            let name = self.jsdoc_name(param).expect("parameter name");
            if self.factory.node(name).kind() == K::Identifier {
                if self.factory.node(tag_name).kind() == K::Identifier
                    && (self.jsdoc_text(name) == self.jsdoc_text(tag_name)
                        || i as i64 == index && self.jsdoc_text(tag_name).is_empty())
                {
                    return Some(param);
                }
            } else if i as i64 == index {
                return Some(param);
            }
        }
        None
    }
    /// port: tsc/internal/parser/reparser.go:getClassLikeData
    fn get_class_like_data(&self, node: NodeId) -> Option<ClassLikeFields> {
        match self.factory.node(node).data() {
            NodeDataRead::ClassDeclaration(d)
                if self.factory.node(node).kind() == K::ClassDeclaration =>
            {
                Some(ClassLikeFields {
                    heritage: d.heritage_clauses(),
                })
            }
            NodeDataRead::ClassExpression(d)
                if self.factory.node(node).kind() == K::ClassExpression =>
            {
                Some(ClassLikeFields {
                    heritage: d.heritage_clauses(),
                })
            }
            _ => None,
        }
    }
    fn set_reparse_heritage(&mut self, node: NodeId, clauses: Option<NodeListId>) {
        match self.factory.node_mut(node).data_mut() {
            NodeData::ClassDeclaration(d) => d.heritage_clauses = clauses,
            NodeData::ClassExpression(d) => d.heritage_clauses = clauses,
            _ => panic!("class payload"),
        }
    }
    fn append_reparse_list(&mut self, list: NodeListId, node: NodeId) {
        let nodes = self.factory.read_list(list).nodes();
        let mut result = self.factory.read_nodes(nodes).iter().collect::<Vec<_>>();
        result.push(Some(node));
        let nodes = self.factory.alloc_nodes(result);
        self.factory.set_list_nodes(list, nodes);
    }
    /// port: tsc/internal/parser/reparser.go:Parser.reparseHosted
    fn reparse_hosted(&mut self, tag: NodeId, mut parent: NodeId, doc: NodeId) {
        match self.factory.node(tag).kind().known() {
            Some(K::JSDocTypeTag) => {
                match self.factory.node(parent).kind().known() {
                    Some(K::VariableStatement) => {
                        let nodes = self.reparse_declarations(parent);
                        for i in 0..nodes.len() {
                            let node = self.factory.read_nodes(nodes).at(i).expect("declaration");
                            if self.jsdoc_type(node).is_none() {
                                if let Some(expr) = self.jsdoc_type(tag) {
                                    let ty = self.add_deep_clone_reparse(self.jsdoc_type(expr));
                                    self.set_reparse_type(node, ty);
                                    self.finish_mutated_node(node);
                                    return;
                                }
                            }
                        }
                    }
                    Some(
                        K::VariableDeclaration
                        | K::ExportAssignment
                        | K::PropertyDeclaration
                        | K::PropertyAssignment
                        | K::ShorthandPropertyAssignment
                        | K::GetAccessor
                        | K::Parameter,
                    ) => {
                        if self.jsdoc_type(parent).is_none() {
                            if let Some(expr) = self.jsdoc_type(tag) {
                                let ty = if self.factory.node(parent).kind() == K::Parameter {
                                    self.reparse_jsdoc_type_literal(self.jsdoc_type(expr))
                                } else {
                                    self.add_deep_clone_reparse(self.jsdoc_type(expr))
                                };
                                self.set_reparse_type(parent, ty);
                                self.finish_mutated_node(parent);
                                return;
                            }
                        }
                    }
                    Some(K::ExpressionStatement) => {
                        let expr = self
                            .reparse_expression(parent)
                            .expect("expression statement expression");
                        if self.factory.node(expr).kind() == K::BinaryExpression
                            && self.is_binary_assignment_declaration(expr)
                        {
                            if let Some(ty) = self.jsdoc_type(tag) {
                                let ty = self.add_deep_clone_reparse(self.jsdoc_type(ty));
                                self.set_reparse_type(expr, ty);
                                self.finish_mutated_node(expr);
                                return;
                            }
                        }
                    }
                    Some(K::ReturnStatement | K::ParenthesizedExpression) => {
                        if let (Some(expr), Some(ty)) =
                            (self.reparse_expression(parent), self.jsdoc_type(tag))
                        {
                            let ty = self.add_deep_clone_reparse(self.jsdoc_type(ty));
                            let cast = self.make_new_cast(ty, expr, true);
                            self.set_reparse_expression(parent, Some(cast));
                            self.finish_mutated_node(parent);
                            return;
                        }
                    }
                    _ => {}
                }
                if let Some(fun) = self.get_function_like_host(parent) {
                    let (types, params, _) = self.reparse_function_fields(fun);
                    let nodes = self.reparse_list_nodes(params);
                    let no_types = self
                        .factory
                        .read_nodes(nodes)
                        .iter()
                        .flatten()
                        .all(|p| self.jsdoc_type(p).is_none());
                    if types.is_none() && self.jsdoc_type(fun).is_none() && no_types {
                        if let Some(expr) = self.jsdoc_type(tag) {
                            let ty = self.add_deep_clone_reparse(self.jsdoc_type(expr));
                            set_function_field!(
                                self.factory.node_mut(fun).data_mut(),
                                full_signature,
                                ty
                            );
                            self.finish_mutated_node(fun);
                        }
                    }
                }
            }
            Some(K::JSDocSatisfiesTag) => match self.factory.node(parent).kind().known() {
                Some(K::VariableStatement) => {
                    let nodes = self.reparse_declarations(parent);
                    for i in 0..nodes.len() {
                        let node = self.factory.read_nodes(nodes).at(i).expect("declaration");
                        if let (Some(expr), Some(ty)) =
                            (self.reparse_initializer(node), self.jsdoc_type(tag))
                        {
                            let ty = self.add_deep_clone_reparse(self.jsdoc_type(ty));
                            let cast = self.make_new_cast(ty, expr, false);
                            self.set_reparse_initializer(node, Some(cast));
                            self.finish_mutated_node(node);
                            break;
                        }
                    }
                }
                Some(
                    K::VariableDeclaration
                    | K::PropertyDeclaration
                    | K::PropertyAssignment
                    | K::ShorthandPropertyAssignment,
                ) => {
                    if let (Some(expr), Some(ty)) =
                        (self.reparse_initializer(parent), self.jsdoc_type(tag))
                    {
                        let ty = self.add_deep_clone_reparse(self.jsdoc_type(ty));
                        let cast = self.make_new_cast(ty, expr, false);
                        self.set_reparse_initializer(parent, Some(cast));
                        self.finish_mutated_node(parent);
                    }
                }
                Some(K::ReturnStatement | K::ParenthesizedExpression | K::ExportAssignment) => {
                    if let (Some(expr), Some(ty)) =
                        (self.reparse_expression(parent), self.jsdoc_type(tag))
                    {
                        let ty = self.add_deep_clone_reparse(self.jsdoc_type(ty));
                        let cast = self.make_new_cast(ty, expr, false);
                        self.set_reparse_expression(parent, Some(cast));
                        self.finish_mutated_node(parent);
                    }
                }
                Some(K::ExpressionStatement) => {
                    let expr = self
                        .reparse_expression(parent)
                        .expect("expression statement expression");
                    if self.factory.node(expr).kind() == K::BinaryExpression
                        && self.is_binary_assignment_declaration(expr)
                    {
                        if let Some(ty) = self.jsdoc_type(tag) {
                            let right = self
                                .factory
                                .node(expr)
                                .data_source()
                                .as_binary_expression()
                                .expect("binary expression")
                                .right()
                                .expect("right operand");
                            let ty = self.add_deep_clone_reparse(self.jsdoc_type(ty));
                            let cast = self.make_new_cast(ty, right, false);
                            if let NodeData::BinaryExpression(d) =
                                self.factory.node_mut(expr).data_mut()
                            {
                                d.right = Some(cast);
                            } else {
                                panic!("binary expression");
                            }
                            self.finish_mutated_node(expr);
                        }
                    }
                }
                _ => {}
            },
            Some(K::JSDocTemplateTag) => {
                if let Some(fun) = self.get_function_like_host(parent) {
                    let (types, _, full) = self.reparse_function_fields(fun);
                    if self.reparse_list_nodes(types).is_nil() && full.is_none() {
                        let types = self.gather_type_parameters(doc, false);
                        self.set_reparse_type_parameters(fun, types);
                        self.finish_mutated_node(fun);
                    }
                } else {
                    let types = match self.factory.node(parent).data() {
                        NodeDataRead::ClassDeclaration(d) => Some(d.type_parameters()),
                        NodeDataRead::ClassExpression(d) => Some(d.type_parameters()),
                        _ => None,
                    };
                    if types == Some(None) {
                        let types = self.gather_type_parameters(doc, false);
                        self.set_reparse_type_parameters(parent, types);
                        self.finish_mutated_node(parent);
                    }
                }
            }
            Some(K::JSDocParameterTag) => {
                if let Some(fun) = self
                    .get_function_like_host(parent)
                    .filter(|&id| self.reparse_function_fields(id).2.is_none())
                {
                    if let Some(param) = self.find_matching_parameter(fun, tag, doc) {
                        if self.jsdoc_type(param).is_none() {
                            if let Some(expr) = self.jsdoc_type(tag) {
                                let ty = self.reparse_jsdoc_type_literal(self.jsdoc_type(expr));
                                self.set_reparse_type(param, ty);
                            }
                        }
                        if self
                            .factory
                            .node(param)
                            .data_source()
                            .as_parameter_declaration()
                            .expect("parameter")
                            .question_token()
                            .is_none()
                        {
                            if let Some(question) = self.make_question_if_optional(tag) {
                                if let NodeData::ParameterDeclaration(d) =
                                    self.factory.node_mut(param).data_mut()
                                {
                                    d.question_token = Some(question);
                                } else {
                                    panic!("parameter");
                                }
                            }
                        }
                        self.finish_mutated_node(param);
                    }
                }
            }
            Some(K::JSDocThisTag) => {
                if let Some(fun) = self.get_function_like_host(parent) {
                    let list = self
                        .reparse_function_fields(fun)
                        .1
                        .expect("function parameter list");
                    let nodes = self.factory.read_list(list).nodes();
                    let has_this = !nodes.is_empty() && {
                        let first = self.factory.read_nodes(nodes).at(0).expect("parameter");
                        let name = self.jsdoc_name(first).expect("parameter name");
                        self.factory.node(name).kind() == K::ThisKeyword
                            || self.factory.node(name).kind() == K::Identifier
                                && self.jsdoc_text(name).as_bytes() == b"this"
                    };
                    if !has_this {
                        let name = self
                            .factory
                            .new_identifier(JsString::from_bytes(b"this".as_slice()));
                        let param = self.factory.new_parameter_declaration(
                            None,
                            None,
                            Some(name),
                            None,
                            None,
                            None,
                        );
                        if let Some(expr) = self.jsdoc_type(tag) {
                            let ty = self.add_deep_clone_reparse(self.jsdoc_type(expr));
                            self.set_reparse_type(param, ty);
                        }
                        self.finish_reparsed_node(param, self.reparse_tag_name(tag));
                        let mut params = Vec::with_capacity(nodes.len() + 1);
                        params.push(param);
                        params.extend(
                            self.factory
                                .read_nodes(nodes)
                                .iter()
                                .map(|p| p.expect("parameter")),
                        );
                        let list = self.new_node_list(self.factory.read_list(list).loc(), params);
                        self.set_reparse_parameters(fun, Some(list));
                        self.finish_mutated_node(fun);
                    }
                }
            }
            Some(K::JSDocReturnTag) => {
                if let Some(fun) = self
                    .get_function_like_host(parent)
                    .filter(|&id| self.reparse_function_fields(id).2.is_none())
                {
                    if self.jsdoc_type(fun).is_none() {
                        if let Some(expr) = self.jsdoc_type(tag) {
                            let ty = self.add_deep_clone_reparse(self.jsdoc_type(expr));
                            self.set_reparse_type(fun, ty);
                            self.finish_mutated_node(fun);
                        }
                    }
                }
            }
            Some(
                K::JSDocReadonlyTag
                | K::JSDocPrivateTag
                | K::JSDocPublicTag
                | K::JSDocProtectedTag
                | K::JSDocOverrideTag,
            ) => {
                if self.factory.node(parent).kind() == K::ExpressionStatement {
                    parent = self
                        .reparse_expression(parent)
                        .expect("expression statement");
                }
                if matches!(
                    self.factory.node(parent).kind().known(),
                    Some(K::MethodDeclaration | K::GetAccessor | K::SetAccessor)
                ) && self.parsing_contexts & (1 << ParsingContext::ObjectLiteralMembers as u32)
                    != 0
                {
                    return;
                }
                if matches!(
                    self.factory.node(parent).kind().known(),
                    Some(
                        K::MethodDeclaration
                            | K::GetAccessor
                            | K::SetAccessor
                            | K::PropertyDeclaration
                            | K::Constructor
                            | K::BinaryExpression
                    )
                ) {
                    let kind = match self.factory.node(tag).kind().known() {
                        Some(K::JSDocReadonlyTag) => K::ReadonlyKeyword,
                        Some(K::JSDocPrivateTag) => K::PrivateKeyword,
                        Some(K::JSDocPublicTag) => K::PublicKeyword,
                        Some(K::JSDocProtectedTag) => K::ProtectedKeyword,
                        _ => K::OverrideKeyword,
                    };
                    let modifier = self.factory.new_modifier(kind.into());
                    self.finish_reparsed_node(modifier, tag);
                    let (mut nodes, loc) = self.node_modifiers(parent).map_or_else(
                        || (Vec::new(), self.factory.node(tag).range()),
                        |list| {
                            let list = self.factory.read_list(list);
                            (
                                self.factory
                                    .read_nodes(list.nodes())
                                    .iter()
                                    .map(|p| p.expect("modifier"))
                                    .collect(),
                                list.loc(),
                            )
                        },
                    );
                    nodes.push(modifier);
                    let list = self.new_modifier_list(loc, nodes);
                    match self.factory.node_mut(parent).data_mut() {
                        NodeData::MethodDeclaration(d) => d.modifiers = Some(list),
                        NodeData::GetAccessorDeclaration(d) => d.modifiers = Some(list),
                        NodeData::SetAccessorDeclaration(d) => d.modifiers = Some(list),
                        NodeData::PropertyDeclaration(d) => d.modifiers = Some(list),
                        NodeData::ConstructorDeclaration(d) => d.modifiers = Some(list),
                        NodeData::BinaryExpression(d) => d.modifiers = Some(list),
                        _ => unreachable!(),
                    }
                    self.finish_mutated_node(parent);
                }
            }
            Some(K::JSDocImplementsTag) => {
                if let Some(ClassLikeFields { heritage: clauses }) =
                    self.get_class_like_data(parent)
                {
                    let class_name = self
                        .factory
                        .node(tag)
                        .data_source()
                        .as_js_doc_implements_tag()
                        .expect("implements tag")
                        .class_name()
                        .expect("implements class");
                    if let Some(clause) = self.find_heritage_clause(clauses, K::ImplementsKeyword) {
                        let types = self
                            .factory
                            .node(clause)
                            .data_source()
                            .as_heritage_clause()
                            .expect("heritage clause")
                            .types()
                            .expect("heritage types");
                        let cloned = self
                            .add_deep_clone_reparse(Some(class_name))
                            .expect("class clone");
                        self.append_reparse_list(types, cloned);
                        self.finish_mutated_node(clause);
                        return;
                    }
                    let cloned = self
                        .add_deep_clone_reparse(Some(class_name))
                        .expect("class clone");
                    let list =
                        self.new_node_list(self.factory.node(class_name).range(), vec![cloned]);
                    let clause = self
                        .factory
                        .new_heritage_clause(K::ImplementsKeyword.into(), Some(list));
                    self.finish_reparsed_node(clause, class_name);
                    if let Some(clauses) = clauses {
                        self.append_reparse_list(clauses, clause);
                    } else {
                        let list =
                            self.new_node_list(self.factory.node(class_name).range(), vec![clause]);
                        self.set_reparse_heritage(parent, Some(list));
                    }
                    self.finish_mutated_node(parent);
                }
            }
            Some(K::JSDocAugmentsTag) => {
                if let Some(ClassLikeFields {
                    heritage: Some(clauses),
                }) = self.get_class_like_data(parent)
                {
                    if let Some(clause) =
                        self.find_heritage_clause(Some(clauses), K::ExtendsKeyword)
                    {
                        let list = self
                            .factory
                            .node(clause)
                            .data_source()
                            .as_heritage_clause()
                            .expect("heritage clause")
                            .types();
                        let nodes = self.reparse_list_nodes(list);
                        if nodes.len() == 1 {
                            let target =
                                self.factory.read_nodes(nodes).at(0).expect("heritage type");
                            let source = self
                                .factory
                                .node(tag)
                                .data_source()
                                .as_js_doc_augments_tag()
                                .expect("augments tag")
                                .class_name()
                                .expect("augments class");
                            let (target_expr, target_types) =
                                self.reparse_expression_with_type_arguments(target);
                            let (source_expr, source_types) =
                                self.reparse_expression_with_type_arguments(source);
                            if self.has_same_property_access_name(target_expr, source_expr)
                                && target_types.is_none()
                            {
                                if let Some(source_types) = source_types {
                                    let list = self.factory.read_list(source_types);
                                    let loc = list.loc();
                                    let nodes = list.nodes();
                                    drop(list);
                                    let mut result = Vec::with_capacity(nodes.len());
                                    for i in 0..nodes.len() {
                                        let node = self.factory.read_nodes(nodes).at(i);
                                        result.push(
                                            self.add_deep_clone_reparse(node)
                                                .expect("cloned type argument"),
                                        );
                                    }
                                    let list = self.new_node_list(loc, result);
                                    if let NodeData::ExpressionWithTypeArguments(d) =
                                        self.factory.node_mut(target).data_mut()
                                    {
                                        d.type_arguments = Some(list);
                                    } else {
                                        panic!("expression with type arguments");
                                    }
                                    self.finish_mutated_node(target);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    fn find_heritage_clause(&self, clauses: Option<NodeListId>, kind: K) -> Option<NodeId> {
        let nodes = self.reparse_list_nodes(clauses);
        self.factory.read_nodes(nodes).iter().flatten().find(|&id| {
            self.factory
                .node(id)
                .data_source()
                .as_heritage_clause()
                .expect("heritage clause")
                .token()
                == kind
        })
    }
    fn reparse_expression_with_type_arguments(&self, node: NodeId) -> (NodeId, Option<NodeListId>) {
        let n = self.factory.node(node);
        let d = n
            .data_source()
            .as_expression_with_type_arguments()
            .expect("expression with type arguments");
        (
            d.expression().expect("type argument expression"),
            d.type_arguments(),
        )
    }
}

/// port: tsc/internal/ast/utilities.go:IsFunctionLikeKind
fn is_function_like_kind(kind: ts_ast::NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::MethodSignature
                | K::CallSignature
                | K::JSDocSignature
                | K::ConstructSignature
                | K::IndexSignature
                | K::FunctionType
                | K::ConstructorType
        )
    ) || is_function_like_declaration_kind(kind)
}
/// port: tsc/internal/ast/utilities.go:isFunctionLikeDeclarationKind
fn is_function_like_declaration_kind(kind: ts_ast::NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::FunctionDeclaration
                | K::MethodDeclaration
                | K::Constructor
                | K::GetAccessor
                | K::SetAccessor
                | K::FunctionExpression
                | K::ArrowFunction
        )
    )
}
impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/ast/utilities.go:HasSamePropertyAccessName
    fn has_same_property_access_name(&self, mut a: NodeId, mut b: NodeId) -> bool {
        loop {
            if self.factory.node(a).kind() == K::Identifier
                && self.factory.node(b).kind() == K::Identifier
            {
                return self.jsdoc_text(a) == self.jsdoc_text(b);
            }
            if self.factory.node(a).kind() != K::PropertyAccessExpression
                || self.factory.node(b).kind() != K::PropertyAccessExpression
            {
                return false;
            }
            if self.jsdoc_text(self.jsdoc_name(a).expect("property name"))
                != self.jsdoc_text(self.jsdoc_name(b).expect("property name"))
            {
                return false;
            }
            a = self.reparse_expression(a).expect("property target");
            b = self.reparse_expression(b).expect("property target");
        }
    }
    /// port: tsc/internal/ast/utilities.go:GetRightMostAssignedExpression
    fn get_right_most_assigned_expression(&self, node: Option<NodeId>) -> NodeId {
        let mut node = node.expect("nil assignment expression");
        loop {
            let view = self.factory.node(node);
            if view.kind() != K::BinaryExpression {
                break;
            }
            let data = view
                .data_source()
                .as_binary_expression()
                .expect("binary expression");
            if !ts_ast::is_assignment_operator(
                self.factory
                    .node(data.operator_token().expect("binary operator"))
                    .kind(),
            ) {
                break;
            }
            let mut left = data.left().expect("left operand");
            while self.factory.node(left).kind() == K::PartiallyEmittedExpression {
                left = self
                    .factory
                    .node(left)
                    .data_source()
                    .as_partially_emitted_expression()
                    .expect("partially emitted expression")
                    .expression()
                    .expect("partially emitted inner");
            }
            if !ts_ast::is_left_hand_side_expression_kind(self.factory.node(left).kind()) {
                break;
            }
            node = data.right().expect("assignment right operand");
        }
        node
    }
    // Only the BinaryExpression arm of GetAssignmentDeclarationKind is needed
    // here. This boolean projection deliberately claims no full-function port.
    fn is_binary_assignment_declaration(&self, node: NodeId) -> bool {
        let node = self.factory.node(node);
        let data = node
            .data_source()
            .as_binary_expression()
            .expect("binary expression");
        if self
            .factory
            .node(data.operator_token().expect("binary operator"))
            .kind()
            != K::EqualsToken
        {
            return false;
        }
        let left = data.left().expect("left operand");
        if !matches!(
            self.factory.node(left).kind().known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) {
            return false;
        }
        let js = self.factory.node(left).flags() & node_flags::JAVA_SCRIPT_FILE != 0;
        let target = self.reparse_expression(left).expect("access target");
        if js {
            if self.reparse_is_module_exports_access(left)
                && !self.reparse_is_identifier(data.right().expect("right operand"), b"exports")
            {
                return true;
            }
            if (self.reparse_is_module_exports_access(target)
                || self.reparse_is_identifier(target, b"exports"))
                && self.reparse_access_name(left).is_some()
            {
                return true;
            }
            if self.factory.node(target).kind() == K::ThisKeyword {
                return true;
            }
        }
        self.reparse_is_entity_name_expression(target, js)
            && (self.factory.node(left).kind() == K::ElementAccessExpression
                || self
                    .jsdoc_name(left)
                    .is_some_and(|id| self.factory.node(id).kind() == K::Identifier))
    }
    fn reparse_is_identifier(&self, node: NodeId, text: &[u8]) -> bool {
        self.factory.node(node).kind() == K::Identifier && self.jsdoc_text(node).as_bytes() == text
    }
    fn reparse_is_module_exports_access(&self, node: NodeId) -> bool {
        matches!(
            self.factory.node(node).kind().known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) && self
            .reparse_expression(node)
            .is_some_and(|id| self.reparse_is_identifier(id, b"module"))
            && self
                .reparse_access_name(node)
                .is_some_and(|id| self.jsdoc_text(id).as_bytes() == b"exports")
    }
    fn reparse_access_name(&self, node: NodeId) -> Option<NodeId> {
        match self.factory.node(node).kind().known() {
            Some(K::PropertyAccessExpression) => self
                .jsdoc_name(node)
                .filter(|&id| self.factory.node(id).kind() == K::Identifier),
            Some(K::ElementAccessExpression) => {
                let mut arg = self
                    .factory
                    .node(node)
                    .data_source()
                    .as_element_access_expression()
                    .expect("element access")
                    .argument_expression()
                    .expect("element argument");
                while self.factory.node(arg).kind() == K::ParenthesizedExpression {
                    arg = self
                        .reparse_expression(arg)
                        .expect("parenthesized expression");
                }
                self.reparse_is_literal_name(arg).then_some(arg)
            }
            _ => panic!("Unhandled case in GetElementOrPropertyAccessName"),
        }
    }
    fn reparse_is_literal_name(&self, node: NodeId) -> bool {
        matches!(
            self.factory.node(node).kind().known(),
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral | K::NumericLiteral)
        )
    }
    fn reparse_is_entity_name_expression(&self, mut node: NodeId, js: bool) -> bool {
        loop {
            match self.factory.node(node).kind().known() {
                Some(K::Identifier) => return true,
                Some(K::ThisKeyword) => return js,
                Some(K::PropertyAccessExpression) => {
                    if self
                        .jsdoc_name(node)
                        .is_none_or(|id| self.factory.node(id).kind() != K::Identifier)
                    {
                        return false;
                    }
                    node = self.reparse_expression(node).expect("property target");
                }
                Some(K::ElementAccessExpression) if js => {
                    let arg = self
                        .factory
                        .node(node)
                        .data_source()
                        .as_element_access_expression()
                        .expect("element access")
                        .argument_expression()
                        .expect("element argument");
                    if !self.reparse_is_literal_name(arg) {
                        return false;
                    }
                    node = self.reparse_expression(node).expect("element target");
                }
                _ => return false,
            }
        }
    }
}
