//! The `this.x = ...` member collection of JS classes (`transform.go`).
use super::transform::{ThisPropertyAssignmentKey, Transformer, NIL};
use std::collections::HashSet;
use tsr_ast::{FactoryMethods, JSDeclarationKind, JsString, NodeId, SyntaxKind as K};
use tsr_printer::emit_resolver::DeclarationEmitResolver;

impl<R: DeclarationEmitResolver> Transformer<'_, R> {
    // port: tsc/internal/transformers/declarations/transform.go:getThisPropertyAssignmentKey
    fn get_this_property_assignment_key(
        &self,
        name: Option<NodeId>,
        node: NodeId,
        is_static: bool,
    ) -> Result<ThisPropertyAssignmentKey, R::Error> {
        let is_private = self.kind(name.expect(NIL)) == K::PrivateIdentifier;
        if let Some(name) = name {
            if !tsr_ast::is_dynamic_name(self.view(), name)? {
                if let Some(name_text) =
                    tsr_ast::utilities_targets::try_get_text_of_property_name(self.view(), name)?
                {
                    return Ok(ThisPropertyAssignmentKey {
                        name: JsString::from_bytes(name_text),
                        node: None,
                        is_static,
                        is_private,
                    });
                }
            }
        }
        Ok(ThisPropertyAssignmentKey {
            name: JsString::default(),
            node: Some(node),
            is_static,
            is_private,
        })
    }

    /// The pin's `thisPropertyVisitor` over one node: the callback, then
    /// (unless it stops) the same visit of every child, in order.
    fn this_property_visitor_visit_each_child(&mut self, node: NodeId) -> Result<(), R::Error> {
        let mut stack: Vec<NodeId> = self.children_of(node)?.into_iter().rev().collect();
        while let Some(child) = stack.pop() {
            if self.visit_this_property_assignments(child)? {
                stack.extend(self.children_of(child)?.into_iter().rev());
            }
        }
        Ok(())
    }

    /// Answers whether the pin's callback goes on to visit the node's
    /// children (`thisPropertyVisitor.VisitEachChild(node)`).
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.visitThisPropertyAssignments
    fn visit_this_property_assignments(&mut self, node: NodeId) -> Result<bool, R::Error> {
        let this_container = tsr_ast::get_this_container(self.view(), node, false, false)?;
        let Some(this_target) = self.parent(this_container) else {
            return Ok(false); // thisContainer was source file, can't have expando-this
        };
        let is_static = tsr_ast::utilities::has_static_modifier(self.view(), this_container)?
            || self.kind(this_container) == K::ClassStaticBlockDeclaration;
        if this_target != self.enclosing_declaration {
            return Ok(false); // stop searching within new `this` contexts
        }
        if tsr_ast::get_assignment_declaration_kind(self.view(), node)?
            == JSDeclarationKind::ThisProperty
        {
            self.collect_this_property_assignment(node, this_target, is_static)?;
        }
        Ok(true)
    }
    /// The `JSDeclarationKindThisProperty` case of
    /// `visitThisPropertyAssignments`.
    fn collect_this_property_assignment(
        &mut self,
        node: NodeId,
        this_target: NodeId,
        is_static: bool,
    ) -> Result<(), R::Error> {
        let mut name = tsr_ast::get_name_of_declaration(self.view(), Some(node))?;
        let base = self.resolver.referenced_member_value_declaration(node)?;
        let key = self.get_this_property_assignment_key(name, node, is_static)?;
        if base.is_none() || self.seen_properties.contains(&key) {
            return Ok(());
        }
        self.seen_properties.insert(key);

        // problem: this prop might be overriding a prop from a base type. The checker has special bails for override compat comparisons for binary expression properties,
        // but what we transform to won't - so we either need to match the base type (for example, if it's a getter/setter) or emit nothing
        // See `checkKindsOfPropertyMemberOverrides` in the checker for what we're trying to satisfy here
        let heritage_clauses = self.class_heritage_clauses(this_target);
        if !self.list_nodes(heritage_clauses).is_empty()
            && !self.is_class_extending_null(Some(this_target))?
        {
            // there is a base type any assignments might be "from"
            self.report_inference_fallback(this_target)?; // Add an isolated declarations error on this class - we can't know how to transform this prop into an assignment without referring to type information
            if self.resolver.redundant_this_property_assignment(node)? {
                return Ok(()); // skip assignments whose member is already provided by an `extends` base type (an inherited accessor/method, or an identical inherited property)
                               // TODO: If the property has an explicit `@type` annotation, we should probably emit it (maybe with an `override` modifier) instead of skipping it
            }
        }

        let mods = if is_static {
            let modifier = self.new_modifier(K::StaticKeyword);
            Some(self.new_modifier_list(vec![modifier]))
        } else {
            None
        };
        if tsr_ast::has_dynamic_name(self.view(), Some(node))? {
            if !crate::utilities::is_simple_inlineable_expression(&*self.output, name.expect(NIL)) {
                return Ok(()); // Member either becomes an index signature or is a reassignment
            }
            self.check_name(node)?;
            name = Some(self.output.new_computed_property_name(name)); // Convert `this[foo] = expr` to `[foo]: Type`
        }
        let name = name.expect(NIL);
        if tsr_ast::utilities_targets::get_text_of_property_name(self.view(), name)?
            == b"constructor"
        {
            return Ok(()); // `constructor` is a builtin class member, not allowed to redeclare it
        }
        let name = if self.kind(name) == K::Identifier
            && !tsr_scanner::is_identifier_text(
                self.node_text(name)?.as_bytes(),
                tsr_core::LanguageVariant::STANDARD,
            ) {
            self.emit.new_string_literal_from_node(self.output, name)
        } else {
            name
        };
        let ty = self.ensure_type(node, false)?;
        let prop = self
            .output
            .new_property_declaration(mods, Some(name), None, ty, None);
        let parent = self.parent(node).expect(NIL);
        if self.kind(parent) == K::ExpressionStatement {
            self.preserve_js_doc(prop, parent);
        }
        self.this_property_assignments_collected.push(prop);
        Ok(())
    }

    fn class_heritage_clauses(&self, node: NodeId) -> Option<tsr_ast::NodeListId> {
        let read = self.node(node);
        match read.kind().known() {
            Some(K::ClassDeclaration) => read
                .as_class_declaration()
                .expect("class payload")
                .heritage_clauses(),
            Some(K::ClassExpression) => read
                .as_class_expression()
                .expect("class expression payload")
                .heritage_clauses(),
            _ => panic!("{NIL}"),
        }
    }

    // port: tsc/internal/transformers/declarations/transform.go:isClassExtendingNull
    fn is_class_extending_null(&self, node: Option<NodeId>) -> Result<bool, R::Error> {
        let Some(node) = node else {
            return Ok(false);
        };
        let Some(extends_clause) =
            tsr_ast::utilities_class::get_heritage_clause(self.view(), node, K::ExtendsKeyword)?
        else {
            return Ok(false);
        };
        let types = self
            .node(extends_clause)
            .as_heritage_clause()
            .expect("heritage clause payload")
            .types();
        let nodes = self.list_nodes(types);
        if types.is_none() || nodes.len() != 1 {
            return Ok(false);
        }
        let expr = self.node(nodes[0]).expression();
        Ok(expr.is_some_and(|expr| self.kind(expr) == K::NullKeyword))
    }

    // collectThisPropertyAssignments finds `this.x = expr` assignments in constructors, methods, and static blocks
    // of JS classes and synthesizes PropertyDeclaration nodes for each unique property name.
    // port: tsc/internal/transformers/declarations/transform.go:DeclarationTransformer.collectThisPropertyAssignments
    pub fn collect_this_property_assignments(
        &mut self,
        class_node: NodeId,
    ) -> Result<Vec<NodeId>, R::Error> {
        let members = self.list_nodes(self.node(class_node).member_list());
        let mut seen = HashSet::new();
        // Pre-populate seen with existing direct member nodes to avoid duplicates
        for &member in &members {
            if let Some(name) = self.node(member).name() {
                let is_static = tsr_ast::utilities::is_static(self.view(), member)?;
                seen.insert(self.get_this_property_assignment_key(
                    Some(name),
                    member,
                    is_static,
                )?);
            }
        }
        self.seen_properties = seen;
        self.this_property_assignments_collected = Vec::new();

        let mut result = Ok(());
        for n in members {
            result = self.this_property_visitor_visit_each_child(n);
            if result.is_err() {
                break;
            }
        }
        self.seen_properties.clear();
        let collected = std::mem::take(&mut self.this_property_assignments_collected);
        result.map(|()| collected)
    }
}
