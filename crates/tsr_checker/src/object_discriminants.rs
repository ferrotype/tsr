//! Contextual discrimination tests only expressions whose type can be read
//! without their context, preventing a contextual-type recursion cycle.
use crate::{type_flags as tf, CheckerState, Error, TypeId, UnionReduction};
use tsr_arena::NodeId;
use tsr_ast::{symbol_flags as sf, JsString, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.discriminateContextualTypeByObjectMembers
    pub(crate) fn discriminate_object_context(
        &mut self,
        node: NodeId,
        context: TypeId,
    ) -> Result<TypeId, Error> {
        if let Some(&ty) = self.bindings.discriminated_contexts.get(&(node, context)) {
            return Ok(ty);
        }
        let properties = self.source_list(node, self.node(node)?.property_list())?;
        let matching = self.matching_object_union_constituent(context, &properties)?;
        let ty = if let Some(ty) = matching {
            ty
        } else {
            let mut discriminants = Vec::new();
            for property in properties {
                let Some(symbol) = self.raw_declaration_symbol(property)? else {
                    continue;
                };
                let name = self.symbol(symbol)?.name_to_owned();
                let read = self.node(property)?;
                let expression = if read.kind() == K::PropertyAssignment {
                    let expression = read
                        .initializer()
                        .ok_or(Error::MissingLink("discriminant initializer"))?;
                    if !self.possible_object_discriminant(expression)? {
                        continue;
                    }
                    expression
                } else if read.kind() == K::ShorthandPropertyAssignment {
                    read.name()
                        .ok_or(Error::MissingLink("shorthand discriminant"))?
                } else {
                    continue;
                };
                if self.discriminant_property(context, name.as_bytes())? {
                    discriminants.push((name, Some(expression)));
                }
            }
            let symbol = self
                .get_symbol_of_declaration(node)?
                .ok_or(Error::MissingLink("object context symbol"))?;
            let members = self.symbol(symbol)?.members();
            for property in self.get_properties_of_type(context)? {
                let read = self.symbol(property)?;
                let name = read.name_to_owned();
                if read.flags() & sf::OPTIONAL != 0
                    && self.member_symbol(members, name.as_bytes())?.is_none()
                    && self.discriminant_property(context, name.as_bytes())?
                {
                    discriminants.push((name, None));
                }
            }
            self.discriminate_object_items(context, &discriminants)?
        };
        self.bindings
            .discriminated_contexts
            .insert((node, context), ty);
        Ok(ty)
    }
    // port: tsc/internal/checker/checker.go:Checker.getMatchingUnionConstituentForObjectLiteral
    fn matching_object_union_constituent(
        &mut self,
        union: TypeId,
        properties: &[NodeId],
    ) -> Result<Option<TypeId>, Error> {
        if self.types.union(union)?.key_property_name.is_none() {
            self.compute_key_property_name(union)?;
        }
        let name = self
            .types
            .union(union)?
            .key_property_name
            .clone()
            .unwrap_or_default();
        if name.as_bytes().is_empty() {
            return Ok(None);
        }
        for &property in properties {
            if self.node(property)?.kind() != K::PropertyAssignment {
                continue;
            }
            let Some(symbol) = self.raw_declaration_symbol(property)? else {
                continue;
            };
            if self.symbol(symbol)?.name_bytes() != name.as_bytes() {
                continue;
            }
            let expression = self
                .ast(property)?
                .node(property)?
                .initializer()
                .ok_or(Error::MissingLink("key discriminant"))?;
            if self.possible_object_discriminant(expression)? {
                let key = self.get_context_free_type_of_expression(expression)?;
                let key = self.get_regular_type_of_literal_type(key)?;
                return Ok(self
                    .types
                    .union(union)?
                    .constituent_map
                    .as_ref()
                    .and_then(|map| map.get(&key))
                    .copied()
                    .filter(|&ty| ty != self.builtins.unknown_type));
            }
        }
        Ok(None)
    }
    // port: tsc/internal/checker/checker.go:Checker.isPossiblyDiscriminantValue
    fn possible_object_discriminant(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.node(node)?;
        match read.kind().known() {
            Some(
                K::StringLiteral
                | K::NumericLiteral
                | K::BigIntLiteral
                | K::NoSubstitutionTemplateLiteral
                | K::TemplateExpression
                | K::TrueKeyword
                | K::FalseKeyword
                | K::NullKeyword
                | K::Identifier
                | K::UndefinedKeyword,
            ) => Ok(true),
            Some(K::PropertyAccessExpression | K::ParenthesizedExpression) => self
                .possible_object_discriminant(
                    read.expression()
                        .ok_or(Error::MissingLink("discriminant expression"))?,
                ),
            Some(K::JsxExpression) => read
                .expression()
                .map(|node| self.possible_object_discriminant(node))
                .transpose()
                .map(|result| result.unwrap_or(true)),
            _ => Ok(false),
        }
    }
    // port: tsc/internal/checker/relater.go:Checker.discriminateTypeByDiscriminableItems
    fn discriminate_object_items(
        &mut self,
        context: TypeId,
        items: &[(JsString, Option<NodeId>)],
    ) -> Result<TypeId, Error> {
        let types = self.types.compound_types(context)?.clone();
        let mut include = Vec::with_capacity(types.len());
        for &ty in types.iter() {
            let reduced = self.get_reduced_type(ty)?;
            include.push(u8::from(
                self.types.flags(ty)? & tf::PRIMITIVE == 0
                    && self.types.flags(reduced)? & tf::NEVER == 0,
            ));
        }
        for (name, expression) in items {
            let mut matched = false;
            for (index, &ty) in types.iter().enumerate() {
                if include[index] == 0 {
                    continue;
                }
                if let Some(target) = self.property_or_index_type(ty, name.as_bytes())? {
                    let source = match expression {
                        Some(node) => self.get_context_free_type_of_expression(*node)?,
                        None => self.builtins.undefined_type,
                    };
                    let mut current = false;
                    for part in self.distributed_types(source)? {
                        if self.is_type_related_to(part, target, crate::RelationKind::Assignable)? {
                            current = true;
                            break;
                        }
                    }
                    if current {
                        matched = true;
                    } else {
                        include[index] = 2;
                    }
                }
            }
            for included in &mut include {
                if *included == 2 {
                    *included = u8::from(!matched);
                }
            }
        }
        if include.contains(&0) {
            let selected = types
                .iter()
                .zip(include)
                .filter_map(|(&ty, include)| (include == 1).then_some(ty))
                .collect::<Vec<_>>();
            let result = self.get_union_type_ex(&selected, UnionReduction::None, None, None)?;
            if self.types.flags(result)? & tf::NEVER == 0 {
                return Ok(result);
            }
        }
        Ok(context)
    }
}
