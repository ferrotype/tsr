use super::{tracker::Selector, transform::Transformer, util};
use std::collections::{HashMap, HashSet};
use ts_ast::{
    modifier_flags as mf, node_flags as nf, Factory, FactoryMethods, JsString, NodeId, NodeListId,
    RuntimeFactory, SyntaxKind as K,
};
use ts_printer::emit_resolver::DeclarationEmitResolver;

#[derive(Default)]
pub(super) struct CommonJsState {
    pub assignment: Option<NodeId>,
    pub assignment_name: Option<NodeId>,
    pub members: Vec<NodeId>,
    pub witnessed: HashSet<JsString>,
    pub expando_hosts: HashMap<NodeId, Option<NodeId>>,
    pub expando_members: HashMap<NodeId, Vec<NodeId>>,
    pub deferred_expando: HashMap<NodeId, Vec<NodeId>>,
}
pub(super) struct Context {
    selector: Selector,
    error_name: Option<NodeId>,
    suppress: bool,
}
impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    pub(super) fn save_expression_context(&mut self, node: NodeId) -> Result<Context, R::Error> {
        let context = Context {
            selector: self.tracker.selector.clone(),
            error_name: self.tracker.error_name,
            suppress: self.suppress_context,
        };
        if util::can_produce_diagnostics(&self.node(node)) && !self.suppress_context {
            self.select_context(node, false)?;
        }
        if matches!(
            self.node(node).kind().known(),
            Some(K::TypeLiteral | K::MappedType)
        ) && !self.node(node).parent().is_some_and(|p| {
            matches!(
                self.node(p).kind().known(),
                Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration)
            )
        }) {
            self.suppress_context = true;
        }
        Ok(context)
    }
    pub(super) fn restore_expression_context(&mut self, context: Context) {
        self.tracker.selector = context.selector;
        self.tracker.error_name = context.error_name;
        self.suppress_context = context.suppress;
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.visitCJSExportAssignments
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.visitNestedExpression
    pub fn collect_nested_exports(
        &mut self,
        source: NodeId,
        assignments: bool,
    ) -> Result<(), R::Error> {
        enum Frame {
            Enter(NodeId),
            Exit(Context),
        }
        let common_js = self
            .output
            .read_source_file(source)?
            .common_js_module_indicator()
            .is_some();
        let mut frames = vec![Frame::Enter(source)];
        while let Some(frame) = frames.pop() {
            let Frame::Enter(node) = frame else {
                if let Frame::Exit(context) = frame {
                    self.restore_expression_context(context);
                }
                continue;
            };
            let context = self.save_expression_context(node)?;
            let kind = ts_ast::get_assignment_declaration_kind(self.output.view(), node)?;
            if assignments {
                if common_js && kind == ts_ast::JSDeclarationKind::ModuleExports {
                    let right =
                        Self::required(self.node(node).as_binary_expression().unwrap().right())?;
                    let input = Self::required(self.node(node).parent())?;
                    let result = self.export_assignment_from(input, node, right, true)?;
                    self.cjs.assignment = Some(result);
                    self.has_scope_marker = true;
                    self.external_indicator = true;
                }
            } else {
                match kind {
                    ts_ast::JSDeclarationKind::Property => self.expando_assignment(node)?,
                    ts_ast::JSDeclarationKind::ExportsProperty if common_js => {
                        let left =
                            Self::required(self.node(node).as_binary_expression().unwrap().left())?;
                        let name = Self::required(ts_ast::get_element_or_property_access_name(
                            self.output.view(),
                            left,
                        )?)?;
                        let name = self.preferred_export_name(name)?;
                        if let Some(result) = self.common_js_export(node, name)? {
                            self.cjs.members.push(result);
                        }
                    }
                    ts_ast::JSDeclarationKind::ObjectDefinePropertyExports if common_js => {
                        let args = self.list_nodes(self.node(node).argument_list());
                        let name = *args.get(1).ok_or(ts_arena::Error::InvalidGraph)?;
                        let name = self.preferred_export_name(name)?;
                        if let Some(result) = self.common_js_export(node, name)? {
                            self.cjs.members.push(result);
                        }
                    }
                    _ => {}
                }
            }
            frames.push(Frame::Exit(context));
            frames.extend(
                self.assignment_children(node)?
                    .into_iter()
                    .rev()
                    .map(Frame::Enter),
            );
        }
        Ok(())
    }
    // The preferred output name retains its original parent for the native
    // reference lookup. Its lookup itself runs through the checker-owned proxy.
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.getNameExpressionPreferringIdentifier
    fn preferred_export_name(&mut self, mut name: NodeId) -> Result<NodeId, R::Error> {
        if self.node(name).kind() == K::NumericLiteral {
            let text = self.output.view().node_text(name)?.into_js_string();
            name = self.output.new_string_literal(text, 0);
        }
        if matches!(
            self.node(name).kind().known(),
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
        ) {
            let text = self.output.view().node_text(name)?.into_js_string();
            if ts_scanner::is_identifier_text(text.as_bytes(), ts_core::LanguageVariant::STANDARD) {
                let keyword = ts_scanner::string_to_token(text.as_bytes());
                if matches!(keyword, K::Unknown | K::DefaultKeyword) {
                    let parent = self.node(name).parent();
                    let result = self.output.new_identifier(text);
                    self.output.set_node_parent(result, parent);
                    self.output
                        .set_node_flags(result, self.node(result).flags() & !nf::SYNTHESIZED);
                    return Ok(result);
                }
            }
        }
        Ok(name)
    }
    pub fn modifier_list(&mut self, flags: u32) -> Option<NodeListId> {
        let nodes =
            ts_ast::utilities_middle::create_modifiers_from_modifier_flags(flags, |kind| {
                Some(self.output.new_modifier(kind))
            })?;
        let nodes = self.output.alloc_nodes(nodes);
        Some(self.output.new_modifier_list(nodes))
    }
    pub fn named_export(&mut self, local: Option<NodeId>, name: NodeId) -> NodeId {
        let specifier = self.output.new_export_specifier(false, local, Some(name));
        let list = self.new_list(vec![specifier]);
        let named = self.output.new_named_exports(Some(list));
        self.output
            .new_export_declaration(None, false, Some(named), None, None)
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformBinaryExpressionToExportDeclaration
    pub fn binary_export(&mut self, node: NodeId, name: NodeId) -> Result<NodeId, R::Error> {
        let right = Self::required(self.node(node).as_binary_expression().unwrap().right())?;
        self.entity_visible(right)?;
        let property = if self.node(name).kind() == K::Identifier
            && self.output.view().node_text(right)?.as_bytes()
                == self.output.view().node_text(name)?.as_bytes()
        {
            None
        } else {
            Some(right)
        };
        Ok(self.named_export(property, name))
    }
    fn common_js_export(&mut self, node: NodeId, name: NodeId) -> Result<Option<NodeId>, R::Error> {
        let result = self.common_js_export_worker(node, name)?;
        let Some(result) = result else {
            return Ok(None);
        };
        let Some(name) = self.cjs.assignment_name else {
            return Ok(Some(result));
        };
        let mut members = Vec::new();
        self.append_flat(result, &mut members);
        for member in &mut members {
            *member = self.strip_declare(*member)?;
        }
        let list = self.new_list(members);
        let block = self.output.new_module_block(Some(list));
        let modifiers = self.declare_modifiers();
        Ok(Some(self.output.new_module_declaration(
            modifiers,
            K::NamespaceKeyword.into(),
            Some(name),
            None,
            Some(block),
        )))
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.transformCommonJSExportWorker
    fn common_js_export_worker(
        &mut self,
        node: NodeId,
        name: NodeId,
    ) -> Result<Option<NodeId>, R::Error> {
        let name_text = if matches!(
            self.node(name).kind().known(),
            Some(K::Identifier | K::StringLiteral)
        ) {
            self.output.view().node_text(name)?.into_js_string()
        } else {
            JsString::default()
        };
        if !name_text.is_empty() && self.cjs.witnessed.contains(&name_text) {
            return Ok(None);
        }
        self.cjs.witnessed.insert(name_text.clone());
        self.external_indicator = true;
        self.has_scope_marker = true;
        if self.node(node).kind() == K::BinaryExpression {
            let right = Self::required(self.node(node).as_binary_expression().unwrap().right())?;
            let parent = self.node(node).parent();
            let alias = self.node(right).kind() == K::Identifier
                && if let Some(symbol) = self.resolver.bound_symbol_of_declaration(node)? {
                    self.resolver.symbol_declarations(symbol)?.len() == 1
                } else {
                    false
                };
            if alias
                && parent.is_some_and(|p| {
                    self.node(p).kind() == K::ExpressionStatement
                        && self
                            .node(p)
                            .parent()
                            .is_some_and(|p| self.node(p).kind() == K::SourceFile)
                })
            {
                return self.binary_export(node, name).map(Some);
            }
            let right = util::unwrap_parenthesized_expression(self.output.view(), right)?;
            if self.node(right).kind() == K::ClassExpression {
                return self.common_js_class(node, name, right).map(Some);
            }
        }
        let is_default =
            self.node(name).kind() == K::Identifier && name_text.as_bytes() == b"default";
        if self.node(name).kind() == K::Identifier && !is_default {
            let referenced = if name.arena() == self.output.id().arena() {
                match self.node(name).parent() {
                    Some(parent) => self
                        .resolver
                        .referenced_name_declaration(name_text, parent)?,
                    None => None,
                }
            } else {
                self.resolver.referenced_value_declaration(name)?
            };
            if referenced.is_none() || referenced == Some(node) {
                self.tracker.fallback.push(Some(node));
                let result = self.ensure_type(node, false);
                self.tracker.fallback.pop();
                let ty = result?;
                let declaration = self
                    .output
                    .new_variable_declaration(Some(name), None, ty, None);
                let declarations = self.new_list(vec![declaration]);
                let list = self
                    .output
                    .new_variable_declaration_list(Some(declarations), 0);
                let modifiers = self
                    .modifier_list(mf::EXPORT | if self.needs_declare { mf::AMBIENT } else { 0 });
                return Ok(Some(
                    self.output.new_variable_statement(modifiers, Some(list)),
                ));
            }
        }
        let local = self.unique_name(JsString::from_bytes(if is_default {
            b"_default".as_slice()
        } else {
            b"_exported".as_slice()
        }));
        self.tracker.selector =
            Selector::fixed(super::diagnostics::SymbolAccessibilityDiagnostic {
                diagnostic_message:
                    ts_diagnostics::Default_export_of_the_module_has_or_is_using_private_name_0,
                error_node: Some(node),
                type_name: None,
            });
        self.tracker.fallback.push(Some(node));
        let result = self.ensure_type(node, false);
        self.tracker.fallback.pop();
        let ty = result?;
        let declaration = self.const_variable(local, ty, None);
        let assignment = if is_default {
            self.output
                .new_export_assignment(self.node(node).modifiers(), false, None, Some(local))
        } else {
            self.named_export(Some(local), name)
        };
        self.emit
            .assign_comment_range(self.output, declaration, node);
        self.emit
            .add_emit_flags(assignment, ts_printer::emit_flags::NO_COMMENTS);
        Ok(Some(self.syntax_list(vec![declaration, assignment])))
    }
    fn common_js_class(
        &mut self,
        node: NodeId,
        name: NodeId,
        class: NodeId,
    ) -> Result<NodeId, R::Error> {
        let original_name = self.node(class).name();
        let name_text = original_name
            .map(|name| {
                self.output
                    .view()
                    .node_text(name)
                    .map(ts_ast::NodeText::into_js_string)
            })
            .transpose()?
            .filter(|name| !name.is_empty());
        if let Some(text) = name_text {
            self.tracker.watched_class = self.resolver.bound_symbol_of_declaration(class)?;
            self.tracker.class_tracked = false;
            let result = (|| {
                let class_name = self.output.new_identifier(text.clone());
                let modifiers = self.modifier_list(mf::EXPORT);
                let declaration =
                    self.class_expression_declaration(class, class_name, modifiers)?;
                self.emit
                    .assign_comment_range(self.output, declaration, node);
                if self.node(name).kind() != K::Identifier
                    || self.output.view().node_text(name)?.as_bytes() != text.as_bytes()
                    || self.tracker.class_tracked
                {
                    let namespace = self.unique_name(JsString::from_bytes(b"_ns".as_slice()));
                    let modifiers = self.declare_modifiers();
                    let statements = self.new_list(vec![declaration]);
                    let body = self.output.new_module_block(Some(statements));
                    let namespace_declaration = self.output.new_module_declaration(
                        modifiers,
                        K::NamespaceKeyword.into(),
                        Some(namespace),
                        None,
                        Some(body),
                    );
                    let alias_base = if self.node(name).kind() == K::Identifier {
                        let mut bytes = vec![b'_'];
                        bytes.extend_from_slice(self.output.view().node_text(name)?.as_bytes());
                        if ts_scanner::is_identifier_text(
                            &bytes,
                            ts_core::LanguageVariant::STANDARD,
                        ) {
                            bytes
                        } else {
                            b"_exported".to_vec()
                        }
                    } else {
                        b"_exported".to_vec()
                    };
                    let alias = self.unique_name(JsString::from_bytes(alias_base));
                    let qualified = self
                        .output
                        .new_qualified_name(Some(namespace), Some(class_name));
                    let import = self.output.new_import_equals_declaration(
                        None,
                        false,
                        Some(alias),
                        Some(qualified),
                    );
                    let export = self.named_export(Some(alias), name);
                    self.emit
                        .add_emit_flags(export, ts_printer::emit_flags::NO_COMMENTS);
                    Ok(self.syntax_list(vec![namespace_declaration, import, export]))
                } else {
                    let data = self
                        .node(declaration)
                        .data_source()
                        .as_class_declaration()
                        .unwrap()
                        .to_owned();
                    let modifiers = self.modifier_list(
                        mf::EXPORT | if self.needs_declare { mf::AMBIENT } else { 0 },
                    );
                    Ok(self.output.update_class_declaration(
                        declaration,
                        modifiers,
                        data.name,
                        data.type_parameters,
                        data.heritage_clauses,
                        data.members,
                    ))
                }
            })();
            self.tracker.watched_class = None;
            self.tracker.class_tracked = false;
            result
        } else {
            let identifier = self.node(name).kind() == K::Identifier;
            let class_name = if identifier {
                name
            } else {
                self.unique_name(JsString::from_bytes(b"_class".as_slice()))
            };
            let modifiers =
                self.modifier_list(mf::EXPORT | if self.needs_declare { mf::AMBIENT } else { 0 });
            let declaration = self.class_expression_declaration(class, class_name, modifiers)?;
            self.emit
                .assign_comment_range(self.output, declaration, node);
            if identifier {
                Ok(declaration)
            } else {
                let export = self.named_export(Some(class_name), name);
                self.emit
                    .add_emit_flags(export, ts_printer::emit_flags::NO_COMMENTS);
                Ok(self.syntax_list(vec![declaration, export]))
            }
        }
    }
    fn strip_declare(&mut self, node: NodeId) -> Result<NodeId, R::Error> {
        let Some(modifiers) = self.node(node).modifiers() else {
            return Ok(node);
        };
        let flags = self.output.read_list(modifiers).modifier_flags();
        if flags & mf::AMBIENT == 0 {
            return Ok(node);
        }
        let modifiers = self.modifier_list(flags & !mf::AMBIENT);
        self.replace_top_level_modifiers(node, modifiers)
    }
}
