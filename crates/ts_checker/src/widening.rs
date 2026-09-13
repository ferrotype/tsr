//! Widening contexts are transient trees indexed within one computation. Only
//! the native root results and undefined properties survive in checker caches.
use crate::{
    object_flags as of, type_flags as tf, types::Map, CheckerState, Error, TypeId, UnionReduction,
};
use ts_arena::SymbolId;
use ts_ast::{symbol_flags as sf, JsString};

#[derive(Default)]
struct Context {
    parent: Option<usize>,
    property_name: JsString,
    siblings: Option<Vec<TypeId>>,
    properties: Option<Vec<SymbolId>>,
    children: Map<JsString, usize>,
    widened: Map<TypeId, TypeId>,
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getWidenedType
    pub(crate) fn widened_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        self.widened_type_with_context(ty, None, &mut Vec::new())
    }

    // port: tsc/internal/checker/checker.go:Checker.getWidenedTypeWithContext
    fn widened_type_with_context(
        &mut self,
        ty: TypeId,
        context: Option<usize>,
        contexts: &mut Vec<Context>,
    ) -> Result<TypeId, Error> {
        let record = *self.types.get(ty)?;
        if record.object_flags & of::REQUIRES_WIDENING == 0 {
            return Ok(ty);
        }
        if context.is_none() {
            if let Some(&cached) = self.query.widened_types.get(&ty) {
                return Ok(cached);
            }
        }
        let result = if record.flags & (tf::ANY | tf::NULLABLE) != 0 {
            Some(self.builtins.any_type)
        } else if record.object_flags & of::OBJECT_LITERAL != 0 {
            Some(self.widened_object_literal(ty, context, contexts)?)
        } else if record.flags & tf::UNION != 0 {
            let parts = self.types.compound_types(ty)?.clone();
            let context = if let Some(context) = context {
                context
            } else {
                let id = contexts.len();
                contexts.push(Context {
                    siblings: Some(parts.to_vec()),
                    ..Default::default()
                });
                id
            };
            let mut types = Vec::with_capacity(parts.len());
            let mut empty = false;
            for &part in parts.iter() {
                let widened = if self.types.flags(part)? & tf::NULLABLE != 0 {
                    part
                } else {
                    self.widened_type_with_context(part, Some(context), contexts)?
                };
                empty |= self.empty_object_type(widened)?;
                types.push(widened);
            }
            Some(self.get_union_type_ex(
                &types,
                if empty {
                    UnionReduction::Subtype
                } else {
                    UnionReduction::Literal
                },
                None,
                None,
            )?)
        } else if record.flags & tf::INTERSECTION != 0 {
            let parts = self.types.compound_types(ty)?.clone();
            let mut widened = Vec::with_capacity(parts.len());
            for &part in parts.iter() {
                widened.push(self.widened_type(part)?);
            }
            Some(self.get_intersection_type(&widened)?)
        } else if self.is_array_type(ty)? || self.is_tuple_type(ty)? {
            let arguments = self.get_type_arguments(ty)?;
            let mut widened = Vec::with_capacity(arguments.len());
            for &part in arguments.iter() {
                widened.push(self.widened_type(part)?);
            }
            Some(self.create_type_reference(self.types.target(ty)?, &widened)?)
        } else {
            None
        };
        if let Some(result) = result {
            if context.is_none() {
                self.query.widened_types.insert(ty, result);
            }
            Ok(result)
        } else {
            Ok(ty)
        }
    }

    // port: tsc/internal/checker/checker.go:WideningContext.getChildContext
    fn widening_child(context: usize, name: JsString, contexts: &mut Vec<Context>) -> usize {
        if let Some(&id) = contexts[context].children.get(name.as_bytes()) {
            return id;
        }
        let id = contexts.len();
        contexts.push(Context {
            parent: Some(context),
            property_name: name.clone(),
            ..Default::default()
        });
        contexts[context].children.insert(name, id);
        id
    }

    // port: tsc/internal/checker/checker.go:Checker.getSiblingsOfContext
    fn widening_siblings(
        &mut self,
        context: usize,
        contexts: &mut Vec<Context>,
    ) -> Result<Vec<TypeId>, Error> {
        if let Some(siblings) = &contexts[context].siblings {
            return Ok(siblings.clone());
        }
        let parent = contexts[context]
            .parent
            .ok_or(Error::MissingLink("widening parent"))?;
        let name = contexts[context].property_name.clone();
        let mut siblings = Vec::new();
        for ty in self.widening_siblings(parent, contexts)? {
            if self.types.get(ty)?.object_flags & of::OBJECT_LITERAL != 0 {
                if let Some(property) = self.constituent_property(ty, name.as_bytes(), false)? {
                    let ty = self.get_type_of_symbol(property)?;
                    siblings.extend(self.distributed_types(ty)?);
                }
            }
        }
        contexts[context].siblings = Some(siblings.clone());
        Ok(siblings)
    }

    // port: tsc/internal/checker/checker.go:Checker.getPropertiesOfContext
    fn widening_properties(
        &mut self,
        context: usize,
        contexts: &mut Vec<Context>,
    ) -> Result<Vec<SymbolId>, Error> {
        if let Some(properties) = &contexts[context].properties {
            return Ok(properties.clone());
        }
        let mut names = Map::default();
        let mut properties = Vec::new();
        for ty in self.widening_siblings(context, contexts)? {
            let flags = self.types.get(ty)?.object_flags;
            if flags & of::OBJECT_LITERAL != 0 && flags & of::CONTAINS_SPREAD == 0 {
                for property in self.get_properties_of_type(ty)? {
                    let name = self.symbol(property)?.name_to_owned();
                    if let Some(&index) = names.get(name.as_bytes()) {
                        properties[index] = property;
                    } else {
                        names.insert(name, properties.len());
                        properties.push(property);
                    }
                }
            }
        }
        contexts[context].properties = Some(properties.clone());
        Ok(properties)
    }

    // port: tsc/internal/checker/checker.go:Checker.getUndefinedProperty
    fn undefined_property(&mut self, property: SymbolId) -> Result<SymbolId, Error> {
        let name = self.symbol(property)?.name_to_owned();
        if let Some(&cached) = self.query.undefined_properties.get(name.as_bytes()) {
            return Ok(cached);
        }
        let result =
            self.create_symbol_with_type(property, self.builtins.undefined_or_missing_type)?;
        self.symbol_mut(result)?.flags |= sf::OPTIONAL;
        self.query.undefined_properties.insert(name, result);
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.getWidenedTypeOfObjectLiteral
    fn widened_object_literal(
        &mut self,
        ty: TypeId,
        context: Option<usize>,
        contexts: &mut Vec<Context>,
    ) -> Result<TypeId, Error> {
        if let Some(context) = context {
            if let Some(&cached) = contexts[context].widened.get(&ty) {
                return Ok(cached);
            }
        }
        let mut members = ts_ast::SymbolTable::new();
        for property in self.get_properties_of_type(ty)? {
            let name = self.symbol(property)?.name_to_owned();
            let widened = if self.symbol(property)?.flags() & sf::PROPERTY != 0 {
                let original = self.get_type_of_symbol(property)?;
                let child =
                    context.map(|context| Self::widening_child(context, name.clone(), contexts));
                let widened = self.widened_type_with_context(original, child, contexts)?;
                if widened == original {
                    property
                } else {
                    self.create_symbol_with_type(property, widened)?
                }
            } else {
                property
            };
            members.insert(name, Some(widened));
        }
        if let Some(context) = context {
            for property in self.widening_properties(context, contexts)? {
                let name = self.symbol(property)?.name_to_owned();
                if !members.contains_key(name.as_bytes()) {
                    members.insert(name, Some(self.undefined_property(property)?));
                }
            }
        }
        let mut indexes = Vec::new();
        for index in self.index_infos_of_type(ty)? {
            let info = self.signatures.index_info(index)?.clone();
            let value = self.widened_type(info.value_type)?;
            indexes.push(self.signatures.new_index_info(
                info.key_type,
                value,
                info.is_readonly,
                info.declaration,
                info.components,
            )?);
        }
        let table = self.alloc_symbol_table(members);
        let record = *self.types.get(ty)?;
        let result = self.new_anonymous_type(record.symbol, Some(table), &[], &[], &indexes)?;
        self.types.get_mut(result)?.object_flags |=
            record.object_flags & (of::JS_LITERAL | of::NON_INFERRABLE_TYPE);
        if let Some(context) = context {
            if contexts[context].parent.is_some() {
                contexts[context].widened.insert(ty, result);
            }
        }
        Ok(result)
    }
}
