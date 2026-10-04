use crate::{
    change_nodes::NodeTracker,
    code_actions::{localized, Fix, Provider},
    syntax::Syntax,
    CompletionOptions, LanguageService, Result,
};
use std::collections::HashSet;
use tsr_ast::{modifier_flags as mf, NodeId, SyntaxKind as K};
use tsr_checker::{BuilderRequest, Operation};
use tsr_core::TextRange;
impl LanguageService<'_> {
    // port: tsc/internal/ls/codeactions_fixclassincorrectlyimplementsinterface.go:getCodeActionsToFixClassIncorrectlyImplementsInterface
    pub(crate) fn class_fixes(
        &mut self,
        c: &mut Operation<'_>,
        source: NodeId,
        span: TextRange,
        options: &CompletionOptions,
        locale: &tsr_locale::Locale,
    ) -> Result<Vec<Fix>> {
        let mut syntax = Syntax::new(self.view(source)?, source)?;
        let token = syntax.nav().get_token_at_position(span.pos())?;
        let Some(class) = tsr_ast::utilities::get_containing_class(syntax.view, token)? else {
            return Ok(Vec::new());
        };
        let types: Vec<_> =
            tsr_ast::utilities_class::get_implements_heritage_clause_elements(syntax.view, class)?;
        let mut result = Vec::new();
        for ty in types {
            let mut tracker = NodeTracker::new(self.program, &options.format);
            let mut imports = tsr_autoimport::ImportAdder::default();
            self.class_fix_changes(
                c,
                &mut syntax,
                &mut tracker,
                class,
                ty,
                options,
                locale,
                &mut imports,
            )?;
            let changes = tracker.finish(self)?;
            if !changes.unmappable.is_empty() {
                continue;
            }
            let mut edits: Vec<_> = changes.edits.into_values().flatten().collect();
            edits.extend(
                self.import_adder_action_edits(&syntax, options, &imports)?
                    .into_iter()
                    .flatten()
                    .map(|e| *e),
            );
            if edits.is_empty() {
                continue;
            }
            let start = syntax.start(ty)?;
            let end = syntax.view.node(ty)?.end();
            result.push(Fix {
                title: localized(
                    tsr_diagnostics::Implement_interface_0,
                    locale,
                    &[&syntax.file.text().as_bytes()[start as usize..end as usize]],
                ),
                edits,
            });
        }
        Ok(result)
    }
    // port: tsc/internal/ls/codeactions_fixclassincorrectlyimplementsinterface.go:getAllCodeActionsToFixClassIncorrectlyImplementsInterface
    pub(crate) fn all_class_fixes(
        &mut self,
        c: &mut Operation<'_>,
        source: NodeId,
        options: &CompletionOptions,
        locale: &tsr_locale::Locale,
    ) -> Result<Option<Fix>> {
        let mut syntax = Syntax::new(self.view(source)?, source)?;
        let mut tracker = NodeTracker::new(self.program, &options.format);
        let mut imports = tsr_autoimport::ImportAdder::default();
        let mut seen = HashSet::new();
        let codes = Provider::Class.codes();
        for diagnostic in self.action_diagnostics(c, source)? {
            if !diagnostic.source.is_empty() || !codes.contains(&diagnostic.code) {
                continue;
            }
            let token = syntax.nav().get_token_at_position(diagnostic.loc.pos())?;
            let Some(class) = tsr_ast::utilities::get_containing_class(syntax.view, token)? else {
                continue;
            };
            if !seen.insert(class) {
                continue;
            }
            for ty in tsr_ast::utilities_class::get_implements_heritage_clause_elements(
                syntax.view,
                class,
            )? {
                self.class_fix_changes(
                    c,
                    &mut syntax,
                    &mut tracker,
                    class,
                    ty,
                    options,
                    locale,
                    &mut imports,
                )?;
            }
        }
        let changes = tracker.finish(self)?;
        if !changes.unmappable.is_empty() {
            return Ok(None);
        }
        let mut edits: Vec<_> = changes.edits.into_values().flatten().collect();
        edits.extend(
            self.import_adder_action_edits(&syntax, options, &imports)?
                .into_iter()
                .flatten()
                .map(|e| *e),
        );
        Ok((!edits.is_empty()).then(|| Fix {
            title: localized(
                tsr_diagnostics::Implement_all_unimplemented_interfaces,
                locale,
                &[],
            ),
            edits,
        }))
    }
    // port: tsc/internal/ls/codeactions_fixclassincorrectlyimplementsinterface.go:addChanges
    #[allow(
        clippy::too_many_arguments,
        reason = "Keep the source, class and shared edit/import transactions explicit, matching addChanges"
    )]
    fn class_fix_changes(
        &mut self,
        c: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        tracker: &mut NodeTracker<'_>,
        class: NodeId,
        implemented: NodeId,
        options: &CompletionOptions,
        locale: &tsr_locale::Locale,
        imports: &mut tsr_autoimport::ImportAdder,
    ) -> Result<()> {
        self.check_canceled()?;
        let view = syntax.view;
        let source = syntax.source;
        let members: Vec<_> = view
            .node_slice(view.node(class)?.members(view)?)?
            .iter()
            .flatten()
            .collect();
        let constructor = members
            .iter()
            .copied()
            .find(|&n| view.node(n).is_ok_and(|n| n.kind() == K::Constructor));
        let target = c.get_type_at_location(implemented)?;
        let class_type = c.get_type_at_location(class)?;
        for key in [c.get_number_type(), c.get_string_type()] {
            if c.get_index_info_of_type(class_type, key)?.is_some() {
                continue;
            }
            let Some(info) = c.get_index_info_of_type(target, key)? else {
                continue;
            };
            let mut builder = c.node_builder_with_emit(&tracker.emit);
            if let Some(root) = builder.index_info_to_index_signature_declaration(
                info,
                BuilderRequest {
                    enclosing: Some(class),
                    ..Default::default()
                },
            )? {
                let nodes = builder.into_syntax();
                tracker.retain_generated(nodes, &[root])?;
                insert_member(tracker, source, class, constructor, root)?;
            }
        }
        let mut present = HashSet::new();
        if let Some(symbol) = c.bound_symbol_of_node(class)? {
            if let Some(table) = c.symbol(symbol)?.members() {
                for (name, id) in c.symbol_table(table)? {
                    if id.is_some() {
                        present.insert(name.to_vec());
                    }
                }
            }
        }
        if let Some(base) =
            tsr_ast::utilities_class::get_class_extends_heritage_element(view, class)?
        {
            let base = c.get_type_at_location(base)?;
            for member in c.get_properties_of_type(base)? {
                if c.get_declaration_modifier_flags_from_symbol(member)? & mf::PRIVATE == 0 {
                    present.insert(c.symbol(member)?.name_bytes().to_vec());
                }
            }
        }
        for symbol in c.get_properties_of_type(target)? {
            let name = c.symbol(symbol)?.name_bytes().to_vec();
            let flags = c.get_declaration_modifier_flags_from_symbol(symbol)?;
            if flags & mf::PRIVATE != 0 || !present.insert(name) {
                continue;
            }
            let declaration = c.symbol_declarations(symbol)?.iter().flatten().next();
            let mut modifiers = flags & mf::STATIC;
            if flags & mf::PUBLIC != 0 {
                modifiers |= mf::PUBLIC;
            } else if flags & mf::PROTECTED != 0 {
                modifiers |= mf::PROTECTED;
            }
            if let Some(d) = declaration {
                if tsr_ast::utilities::is_auto_accessor_property_declaration(self.view(d)?, d)? {
                    modifiers |= mf::ACCESSOR;
                }
                if self.program.options().no_implicit_override.is_true()
                    && tsr_ast::utilities_class::has_abstract_modifier(self.view(d)?, d)?
                {
                    modifiers |= mf::OVERRIDE;
                }
            }
            let signature_only = view.node(class)?.flags() & tsr_ast::node_flags::AMBIENT != 0;
            if let Some((nodes, roots)) = self.member_nodes(
                c,
                syntax,
                symbol,
                class,
                signature_only,
                modifiers,
                &[],
                options,
                true,
                Some(locale),
                &tracker.emit,
                imports,
            )? {
                tracker.retain_generated(nodes, &roots)?;
                for root in roots {
                    insert_member(tracker, source, class, constructor, root)?;
                }
            }
        }
        Ok(())
    }
}
fn insert_member(
    t: &mut NodeTracker<'_>,
    source: NodeId,
    class: NodeId,
    constructor: Option<NodeId>,
    node: NodeId,
) -> Result<()> {
    if let Some(constructor) = constructor {
        t.insert_after(source, constructor, node)
    } else {
        t.insert_member_at_start(source, class, node)
    }
}
