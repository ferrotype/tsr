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
        let class_flags = if kind == Container::Class {
            class_flags(syntax, context, position)?
        } else {
            0
        };
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
                    object_contextual_type(checker, syntax, node)?
                } else {
                    Some(checker.get_type_at_location(node)?)
                };
                let Some(ty) = ty else {
                    context.container = None;
                    context.filter = Filter::All;
                    context.new_identifier = true;
                    return Ok(None);
                };
                if kind == Container::Object {
                    let completions =
                        checker.get_contextual_type(node, cf::IGNORE_NODE_INFERENCES)?;
                    let index_type = completions.unwrap_or(ty);
                    let number_index = checker.get_number_index_type(index_type)?;
                    context.new_identifier = checker.get_string_index_type(index_type)?.is_some()
                        || number_index.is_some();
                    symbols = self.object_expression_properties(checker, node, ty, completions)?;
                    if symbols.is_empty() && number_index.is_none() {
                        context.container = None;
                        context.filter = Filter::All;
                        return Ok(None);
                    }
                } else {
                    context.new_identifier = false;
                    symbols = properties(checker, ty)?;
                }
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
            }
            Container::Class => {
                let read = view.node(node)?;
                let mut bases =
                    tsr_ast::utilities_class::get_class_extends_heritage_element(view, node)?
                        .into_iter()
                        .collect::<Vec<_>>();
                if class_flags & mf::OVERRIDE == 0 {
                    bases.extend(
                        tsr_ast::utilities_class::get_implements_heritage_clause_elements(
                            view, node,
                        )?,
                    );
                }
                if class_flags & mf::PRIVATE != 0 {
                    bases.clear();
                }
                for base in bases {
                    let mut ty = checker.get_type_at_location(base)?;
                    if class_flags & mf::STATIC != 0 {
                        let Some(symbol) = checker.type_symbol(ty)? else {
                            continue;
                        };
                        ty = checker.get_type_of_symbol_at_location(
                            checker.symbol_ref(symbol)?,
                            Some(node),
                        )?;
                    }
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
                    return self
                        .local_export_completions(checker, syntax, node)
                        .map(Some);
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
            if kind == Container::Class {
                if !matches!(
                    read.kind().known(),
                    Some(
                        K::PropertyDeclaration
                            | K::MethodDeclaration
                            | K::GetAccessor
                            | K::SetAccessor
                    )
                ) {
                    continue;
                }
                let flags = read.modifier_flags(view)?;
                if flags & mf::PRIVATE != 0
                    || (flags & mf::STATIC != 0) != (class_flags & mf::STATIC != 0)
                {
                    continue;
                }
            }
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
                if checker.symbol_declarations(symbol)?.is_empty() {
                    continue;
                }
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

    fn local_export_completions(
        &self,
        checker: &Operation<'_>,
        syntax: &Syntax<'_>,
        node: tsr_ast::NodeId,
    ) -> Result<Vec<Candidate>> {
        let Some(container) = crate::definition::ancestor(syntax.view, Some(node), |id| {
            Ok(matches!(
                syntax.view.node(id)?.kind().known(),
                Some(K::SourceFile | K::ModuleDeclaration)
            ))
        })?
        else {
            return Ok(Vec::new());
        };
        let file = self
            .program
            .file_of_node(container)
            .ok_or(tsr_arena::Error::WrongOwner)?;
        let Some(binding) = file.bound().view().node_binding(container)? else {
            return Ok(Vec::new());
        };
        let Some(locals) = binding.locals else {
            return Ok(Vec::new());
        };
        let exports = binding
            .symbol
            .map(|id| {
                checker
                    .symbol_ref(id)
                    .and_then(|id| checker.symbol(id).map(tsr_ast::SymbolRef::exports))
            })
            .transpose()?
            .flatten();
        let exports = exports.map(|id| checker.symbol_table(id)).transpose()?;
        checker
            .symbol_table(locals)?
            .iter()
            .filter_map(|(name, symbol)| {
                symbol.map(|symbol| {
                    Ok(Candidate {
                        symbol: checker.symbol_ref(symbol)?,
                        sort: if exports.is_some_and(|table| table.contains_key(name)) {
                            "12"
                        } else {
                            "11"
                        },
                        nullable: false,
                        this_member: false,
                        promise: false,
                    })
                })
            })
            .collect()
    }
}

// port: tsc/internal/ls/completions.go:tryGetObjectLiteralContextualType
fn object_contextual_type(
    checker: &mut Operation<'_>,
    syntax: &Syntax<'_>,
    node: tsr_ast::NodeId,
) -> Result<Option<tsr_checker::TypeRef>> {
    if let Some(ty) = checker.get_contextual_type(node, cf::NONE)? {
        return Ok(Some(ty));
    }
    let view = syntax.view;
    let Some(parent) = ast::walk_up_parenthesized_expressions(view, view.node(node)?.parent())?
    else {
        return Ok(None);
    };
    let read = view.node(parent)?;
    if let Some(binary) = read.data_source().as_binary_expression() {
        if binary.left() == Some(node)
            && binary
                .operator_token()
                .is_some_and(|id| view.node(id).is_ok_and(|n| n.kind() == K::EqualsToken))
        {
            return Ok(Some(checker.get_type_at_location(parent)?));
        }
    }
    if tsr_ast::utilities_positions::is_expression(view, parent)? {
        return Ok(checker.get_contextual_type(parent, cf::NONE)?);
    }
    Ok(None)
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

// Flags for the member being completed, distinct from the modifiers retained in
// a generated snippet: a half-typed identifier can itself stand for a modifier.
fn class_flags(syntax: &mut Syntax<'_>, context: &Context, position: i64) -> Result<u32> {
    let Some(token) = context.token else {
        return Ok(0);
    };
    let tr = syntax.view.node(token)?;
    let mut element = tr.parent();
    if tr.kind() == K::SemicolonToken {
        element = element.and_then(|n| syntax.view.node(n).ok()?.parent());
    }
    let mut flags = if let Some(element) = element {
        let read = syntax.view.node(element)?;
        if ast::is_class_element(&read) {
            read.modifier_flags(syntax.view)?
        } else {
            0
        }
    } else {
        0
    };
    if tr.kind() == K::Identifier
        && !(syntax.start(token)? <= position && position <= i64::from(tr.end()))
    {
        flags |= match syntax.view.node_text(token)?.as_bytes() {
            b"private" => mf::PRIVATE,
            b"static" => mf::STATIC,
            b"override" => mf::OVERRIDE,
            _ => 0,
        };
    }
    if element.is_some_and(|n| {
        syntax
            .view
            .node(n)
            .is_ok_and(|n| n.kind() == K::ClassStaticBlockDeclaration)
    }) {
        flags |= mf::STATIC;
    }
    Ok(flags)
}
