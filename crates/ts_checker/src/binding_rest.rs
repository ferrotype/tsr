//! Object rest shares the native spread-property selection and symbol cloning
//! rules. Generic rest preserves the checked global `Omit` alias identity.
use crate::{
    object_flags as of, type_facts as facts, type_flags as tf, CheckerState, Error, RelationKind,
    TypeId,
};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    check_flags as cf, modifier_flags as mf, symbol_flags as sf, JsString, SymbolTable,
    SyntaxKind as K,
};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.isValidSpreadType
    pub(crate) fn valid_spread_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        let ty = self
            .map_type(ty, &mut |checker, part| {
                Ok(Some(checker.base_constraint_of_type(part)?.unwrap_or(part)))
            })?
            .ok_or(Error::MissingLink("spread constraint"))?;
        let ty = self.type_with_facts(ty, facts::TRUTHY)?;
        let flags = self.types.flags(ty)?;
        if flags & (tf::ANY | tf::NON_PRIMITIVE | tf::OBJECT | tf::INSTANTIABLE_NON_PRIMITIVE) != 0
        {
            return Ok(true);
        }
        if flags & tf::UNION_OR_INTERSECTION != 0 {
            for &part in self.types.compound_types(ty)?.clone().iter() {
                if !self.valid_spread_type(part)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getRestType
    pub(crate) fn object_rest_type(
        &mut self,
        source: TypeId,
        properties: &[NodeId],
        symbol: Option<SymbolId>,
    ) -> Result<TypeId, Error> {
        let source = self.filter_type(source, &mut |checker, ty| {
            Ok(checker.types.flags(ty)? & tf::NULLABLE == 0)
        })?;
        if self.types.flags(source)? & tf::NEVER != 0 {
            return Ok(self.builtins.empty_object_type);
        }
        if self.types.flags(source)? & tf::UNION != 0 {
            return self
                .map_type(source, &mut |checker, ty| {
                    checker.object_rest_type(ty, properties, symbol).map(Some)
                })?
                .ok_or(Error::MissingLink("object rest union"));
        }
        let mut keys = Vec::with_capacity(properties.len());
        for &property in properties {
            keys.push(self.literal_type_from_property_name(property)?);
        }
        let mut omit = self.get_union_type(&keys)?;
        let mut spreadable = Vec::new();
        let mut omitted = Vec::new();
        for property in self.get_properties_of_type(source)? {
            let literal =
                self.literal_type_from_property(property, tf::STRING_OR_NUMBER_LITERAL_OR_UNIQUE)?;
            if !self.is_type_related_to(literal, omit, RelationKind::Assignable)?
                && self.rest_declaration_modifiers(property)?
                    & mf::NON_PUBLIC_ACCESSIBILITY_MODIFIER
                    == 0
                && self.spreadable_property(property)?
            {
                spreadable.push(property);
            } else {
                omitted.push(literal);
            }
        }
        if self.get_generic_object_flags(source)? & of::IS_GENERIC_OBJECT_TYPE != 0
            || self.is_generic_index_type(omit)?
        {
            if !omitted.is_empty() {
                omitted.insert(0, omit);
                omit = self.get_union_type(&omitted)?;
            }
            if self.types.flags(omit)? & tf::NEVER != 0 {
                return Ok(source);
            }
            let Some(symbol) = self.global_omit_symbol()? else {
                return Ok(self.builtins.error_type);
            };
            let ty = self.get_declared_type_of_symbol(symbol)?;
            let parameters = self
                .query
                .type_aliases
                .get_or_default(symbol)
                .parameters
                .clone()
                .ok_or(Error::MissingLink("Omit type parameters"))?;
            return self.type_alias_instantiation(symbol, ty, &parameters, &[source, omit], None);
        }
        let mut members = SymbolTable::default();
        for property in spreadable {
            let name = self.symbol(property)?.name_to_owned();
            let property = self.spread_symbol(property, false)?;
            members.insert(name, Some(property));
        }
        let members = self.alloc_symbol_table(members);
        let indices = self.index_infos_of_type(source)?;
        let result = self.new_anonymous_type(symbol, Some(members), &[], &[], &indices)?;
        self.types.get_mut(result)?.object_flags |= of::OBJECT_REST_TYPE;
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.isSpreadableProperty
    pub(crate) fn spreadable_property(&self, symbol: SymbolId) -> Result<bool, Error> {
        let flags = self.symbol(symbol)?.flags();
        let declarations = self.symbol_declarations(symbol)?;
        let mut private = false;
        let mut in_class = false;
        for declaration in declarations.iter().flatten() {
            private |= ts_ast::utilities::is_private_identifier_class_element_declaration(
                self.ast(declaration)?,
                declaration,
            )?;
            if let Some(parent) = self.ast(declaration)?.node(declaration)?.parent() {
                in_class |= matches!(
                    self.ast(parent)?.node(parent)?.kind().known(),
                    Some(K::ClassDeclaration | K::ClassExpression)
                );
            }
        }
        Ok(
            !private && flags & (sf::METHOD | sf::GET_ACCESSOR | sf::SET_ACCESSOR) == 0
                || !in_class,
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.getSpreadSymbol
    pub(crate) fn spread_symbol(
        &mut self,
        source: SymbolId,
        readonly: bool,
    ) -> Result<SymbolId, Error> {
        let read = self.symbol(source)?;
        let setonly = read.flags() & sf::SET_ACCESSOR != 0 && read.flags() & sf::GET_ACCESSOR == 0;
        if !setonly && readonly == self.is_readonly_symbol(source)? {
            return Ok(source);
        }
        let flags = sf::PROPERTY | (read.flags() & sf::OPTIONAL);
        let checks = read.check_flags() & cf::LATE | if readonly { cf::READONLY } else { 0 };
        let declarations = read.declarations();
        let name = read.name_to_owned();
        let result = self.new_symbol_ex(flags, name, checks)?;
        let ty = if setonly {
            self.builtins.undefined_type
        } else {
            self.get_type_of_symbol(source)?
        };
        let name_type = self
            .value_symbol_links
            .try_get(source)
            .and_then(|links| links.name_type);
        let links = self.value_symbol_links.get_or_default(result);
        links.resolved_type = Some(ty);
        links.name_type = name_type;
        self.symbol_mut(result)?.declarations = declarations;
        self.mapped_symbol_links
            .get_or_default(result)
            .synthetic_origin = Some(source);
        Ok(result)
    }

    // port: tsc/internal/checker/utilities.go:getDeclarationModifierFlagsFromSymbol
    pub(crate) fn rest_declaration_modifiers(&self, symbol: SymbolId) -> Result<u32, Error> {
        let read = self.symbol(symbol)?;
        let checks = read.check_flags();
        if checks & cf::SYNTHETIC != 0 {
            let flags = if checks & cf::CONTAINS_PUBLIC != 0 {
                mf::PUBLIC
            } else if checks & cf::CONTAINS_PROTECTED != 0 {
                mf::PROTECTED
            } else if checks & cf::CONTAINS_PRIVATE != 0 {
                mf::PRIVATE
            } else {
                0
            };
            return Ok(flags
                | if checks & cf::CONTAINS_STATIC != 0 {
                    mf::STATIC
                } else {
                    0
                });
        }
        if let Some(value) = read.value_declaration() {
            let mut declaration = value;
            if read.flags() & sf::GET_ACCESSOR != 0 {
                for node in self.symbol_declarations(symbol)?.iter().flatten() {
                    if self.ast(node)?.node(node)?.kind() == K::GetAccessor {
                        declaration = node;
                        break;
                    }
                }
            }
            let flags = ts_ast::utilities::get_combined_modifier_flags(
                self.ast(declaration)?,
                declaration,
            )?;
            let parent = read.parent();
            return Ok(
                if parent
                    .map(|parent| {
                        self.symbol(parent)
                            .map(|symbol| symbol.flags() & sf::CLASS != 0)
                    })
                    .transpose()?
                    .unwrap_or(false)
                {
                    flags
                } else {
                    flags & !mf::ACCESSIBILITY_MODIFIER
                },
            );
        }
        Ok(if read.flags() & sf::PROTOTYPE != 0 {
            mf::PUBLIC | mf::STATIC
        } else {
            0
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getGlobalTypeAliasSymbol
    fn global_omit_symbol(&mut self) -> Result<Option<SymbolId>, Error> {
        if let Some(symbol) = self.bindings.omit_symbol {
            return Ok(symbol);
        }
        let symbol = self.resolve_name(
            None,
            b"Omit",
            sf::TYPE_ALIAS,
            Some(d::Cannot_find_global_type_0),
            false,
        )?;
        let symbol = if let Some(symbol) = symbol {
            self.get_declared_type_of_symbol(symbol)?;
            let parameters = self
                .query
                .type_aliases
                .try_get(symbol)
                .and_then(|links| links.parameters.as_ref());
            if parameters.is_some_and(|p| p.len() == 2) {
                Some(symbol)
            } else {
                let declaration = self.declaration_of_kind(symbol, K::TypeAliasDeclaration)?;
                let name = self.symbol(symbol)?.name_to_owned();
                self.error_at(
                    declaration,
                    d::Global_type_0_must_have_1_type_parameter_s,
                    vec![name, JsString::from_bytes(b"2".as_slice())],
                )?;
                None
            }
        } else {
            None
        };
        self.bindings.omit_symbol = Some(symbol);
        Ok(symbol)
    }
}
