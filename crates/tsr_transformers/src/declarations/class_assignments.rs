use super::transform::Transformer;
use std::{collections::HashSet, ops::ControlFlow};
use tsr_ast::{
    modifier_flags as mf, AstView, ChildVisitor, FactoryMethods, JsString, NodeId, NodeListId,
    NodeSlice, RuntimeFactory, SyntaxKind as K,
};
use tsr_printer::emit_resolver::DeclarationEmitResolver;

#[derive(Hash, PartialEq, Eq)]
struct AssignmentKey {
    name: Option<JsString>,
    node: Option<NodeId>,
    is_static: bool,
    is_private: bool,
}

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    // Inline copy of `TryGetTextOfPropertyName`; its Phase 1 home is in tsr_ast (table group targets).
    fn property_text(&self, name: NodeId) -> Result<Option<JsString>, R::Error> {
        match self.node(name).kind().known() {
            Some(
                K::Identifier
                | K::PrivateIdentifier
                | K::StringLiteral
                | K::NumericLiteral
                | K::BigIntLiteral
                | K::NoSubstitutionTemplateLiteral
                | K::JsxNamespacedName,
            ) => Ok(Some(self.output.view().node_text(name)?.into_js_string())),
            Some(K::ComputedPropertyName) => {
                let expression = Self::required(self.node(name).expression())?;
                if matches!(
                    self.node(expression).kind().known(),
                    Some(
                        K::StringLiteral
                            | K::NumericLiteral
                            | K::BigIntLiteral
                            | K::NoSubstitutionTemplateLiteral
                    )
                ) {
                    Ok(Some(
                        self.output.view().node_text(expression)?.into_js_string(),
                    ))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }
    // port: tsc/internal/transformers/declarations/transform.go:getThisPropertyAssignmentKey
    fn assignment_key(
        &self,
        name: NodeId,
        node: NodeId,
        is_static: bool,
    ) -> Result<AssignmentKey, R::Error> {
        let text = if tsr_ast::is_dynamic_name(self.output.view(), name)? {
            None
        } else {
            self.property_text(name)?
        };
        Ok(AssignmentKey {
            node: text.is_none().then_some(node),
            name: text,
            is_static,
            is_private: self.node(name).kind() == K::PrivateIdentifier,
        })
    }
    fn class_extends_null(&self, node: NodeId) -> Result<bool, R::Error> {
        for clause in self.list_nodes(self.class_heritage(node)?) {
            let read = self.node(clause);
            let data = read.as_heritage_clause().unwrap();
            if data.token() != K::ExtendsKeyword {
                continue;
            }
            let types = self.list_nodes(data.types());
            return Ok(types.len() == 1
                && self
                    .node(types[0])
                    .expression()
                    .is_some_and(|expression| self.node(expression).kind() == K::NullKeyword));
        }
        Ok(false)
    }
    fn class_heritage(&self, node: NodeId) -> Result<Option<NodeListId>, R::Error> {
        let read = self.node(node);
        let data = read.data_source();
        match read.kind().known() {
            Some(K::ClassDeclaration) => {
                Ok(data.as_class_declaration().unwrap().heritage_clauses())
            }
            Some(K::ClassExpression) => Ok(data.as_class_expression().unwrap().heritage_clauses()),
            _ => Err(tsr_arena::Error::InvalidGraph.into()),
        }
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.collectThisPropertyAssignments
    pub fn collect_this_property_assignments(
        &mut self,
        class: NodeId,
    ) -> Result<Vec<NodeId>, R::Error> {
        let mut seen = HashSet::new();
        let members = self.list_nodes(self.node(class).member_list());
        for &member in &members {
            if let Some(name) = self.node(member).name() {
                let is_static =
                    tsr_ast::utilities::get_combined_modifier_flags(self.output.view(), member)?
                        & mf::STATIC
                        != 0;
                seen.insert(self.assignment_key(name, member, is_static)?);
            }
        }
        let mut output = Vec::new();
        for member in members {
            let mut stack = self.assignment_children(member)?;
            stack.reverse();
            while let Some(node) = stack.pop() {
                if self.visit_this_property_assignment(class, node, &mut seen, &mut output)? {
                    let children = self.assignment_children(node)?;
                    stack.extend(children.into_iter().rev());
                }
            }
        }
        Ok(output)
    }
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.visitThisPropertyAssignments
    fn visit_this_property_assignment(
        &mut self,
        class: NodeId,
        node: NodeId,
        seen: &mut HashSet<AssignmentKey>,
        output: &mut Vec<NodeId>,
    ) -> Result<bool, R::Error> {
        let container = tsr_ast::get_this_container(self.output.view(), node, false, false)?;
        if self.node(container).parent() != Some(class) {
            return Ok(false);
        }
        if tsr_ast::get_assignment_declaration_kind(self.output.view(), node)?
            != tsr_ast::JSDeclarationKind::ThisProperty
        {
            return Ok(true);
        }
        let mut name = Self::required(tsr_ast::get_name_of_declaration(
            self.output.view(),
            Some(node),
        )?)?;
        let is_static = self.node(container).kind() == K::ClassStaticBlockDeclaration
            || tsr_ast::utilities::get_combined_modifier_flags(self.output.view(), container)?
                & mf::STATIC
                != 0;
        let base = self.resolver.referenced_member_value_declaration(node)?;
        let key = self.assignment_key(name, node, is_static)?;
        if base.is_none() || !seen.insert(key) {
            return Ok(true);
        }
        if !self.list_nodes(self.class_heritage(class)?).is_empty()
            && !self.class_extends_null(class)?
        {
            self.inference_fallback(class)?;
            if self.resolver.redundant_this_property_assignment(node)? {
                return Ok(true);
            }
        }
        if tsr_ast::has_dynamic_name(self.output.view(), Some(node))? {
            // IsSimpleInlineableExpression excludes identifiers deliberately.
            let kind = self.node(name).kind();
            if !matches!(
                kind.known(),
                Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral | K::NumericLiteral)
            ) && !tsr_ast::is_keyword_kind(kind)
            {
                return Ok(true);
            }
            let old = self.tracker.selector.clone();
            if !self.suppress_context {
                self.select_context(node, true)?;
            }
            self.tracker.error_name = self.node(node).name();
            let expression = Self::required(
                self.node(Self::required(self.node(node).name())?)
                    .expression(),
            )?;
            let result = self.entity_visible(expression);
            if !self.suppress_context {
                self.tracker.selector = old;
            }
            self.tracker.error_name = None;
            result?;
            name = self.output.new_computed_property_name(Some(name));
        }
        if self
            .property_text(name)?
            .is_some_and(|text| text.as_bytes() == b"constructor")
        {
            return Ok(true);
        }
        if self.node(name).kind() == K::Identifier {
            let text = self.output.view().node_text(name)?.into_js_string();
            if !tsr_scanner::is_identifier_text(
                text.as_bytes(),
                tsr_core::LanguageVariant::STANDARD,
            ) {
                name = self.output.new_string_literal(text, 0);
            }
        }
        let modifiers = if is_static {
            let modifier = self.output.new_token(K::StaticKeyword.into());
            let nodes = self.output.alloc_nodes(vec![Some(modifier)]);
            Some(self.output.new_modifier_list(nodes))
        } else {
            None
        };
        let ty = self.ensure_type(node, false)?;
        output.push(
            self.output
                .new_property_declaration(modifiers, Some(name), None, ty, None),
        );
        Ok(true)
    }
    pub fn assignment_children(&self, node: NodeId) -> Result<Vec<NodeId>, R::Error> {
        let mut visitor = AssignmentChildren {
            view: self.output.view(),
            nodes: Vec::new(),
            error: None,
        };
        let _ = self.node(node).for_each_child(&mut visitor);
        if let Some(error) = visitor.error {
            return Err(error.into());
        }
        Ok(visitor.nodes)
    }
}
struct AssignmentChildren<'a> {
    view: AstView<'a>,
    nodes: Vec<NodeId>,
    error: Option<tsr_arena::Error>,
}
impl ChildVisitor for AssignmentChildren<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.nodes.push(node);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        match self.view.list(list) {
            Ok(list) => self.visit_node_slice(list.nodes()),
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        match self.view.node_slice(nodes) {
            Ok(nodes) => {
                self.nodes.extend(nodes.iter().flatten());
                ControlFlow::Continue(())
            }
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
}
