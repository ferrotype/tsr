//! Contextual object/binding properties, inherited members, and import clauses.
use crate::{
    completion_context::{Container, Context},
    completion_keywords::Filter,
    completions::{properties, Candidate},
    syntax::Syntax,
    LanguageService, Result,
};
use std::collections::HashSet;
use tsr_ast::{modifier_flags as mf, symbol_flags as sf, utilities as ast, SyntaxKind as K};
use tsr_checker::{context_flags as cf, Operation};

impl LanguageService<'_> {
    pub(crate) fn completion_container(
        &self,
        checker: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        context: &mut Context,
        position: i64,
    ) -> Result<Option<Vec<Candidate>>> {
        let Some((kind, node)) = context.container else {
            return Ok(None);
        };
        if matches!(kind, Container::Interface | Container::Constructor) {
            return Ok(Some(Vec::new()));
        }
        let view = syntax.view;
        let mut symbols = Vec::new();
        let existing: Vec<tsr_ast::NodeId>;
        match kind {
            Container::Jsx => {
                let attrs = view
                    .node(node)?
                    .attributes()
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                let Some(ty) = checker.get_contextual_type(attrs, cf::NONE)? else {
                    return Ok(None);
                };
                let completions = checker.get_contextual_type(attrs, cf::IGNORE_NODE_INFERENCES)?;
                symbols = self.object_expression_properties(checker, attrs, ty, completions)?;
                existing = view
                    .node_slice(view.node(attrs)?.properties(view)?)?
                    .iter()
                    .flatten()
                    .collect();
            }
            Container::Object | Container::Binding => {
                let ty = if kind == Container::Object {
                    checker.get_contextual_type(node, cf::NONE)?
                } else {
                    Some(checker.get_type_at_location(node)?)
                };
                let Some(ty) = ty else {
                    context.container = None;
                    context.filter = Filter::All;
                    context.new_identifier = true;
                    return Ok(None);
                };
                symbols = if kind == Container::Object {
                    let completions =
                        checker.get_contextual_type(node, cf::IGNORE_NODE_INFERENCES)?;
                    self.object_expression_properties(checker, node, ty, completions)?
                } else {
                    properties(checker, ty)?
                };
                existing = if kind == Container::Object {
                    view.node_slice(view.node(node)?.properties(view)?)?
                        .iter()
                        .flatten()
                        .collect()
                } else {
                    view.node_slice(view.node(node)?.elements(view)?)?
                        .iter()
                        .flatten()
                        .collect()
                };
                context.new_identifier = checker.get_string_index_type(ty)?.is_some()
                    || checker.get_number_index_type(ty)?.is_some();
            }
            Container::Class => {
                let read = view.node(node)?;
                let mut bases =
                    tsr_ast::utilities_class::get_class_extends_heritage_element(view, node)?
                        .into_iter()
                        .collect::<Vec<_>>();
                bases.extend(
                    tsr_ast::utilities_class::get_implements_heritage_clause_elements(view, node)?,
                );
                for base in bases {
                    let ty = checker.get_type_at_location(base)?;
                    symbols.extend(checker.properties_of_type(ty)?);
                }
                existing = view
                    .node_slice(read.members(view)?)?
                    .iter()
                    .flatten()
                    .collect();
            }
            Container::Imports | Container::Exports => {
                let mut parent = view.node(node)?.parent();
                let mut specifier = None;
                while let Some(id) = parent {
                    let read = view.node(id)?;
                    if matches!(
                        read.kind().known(),
                        Some(K::ImportDeclaration | K::ExportDeclaration)
                    ) {
                        specifier = read.module_specifier();
                        break;
                    }
                    parent = read.parent();
                }
                if let Some(specifier) = specifier {
                    if let Some(module) = checker.get_symbol_at_location(specifier)? {
                        symbols = checker.get_exports_of_module(module)?;
                    }
                } else if kind == Container::Exports {
                    symbols = checker.get_symbols_in_scope(
                        node,
                        sf::VALUE | sf::TYPE | sf::NAMESPACE | sf::ALIAS,
                    )?;
                }
                existing = view
                    .node_slice(view.node(node)?.elements(view)?)?
                    .iter()
                    .flatten()
                    .collect();
            }
            Container::Constructor | Container::Interface => unreachable!(),
        }
        let mut names = HashSet::new();
        let mut spread = HashSet::new();
        for id in existing {
            if syntax.start(id)? <= position && position <= i64::from(view.node(id)?.end()) {
                continue;
            }
            let read = view.node(id)?;
            if matches!(
                read.kind().known(),
                Some(K::SpreadAssignment | K::JsxSpreadAttribute)
            ) {
                if let Some(expression) = read.expression() {
                    let ty = checker.get_type_at_location(expression)?;
                    for symbol in checker.properties_of_type(ty)? {
                        spread.insert(checker.symbol(symbol)?.name_bytes().to_vec());
                    }
                }
                continue;
            }
            let name = if matches!(
                kind,
                Container::Imports | Container::Exports | Container::Binding
            ) {
                read.property_name().or(read.name())
            } else {
                read.name()
            };
            if let Some(name) = name {
                if ast::is_string_or_numeric_literal_like(&view.node(name)?)
                    || ast::is_member_name(&view.node(name)?)
                {
                    names.insert(view.node_text(name)?.as_bytes().to_vec());
                }
            }
        }
        let mut candidates = Vec::new();
        for symbol in symbols {
            let s = checker.symbol(symbol)?;
            let name = s.name_bytes();
            if names.contains(name)
                || matches!(kind, Container::Imports | Container::Exports) && name == b"default"
            {
                continue;
            }
            if kind == Container::Class {
                if let Some(decl) = s.value_declaration() {
                    let read = checker.node(decl)?;
                    if read.modifier_flags(self.view(decl)?)? & mf::PRIVATE != 0
                        || ast::is_private_identifier_class_element_declaration(
                            self.view(decl)?,
                            decl,
                        )?
                    {
                        continue;
                    }
                }
            }
            candidates.push(Candidate {
                symbol,
                sort: if spread.contains(name) {
                    "13"
                } else if kind != Container::Class && s.flags() & sf::OPTIONAL != 0 {
                    "12"
                } else {
                    "11"
                },
                nullable: false,
                this_member: false,
                promise: false,
            });
        }
        Ok(Some(candidates))
    }
}

impl LanguageService<'_> {
    // port: tsc/internal/ls/completions.go:getPropertiesForObjectExpression
    pub(crate) fn object_expression_properties(
        &self,
        checker: &mut Operation<'_>,
        object: tsr_ast::NodeId,
        contextual: tsr_checker::TypeRef,
        completions: Option<tsr_checker::TypeRef>,
    ) -> Result<Vec<tsr_checker::SymbolRef>> {
        use tsr_checker::{object_flags as of, type_flags as tf};
        let separate = completions.is_some_and(|ty| ty != contextual);
        let types = if checker.type_flags(contextual)? & tf::UNION != 0 {
            checker.constituents(contextual)?
        } else {
            vec![contextual]
        };
        let mut filtered = Vec::new();
        for ty in types {
            if checker.get_promised_type_of_promise(ty)?.is_none() {
                filtered.push(ty);
            }
        }
        let mut ty = checker.get_union_type(&filtered)?;
        if let Some(completions) = completions.filter(|_| separate) {
            if checker.type_flags(completions)? & tf::ANY_OR_UNKNOWN == 0 {
                ty = checker.get_union_type(&[ty, completions])?;
            }
        }
        let properties = if checker.type_flags(ty)? & tf::UNION == 0 {
            checker.get_apparent_properties(ty)?
        } else {
            let mut types = Vec::new();
            for ty in checker.constituents(ty)? {
                if checker.type_flags(ty)? & tf::PRIMITIVE != 0
                    || checker.is_array_like_type(ty)?
                    || checker.is_type_invalid_due_to_union_discriminant(ty, object)?
                    || checker.type_has_call_or_construct_signatures(ty)?
                {
                    continue;
                }
                if checker.type_object_flags(ty)? & of::CLASS != 0 {
                    let properties = checker.get_apparent_properties(ty)?;
                    if nonpublic(checker, &properties)? {
                        continue;
                    }
                }
                types.push(ty);
            }
            checker.get_all_possible_properties_of_types(&types)?
        };
        if checker.type_object_flags(ty)? & of::CLASS != 0 && nonpublic(checker, &properties)? {
            return Ok(Vec::new());
        }
        if !separate {
            return Ok(properties);
        }
        let mut result = Vec::new();
        for property in properties {
            let declarations: Vec<_> = checker
                .symbol_declarations(property)?
                .iter()
                .flatten()
                .collect();
            if declarations.is_empty() {
                result.push(property);
                continue;
            }
            for declaration in declarations {
                if self.view(declaration)?.node(declaration)?.parent() != Some(object) {
                    result.push(property);
                    break;
                }
            }
        }
        Ok(result)
    }
}
// port: tsc/internal/ls/completions.go:containsNonPublicProperties
fn nonpublic(checker: &Operation<'_>, properties: &[tsr_checker::SymbolRef]) -> Result<bool> {
    for &property in properties {
        if checker.get_declaration_modifier_flags_from_symbol(property)?
            & mf::NON_PUBLIC_ACCESSIBILITY_MODIFIER
            != 0
        {
            return Ok(true);
        }
    }
    Ok(false)
}
