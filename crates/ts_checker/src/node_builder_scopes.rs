//! Temporary lexical scopes for serialization. Source bindings stay immutable;
//! checker-owned fake blocks expose the native parameter/type-parameter locals.
use super::{names::NameAccess, NodeBuilder};
use crate::{Error, MapperId, SignatureId, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{FactoryMethods, JsString, SymbolTable, SymbolTableId, SyntaxKind as K};
use ts_nodebuilder::flags as nf;

#[derive(Default)]
pub(super) struct TypeParameterNames {
    names: crate::types::Map<TypeId, NodeId>,
    text: crate::types::Set<JsString>,
    next: crate::types::Map<JsString, usize>,
}

struct ScopeUndo {
    table: SymbolTableId,
    #[allow(
        clippy::option_option,
        reason = "Scope rollback distinguishes a missing table key from a present key with a nil symbol"
    )]
    values: Vec<(JsString, Option<Option<SymbolId>>)>,
}

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/nodebuilderscopes.go:NodeBuilderImpl.enterSignatureScope
    pub(crate) fn with_signature_scope<T>(
        &mut self,
        signature: SignatureId,
        action: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.with_expanded_signature_scope(signature, |builder, _| action(builder))
    }

    pub(super) fn with_expanded_signature_scope<T>(
        &mut self,
        signature: SignatureId,
        action: impl FnOnce(&mut Self, &[SymbolId]) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let data = self.checker.signatures.get(signature)?.clone();
        let expanded = self.checker.expanded_signature_parameters(signature)?;
        self.with_serialization_scope(
            data.declaration,
            &expanded,
            data.type_parameters.as_deref().unwrap_or_default(),
            data.parameters.as_deref().unwrap_or_default(),
            data.mapper,
            |builder| action(builder, &expanded),
        )
    }

    // port: tsc/internal/checker/nodebuilderscopes.go:NodeBuilderImpl.enterNewScope
    #[allow(
        clippy::too_many_arguments,
        reason = "Native scope separates expanded parameters, original parameters, type parameters and mapper"
    )]
    pub(super) fn with_serialization_scope<T>(
        &mut self,
        declaration: Option<NodeId>,
        expanded: &[SymbolId],
        type_parameters: &[TypeId],
        original: &[SymbolId],
        mapper: Option<MapperId>,
        action: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let enclosing = self.enclosing;
        let old_mapper = self.mapper;
        let names = TypeParameterNames {
            names: self.type_parameter_names.names.clone(),
            text: self.type_parameter_names.text.clone(),
            next: self.type_parameter_names.next.clone(),
        };
        let mut undos = Vec::new();
        if mapper.is_some() {
            self.mapper = mapper;
        }
        let result = (|| {
            if self.enclosing.is_some() && declaration.is_some() {
                if !expanded.is_empty() {
                    let mut locals = SymbolTable::default();
                    for (index, &param) in expanded.iter().enumerate() {
                        if !original.is_empty() && original.get(index).copied() != Some(param) {
                            if let Some(&symbol) = original.get(index) {
                                locals.insert(
                                    self.checker.symbol(symbol)?.name_to_owned(),
                                    Some(symbol),
                                );
                            }
                        } else {
                            let declarations: Vec<_> = self
                                .checker
                                .symbol_declarations(param)?
                                .iter()
                                .flatten()
                                .collect();
                            let mut binding = false;
                            for decl in declarations {
                                let read = self.checker.ast(decl)?.node(decl)?;
                                if read.kind() == K::Parameter {
                                    if let Some(name) = read.name() {
                                        if matches!(
                                            self.checker.ast(name)?.node(name)?.kind().known(),
                                            Some(K::ObjectBindingPattern | K::ArrayBindingPattern)
                                        ) {
                                            self.add_binding_scope_names(name, &mut locals)?;
                                            binding = true;
                                            break;
                                        }
                                    }
                                }
                            }
                            if !binding {
                                locals.insert(
                                    self.checker.symbol(param)?.name_to_owned(),
                                    Some(param),
                                );
                            }
                        }
                    }
                    if let Some(undo) = self.push_serialization_scope("params", locals)? {
                        undos.push(undo);
                    }
                }
                if self.flags & nf::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS != 0
                    && !type_parameters.is_empty()
                {
                    let mut locals = SymbolTable::default();
                    for &ty in type_parameters {
                        let name = self.type_parameter_name(ty)?;
                        let name = self.ast.view().node_text(name)?.into_js_string();
                        locals.insert(name, self.checker.types.get(ty)?.symbol);
                    }
                    if let Some(undo) = self.push_serialization_scope("typeParams", locals)? {
                        undos.push(undo);
                    }
                }
            }
            action(self)
        })();
        // Restore existing fake tables even when a nested callback fails.
        let cleanup = (|| {
            for undo in undos {
                let mut table = self.checker.tables.get_mut(undo.table)?;
                for (name, previous) in undo.values {
                    match previous {
                        Some(value) => {
                            table.insert(name, value);
                        }
                        None => {
                            table.remove(name.as_bytes());
                        }
                    }
                }
            }
            Ok::<(), Error>(())
        })();
        self.enclosing = enclosing;
        self.mapper = old_mapper;
        self.type_parameter_names = names;
        // These tables are mutable within the scope. Name query caches must not
        // outlive the table contents they inspected.
        self.name_access = NameAccess::default();
        cleanup?;
        result
    }

    fn push_serialization_scope(
        &mut self,
        kind: &'static str,
        locals: SymbolTable,
    ) -> Result<Option<ScopeUndo>, Error> {
        let enclosing = self
            .enclosing
            .ok_or(Error::MissingLink("serialization enclosing scope"))?;
        let parent = self.checker.ast(enclosing)?.node(enclosing)?.parent();
        let scope = [Some(enclosing), parent]
            .into_iter()
            .flatten()
            .find(|node| self.checker.synthetic_scopes.signature_kinds.get(node) == Some(&kind));
        self.name_access = NameAccess::default();
        if let Some(scope) = scope {
            let table = self
                .checker
                .checker_node_binding(scope)?
                .and_then(|b| b.locals)
                .ok_or(Error::MissingLink("fake scope locals"))?;
            let mut table_data = self.checker.tables.get_mut(table)?;
            let mut values = Vec::new();
            for (name, symbol) in locals {
                let old = table_data.insert(name.clone(), symbol);
                values.push((name, old));
            }
            Ok(Some(ScopeUndo { table, values }))
        } else {
            self.enclosing = Some(self.checker.create_emit_scope(
                enclosing,
                K::Block,
                None,
                None,
                locals,
                Some(kind),
            )?);
            Ok(None)
        }
    }

    fn add_binding_scope_names(
        &mut self,
        mut pattern: NodeId,
        locals: &mut SymbolTable,
    ) -> Result<(), Error> {
        loop {
            // Preserve the pinned walk's return after its first element, including
            // an omitted element. This is not a general binding-pattern traversal.
            let list = self.checker.ast(pattern)?.node(pattern)?.element_list();
            if let Some(element) = self.checker.source_list(pattern, list)?.first().copied() {
                let read = self.checker.ast(element)?.node(element)?;
                if read.kind() == K::OmittedExpression {
                    return Ok(());
                }
                if read.kind() != K::BindingElement {
                    return Err(ts_arena::Error::InvalidGraph.into());
                }
                if let Some(name) = read.name() {
                    if matches!(
                        self.checker.ast(name)?.node(name)?.kind().known(),
                        Some(K::ObjectBindingPattern | K::ArrayBindingPattern)
                    ) {
                        pattern = name;
                        continue;
                    }
                }
                if let Some(symbol) = self.checker.get_symbol_of_declaration(element)? {
                    locals.insert(self.checker.symbol(symbol)?.name_to_owned(), Some(symbol));
                }
            }
            return Ok(());
        }
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.typeParameterToName
    pub(super) fn type_parameter_name(&mut self, ty: TypeId) -> Result<NodeId, Error> {
        let generated = self.flags & nf::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS != 0;
        if generated {
            if let Some(&node) = self.type_parameter_names.names.get(&ty) {
                return Ok(node);
            }
        }
        let symbol = self.checker.types.get(ty)?.symbol;
        let mut node = match symbol {
            Some(symbol) => self.symbol_node(symbol)?,
            None => self
                .ast
                .new_identifier(JsString::from_bytes(b"?".as_slice())),
        };
        if self.ast.view().node(node)?.kind() != K::Identifier {
            return Ok(self
                .ast
                .new_identifier(JsString::from_bytes(b"(Missing type parameter)".as_slice())));
        }
        if let Some(symbol) = symbol {
            if let Some(decl) = self.checker.symbol_declarations(symbol)?.first().flatten() {
                let read = self.checker.ast(decl)?.node(decl)?;
                if read.kind() == K::TypeParameter {
                    if let Some(name) = read.name() {
                        node = self.set_reused_text_range(node, name)?;
                    }
                }
            }
        }
        if generated {
            let raw = self.ast.view().node_text(node)?.into_js_string();
            let mut index = self
                .type_parameter_names
                .next
                .get(&raw)
                .copied()
                .unwrap_or(0);
            let mut name = raw.clone();
            loop {
                let shadow = match self.checker.resolve_name(
                    self.enclosing,
                    name.as_bytes(),
                    ts_ast::symbol_flags::TYPE,
                    None,
                    false,
                )? {
                    Some(found) => {
                        self.checker.symbol(found)?.flags() & ts_ast::symbol_flags::TYPE_PARAMETER
                            != 0
                            && Some(found) != symbol
                    }
                    None => false,
                };
                if !self.type_parameter_names.text.contains(&name) && !shadow {
                    break;
                }
                index += 1;
                let mut bytes = raw.as_bytes().to_vec();
                bytes.extend_from_slice(format!("_{index}").as_bytes());
                name = JsString::from_bytes(bytes);
            }
            if name != raw {
                node = self.ast.new_identifier(name.clone());
                self.id_to_symbol.insert(node, symbol);
            }
            self.type_parameter_names.next.insert(raw, index);
            self.type_parameter_names.names.insert(ty, node);
            self.type_parameter_names.text.insert(name);
        }
        Ok(node)
    }
}
