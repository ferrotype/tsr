//! Native promise adoption and recursive awaiting. Cache entries are nonowning
//! type identities; an unsuccessful computation never occupies a result slot.
use crate::{
    object_flags as of, type_facts as f, type_flags as tf, types::Map, CheckerState, Error,
    RelationKind, TypeId, UnionReduction,
};
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{symbol_flags as sf, JsString, SyntaxKind as K};
use tsr_diagnostics as d;

#[derive(Default)]
pub(crate) struct PromiseState {
    pub promised: Map<TypeId, TypeId>,
    pub awaited: Map<TypeId, TypeId>,
    pub stack: Vec<TypeId>,
    #[allow(
        clippy::option_option,
        reason = "Distinguish an unqueried symbol from a cached absent symbol"
    )]
    awaited_symbols: [Option<Option<SymbolId>>; 2],
    #[allow(
        clippy::option_option,
        reason = "Distinguish an unqueried symbol from a cached absent symbol"
    )]
    constructor_symbols: [Option<Option<SymbolId>>; 2],
}
impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.createPromiseLikeType
    pub(crate) fn create_promise_like_type(&mut self, promised: TypeId) -> Result<TypeId, Error> {
        let global = if let Some(&ty) = self.query.global_types.get("PromiseLikeChecked") {
            ty
        } else {
            let ty = self.get_global_type("PromiseLike", 1, true)?;
            self.query.global_types.insert("PromiseLikeChecked", ty);
            ty
        };
        if global == self.builtins.empty_generic_type {
            return Ok(self.builtins.unknown_type);
        }
        let promised = self
            .awaited_type_no_alias(promised)?
            .unwrap_or(self.builtins.unknown_type);
        self.create_type_reference(global, &[promised])
    }
    // port: tsc/internal/checker/checker.go:Checker.getGlobalValueSymbolResolver
    pub(crate) fn global_promise_constructor_symbol(
        &mut self,
        report: bool,
    ) -> Result<Option<SymbolId>, Error> {
        if let Some(symbol) = self.promises.constructor_symbols[usize::from(report)] {
            return Ok(symbol);
        }
        let symbol = self.resolve_name(
            None,
            b"Promise",
            sf::VALUE,
            report.then_some(d::Cannot_find_global_value_0),
            false,
        )?;
        self.promises.constructor_symbols[usize::from(report)] = Some(symbol);
        Ok(symbol)
    }
    pub(crate) fn global_promise_type(&mut self, report: bool) -> Result<TypeId, Error> {
        let key = if report { "PromiseChecked" } else { "Promise" };
        if let Some(&ty) = self.query.global_types.get(key) {
            return Ok(ty);
        }
        let ty = self.get_global_type("Promise", 1, report)?;
        self.query.global_types.insert(key, ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getGlobalTypeAliasSymbol
    pub(crate) fn global_awaited_symbol(
        &mut self,
        report: bool,
    ) -> Result<Option<SymbolId>, Error> {
        if let Some(symbol) = self.promises.awaited_symbols[usize::from(report)] {
            return Ok(symbol);
        }
        let symbol = self.resolve_name(
            None,
            b"Awaited",
            sf::TYPE_ALIAS,
            report.then_some(d::Cannot_find_global_type_0),
            false,
        )?;
        let symbol = if let Some(symbol) = symbol {
            self.get_declared_type_of_symbol(symbol)?;
            let parameters = self
                .query
                .type_aliases
                .try_get(symbol)
                .and_then(|links| links.parameters.as_ref());
            if parameters.is_some_and(|p| p.len() == 1) {
                Some(symbol)
            } else {
                if report {
                    let declaration = self.declaration_of_kind(symbol, K::TypeAliasDeclaration)?;
                    let name = self.symbol(symbol)?.name_to_owned();
                    self.error_at(
                        declaration,
                        d::Global_type_0_must_have_1_type_parameter_s,
                        vec![name, JsString::from_bytes(b"1".as_slice())],
                    )?;
                }
                None
            }
        } else {
            None
        };
        self.promises.awaited_symbols[usize::from(report)] = Some(symbol);
        Ok(symbol)
    }

    // port: tsc/internal/checker/checker.go:Checker.allTypesAssignableToKind
    pub(crate) fn all_assignable_to_kind(&mut self, ty: TypeId, mask: u32) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::UNION != 0 {
            for &part in self.types.compound_types(ty)?.clone().iter() {
                if !self.all_assignable_to_kind(part, mask)? {
                    return Ok(false);
                }
            }
            Ok(true)
        } else {
            self.type_assignable_to_kind(ty, mask)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.GetPromisedTypeOfPromise
    pub(crate) fn get_promised_type_of_promise(
        &mut self,
        ty: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        self.promised_type_ex(ty, None).map(|(ty, _)| ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getPromisedTypeOfPromiseEx
    fn promised_type_ex(
        &mut self,
        ty: TypeId,
        error: Option<NodeId>,
    ) -> Result<(Option<TypeId>, Option<TypeId>), Error> {
        if self.types.flags(ty)? & tf::ANY != 0 {
            return Ok((None, None));
        }
        if let Some(&cached) = self.promises.promised.get(&ty) {
            return Ok((Some(cached), None));
        }
        let promise = self.global_promise_type(false)?;
        if self.types.get(ty)?.object_flags & of::REFERENCE != 0
            && self.types.target(ty)? == promise
        {
            let result = self.get_type_arguments(ty)?[0];
            self.promises.promised.insert(ty, result);
            return Ok((Some(result), None));
        }
        let base = self.base_constraint_of_type(ty)?.unwrap_or(ty);
        if self.all_assignable_to_kind(base, tf::PRIMITIVE | tf::NEVER)? {
            return Ok((None, None));
        }
        let then = self.property_type(ty, b"then")?;
        if then
            .map(|t| self.types.flags(t).map(|flags| flags & tf::ANY != 0))
            .transpose()?
            .unwrap_or(false)
        {
            return Ok((None, None));
        }
        let signatures = match then {
            Some(then) => self.signatures_of_type(then, false)?,
            None => Vec::new(),
        };
        if signatures.is_empty() {
            if error.is_some() {
                self.error_at(error, d::A_promise_must_have_a_then_method, vec![])?;
            }
            return Ok((None, None));
        }
        let mut this_error = None;
        let mut candidates = Vec::new();
        for signature in signatures {
            let this = self
                .signatures
                .get(signature)?
                .this_parameter
                .map(|symbol| self.get_type_of_symbol(symbol))
                .transpose()?;
            if let Some(this) = this {
                if this != self.builtins.void_type
                    && !self.is_type_related_to(ty, this, RelationKind::Subtype)?
                {
                    this_error = Some(this);
                    continue;
                }
            }
            candidates.push(signature);
        }
        if candidates.is_empty() {
            if error.is_some() {
                let a = self.type_to_string(ty, crate::type_display::DEFAULT_FLAGS)?;
                let b = self.type_to_string(
                    this_error.ok_or(Error::MissingLink("promise this mismatch"))?,
                    crate::type_display::DEFAULT_FLAGS,
                )?;
                self.error_at(
                    error,
                    d::The_this_context_of_type_0_is_not_assignable_to_method_s_this_of_type_1,
                    vec![a, b],
                )?;
            }
            return Ok((None, this_error));
        }
        let mut first = Vec::new();
        for signature in candidates {
            first.push(self.first_parameter_type(signature)?);
        }
        let callback = self.get_union_type(&first)?;
        let callback = self.type_with_facts(callback, f::NE_UNDEFINED_OR_NULL)?;
        if self.types.flags(callback)? & tf::ANY != 0 {
            return Ok((None, None));
        }
        let signatures = self.signatures_of_type(callback, false)?;
        if signatures.is_empty() {
            if error.is_some() {
                self.error_at(
                    error,
                    d::The_first_parameter_of_the_then_method_of_a_promise_must_be_a_callback,
                    vec![],
                )?;
            }
            return Ok((None, None));
        }
        let mut values = Vec::new();
        for signature in signatures {
            values.push(self.first_parameter_type(signature)?);
        }
        let result = self.get_union_type_ex(&values, UnionReduction::Subtype, None, None)?;
        self.promises.promised.insert(ty, result);
        Ok((Some(result), None))
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeOfFirstParameterOfSignature
    fn first_parameter_type(&mut self, signature: crate::SignatureId) -> Result<TypeId, Error> {
        if self
            .signatures
            .get(signature)?
            .parameters
            .as_ref()
            .is_none_or(|p| p.is_empty())
        {
            return Ok(self.builtins.never_type);
        }
        self.parameter_type_at(signature, 0)?
            .ok_or(Error::MissingLink("first signature parameter"))
    }

    // port: tsc/internal/checker/checker.go:Checker.isThenableType
    pub(crate) fn is_thenable_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        let base = self.base_constraint_of_type(ty)?.unwrap_or(ty);
        if self.all_assignable_to_kind(base, tf::PRIMITIVE | tf::NEVER)? {
            return Ok(false);
        }
        if let Some(then) = self.property_type(ty, b"then")? {
            let then = self.type_with_facts(then, f::NE_UNDEFINED_OR_NULL)?;
            return Ok(!self.signatures_of_type(then, false)?.is_empty());
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.isAwaitedTypeInstantiation
    fn is_awaited_instantiation(&mut self, ty: TypeId) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::CONDITIONAL == 0 {
            return Ok(false);
        }
        let symbol = self.global_awaited_symbol(false)?;
        let alias = self
            .types
            .get(ty)?
            .alias
            .map(|alias| self.types.alias(alias))
            .transpose()?;
        Ok(symbol.is_some()
            && alias.is_some_and(|alias| {
                Some(alias.symbol) == symbol && alias.type_arguments.len() == 1
            }))
    }

    // port: tsc/internal/checker/checker.go:Checker.isAwaitedTypeNeeded
    fn awaited_type_needed(&mut self, ty: TypeId) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::ANY != 0 || self.is_awaited_instantiation(ty)? {
            return Ok(false);
        }
        if self.get_generic_object_flags(ty)? & of::IS_GENERIC_OBJECT_TYPE == 0 {
            return Ok(false);
        }
        if let Some(base) = self.base_constraint_of_type(ty)? {
            if self.types.flags(base)? & tf::ANY_OR_UNKNOWN != 0 || self.empty_object_type(base)? {
                return Ok(true);
            }
            let parts = if self.types.flags(base)? & tf::UNION != 0 {
                self.types.compound_types(base)?.clone()
            } else {
                vec![base].into()
            };
            for &part in parts.iter() {
                if self.is_thenable_type(part)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        self.maybe_type_of_kind(ty, tf::TYPE_VARIABLE)
    }

    // port: tsc/internal/checker/checker.go:Checker.getAwaitedTypeNoAlias
    pub(crate) fn awaited_type_no_alias(&mut self, ty: TypeId) -> Result<Option<TypeId>, Error> {
        self.awaited_type_no_alias_ex(ty, None, None, &[])
    }

    // port: tsc/internal/checker/checker.go:Checker.getAwaitedTypeNoAliasEx
    pub(crate) fn awaited_type_no_alias_ex(
        &mut self,
        ty: TypeId,
        error: Option<NodeId>,
        message: Option<&'static d::Message>,
        args: &[JsString],
    ) -> Result<Option<TypeId>, Error> {
        if self.types.flags(ty)? & tf::ANY != 0 || self.is_awaited_instantiation(ty)? {
            return Ok(Some(ty));
        }
        if let Some(&cached) = self.promises.awaited.get(&ty) {
            return Ok(Some(cached));
        }
        if self.types.flags(ty)? & tf::UNION != 0 {
            if self.promises.stack.contains(&ty) {
                return self.awaited_cycle(error);
            }
            self.promises.stack.push(ty);
            let result = self.map_type(ty, &mut |c, part| {
                c.awaited_type_no_alias_ex(part, error, message, args)
            });
            self.promises.stack.pop();
            let result = result?;
            if let Some(result) = result {
                self.promises.awaited.insert(ty, result);
            }
            return Ok(result);
        }
        if self.awaited_type_needed(ty)? {
            self.promises.awaited.insert(ty, ty);
            return Ok(Some(ty));
        }
        let (promised, this_error) = self.promised_type_ex(ty, None)?;
        if let Some(promised) = promised {
            if ty == promised || self.promises.stack.contains(&promised) {
                return self.awaited_cycle(error);
            }
            self.promises.stack.push(ty);
            let result = self.awaited_type_no_alias_ex(promised, error, message, args);
            self.promises.stack.pop();
            let result = result?;
            if let Some(result) = result {
                self.promises.awaited.insert(ty, result);
            }
            return Ok(result);
        }
        if self.is_thenable_type(ty)? {
            if let Some(error) = error {
                let child = if let Some(this) = this_error {
                    let a = self.type_to_string(ty, crate::type_display::DEFAULT_FLAGS)?;
                    let b = self.type_to_string(this, crate::type_display::DEFAULT_FLAGS)?;
                    Some(std::sync::Arc::new(self.diagnostic_for_node(
                        Some(error),
                        d::The_this_context_of_type_0_is_not_assignable_to_method_s_this_of_type_1,
                        vec![a, b],
                    )?))
                } else {
                    None
                };
                let message = message.ok_or(Error::MissingLink("invalid thenable diagnostic"))?;
                let diagnostic = if child.is_some() {
                    tsr_ast::Diagnostic::chain(child, message, args.to_vec())
                } else {
                    self.diagnostic_for_node(Some(error), message, args.to_vec())?
                };
                self.add_diagnostic(diagnostic)?;
            }
            return Ok(None);
        }
        self.promises.awaited.insert(ty, ty);
        Ok(Some(ty))
    }

    fn awaited_cycle(&mut self, error: Option<NodeId>) -> Result<Option<TypeId>, Error> {
        if error.is_some() {
            self.error_at(error,d::Type_is_referenced_directly_or_indirectly_in_the_fulfillment_callback_of_its_own_then_method,vec![])?;
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.unwrapAwaitedType
    pub(crate) fn unwrap_awaited_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if self.types.flags(ty)? & tf::UNION != 0 {
            return self
                .map_type(ty, &mut |c, t| c.unwrap_awaited_type(t).map(Some))?
                .ok_or(Error::MissingLink("unwrap awaited union"));
        }
        if self.is_awaited_instantiation(ty)? {
            return Ok(self
                .types
                .alias(
                    self.types
                        .get(ty)?
                        .alias
                        .ok_or(Error::MissingLink("awaited alias"))?,
                )?
                .type_arguments[0]);
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.createAwaitedTypeIfNeeded
    pub(crate) fn create_awaited_type_if_needed(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if self.awaited_type_needed(ty)? {
            if let Some(symbol) = self.global_awaited_symbol(true)? {
                let argument = self.unwrap_awaited_type(ty)?;
                let declared = self.get_declared_type_of_symbol(symbol)?;
                let parameters = self
                    .query
                    .type_aliases
                    .try_get(symbol)
                    .and_then(|links| links.parameters.clone())
                    .ok_or(Error::MissingLink("Awaited type parameters"))?;
                return self.type_alias_instantiation(
                    symbol,
                    declared,
                    &parameters,
                    &[argument],
                    None,
                );
            }
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getAwaitedType
    pub(crate) fn awaited_type(&mut self, ty: TypeId) -> Result<Option<TypeId>, Error> {
        self.awaited_type_no_alias(ty)?
            .map(|ty| self.create_awaited_type_if_needed(ty))
            .transpose()
    }

    // port: tsc/internal/checker/checker.go:Checker.getAwaitedTypeEx
    pub(crate) fn awaited_type_ex(
        &mut self,
        ty: TypeId,
        error: Option<NodeId>,
        message: Option<&'static d::Message>,
        args: &[JsString],
    ) -> Result<Option<TypeId>, Error> {
        self.awaited_type_no_alias_ex(ty, error, message, args)?
            .map(|ty| self.create_awaited_type_if_needed(ty))
            .transpose()
    }

    // port: tsc/internal/checker/checker.go:Checker.getAwaitedTypeOfPromise
    pub(crate) fn awaited_type_of_promise(&mut self, ty: TypeId) -> Result<Option<TypeId>, Error> {
        self.get_promised_type_of_promise(ty)?
            .map(|ty| self.awaited_type(ty))
            .transpose()
            .map(Option::flatten)
    }

    // port: tsc/internal/checker/checker.go:Checker.createPromiseType
    pub(crate) fn create_promise_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let promise = self.global_promise_type(true)?;
        if promise == self.builtins.empty_generic_type {
            return Ok(self.builtins.unknown_type);
        }
        let ty = self.unwrap_awaited_type(ty)?;
        let ty = self
            .awaited_type_no_alias(ty)?
            .unwrap_or(self.builtins.unknown_type);
        self.create_type_reference(promise, &[ty])
    }
}
