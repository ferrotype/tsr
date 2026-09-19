//! Variance is measured by structural comparison of marker instantiations.
//! Recursive computations restart at the smallest source symbol, preserving
//! upstream's stable result independently of the first reference encountered.
use crate::{
    object_flags as of, type_flags as tf, variance_flags as vf, CheckerState, Error, RelationKind,
    TypeId, TypeList, VarianceFlags,
};
use ts_arena::SymbolId;
use ts_ast::{modifier_flags as mf, symbol_flags as sf};

pub(crate) const REPORTS_UNMEASURABLE: u32 = 1 << 3;
pub(crate) const REPORTS_UNRELIABLE: u32 = 1 << 4;
pub(crate) const RELIABILITY: u32 = REPORTS_UNMEASURABLE | REPORTS_UNRELIABLE;

#[derive(Default)]
pub(crate) struct VarianceState {
    pub links: crate::types::Map<SymbolId, Vec<VarianceFlags>>,
    pub stack: Vec<(SymbolId, TypeList)>,
    pub markers: crate::types::Set<TypeId>,
    pub reliability: u32,
    pub checked_parameter: Option<TypeId>,
}
impl CheckerState {
    // port: tsc/internal/checker/relater.go:Checker.getVariances
    pub(crate) fn variances_of(&mut self, target: TypeId) -> Result<Vec<VarianceFlags>, Error> {
        if self.query.global_types.get("Array") == Some(&target)
            || self.query.global_types.get("ReadonlyArray") == Some(&target)
            || self.types.object_flags(target)? & of::TUPLE != 0
        {
            return Ok(vec![vf::COVARIANT]);
        }
        let symbol = self
            .types
            .get(target)?
            .symbol
            .ok_or(Error::MissingLink("variance target symbol"))?;
        let parameters: TypeList = self.types.interface(target)?.type_parameters().into();
        self.variances_worker(symbol, &parameters)
    }
    // port: tsc/internal/checker/relater.go:Checker.getAliasVariances
    pub(crate) fn alias_variances(
        &mut self,
        symbol: SymbolId,
    ) -> Result<Vec<VarianceFlags>, Error> {
        let parameters = self
            .query
            .type_aliases
            .try_get(symbol)
            .and_then(|links| links.parameters.clone())
            .ok_or(Error::MissingLink("alias type parameters"))?;
        self.variances_worker(symbol, &parameters)
    }
    // port: tsc/internal/checker/relater.go:Checker.getVariancesWorker
    fn variances_worker(
        &mut self,
        symbol: SymbolId,
        parameters: &TypeList,
    ) -> Result<Vec<VarianceFlags>, Error> {
        if let Some(variances) = self.variance.links.get(&symbol) {
            return Ok(variances.clone());
        }
        if let Some(start) = self.variance.stack.iter().position(|(s, _)| *s == symbol) {
            let mut smallest = start;
            for index in start + 1..self.variance.stack.len() {
                if self
                    .compare_symbols(
                        Some(self.variance.stack[index].0),
                        Some(self.variance.stack[smallest].0),
                    )?
                    .is_lt()
                {
                    smallest = index;
                }
            }
            if smallest > start {
                let stack = std::mem::take(&mut self.variance.stack);
                let result = self.variances_worker(stack[smallest].0, &stack[smallest].1);
                self.variance.stack = stack;
                result?;
            }
            return Ok(self.variance.links.entry(symbol).or_default().clone());
        }
        let saved_start = if self.variance.stack.is_empty() {
            let depth = self.resolution.depth();
            Some(self.resolution.set_resolution_start(depth))
        } else {
            None
        };
        self.variance.stack.push((symbol, parameters.clone()));
        let result = (|| {
            let mut variances = Vec::with_capacity(parameters.len());
            for &parameter in parameters.iter() {
                let modifiers = self.type_parameter_modifiers(parameter)?;
                let variance = if modifiers & mf::OUT != 0 {
                    if modifiers & mf::IN != 0 {
                        vf::INVARIANT
                    } else {
                        vf::COVARIANT
                    }
                } else if modifiers & mf::IN != 0 {
                    vf::CONTRAVARIANT
                } else {
                    let saved = std::mem::take(&mut self.variance.reliability);
                    let result = (|| {
                        let super_type = self.create_marker_type(
                            symbol,
                            parameter,
                            self.builtins.marker_super_type,
                        )?;
                        let sub_type = self.create_marker_type(
                            symbol,
                            parameter,
                            self.builtins.marker_sub_type,
                        )?;
                        let mut variance = if self.is_type_related_to(
                            sub_type,
                            super_type,
                            RelationKind::Assignable,
                        )? {
                            vf::COVARIANT
                        } else {
                            0
                        };
                        if self.is_type_related_to(
                            super_type,
                            sub_type,
                            RelationKind::Assignable,
                        )? {
                            variance |= vf::CONTRAVARIANT;
                        }
                        if variance == vf::BIVARIANT {
                            let other = self.create_marker_type(
                                symbol,
                                parameter,
                                self.builtins.marker_other_type,
                            )?;
                            if self.is_type_related_to(
                                other,
                                super_type,
                                RelationKind::Assignable,
                            )? {
                                variance = vf::INDEPENDENT;
                            }
                        }
                        if self.variance.reliability & REPORTS_UNMEASURABLE != 0 {
                            variance |= vf::UNMEASURABLE;
                        }
                        if self.variance.reliability & REPORTS_UNRELIABLE != 0 {
                            variance |= vf::UNRELIABLE;
                        }
                        Ok::<_, Error>(variance)
                    })();
                    self.variance.reliability = saved;
                    result?
                };
                if self
                    .variance
                    .links
                    .get(&symbol)
                    .is_some_and(|v| !v.is_empty())
                {
                    break;
                }
                variances.push(variance);
            }
            if self.variance.links.get(&symbol).is_none_or(Vec::is_empty) {
                self.variance.links.insert(symbol, variances);
            }
            Ok(self.variance.links[&symbol].clone())
        })();
        self.variance.stack.pop();
        if let Some(start) = saved_start {
            self.resolution.set_resolution_start(start);
        }
        if result.is_err() {
            self.variance.links.remove(&symbol);
        }
        result
    }
    // port: tsc/internal/checker/relater.go:Checker.createMarkerType
    pub(crate) fn create_marker_type(
        &mut self,
        symbol: SymbolId,
        source: TypeId,
        target: TypeId,
    ) -> Result<TypeId, Error> {
        let mapper = self.new_type_mapper(&[source], &[target])?;
        let ty = self.get_declared_type_of_symbol(symbol)?;
        if self.types.flags(ty)? & tf::ANY != 0 {
            return Ok(ty);
        }
        let result = if self.symbol(symbol)?.flags() & sf::TYPE_ALIAS != 0 {
            let parameters = self
                .query
                .type_aliases
                .try_get(symbol)
                .and_then(|l| l.parameters.clone())
                .ok_or(Error::MissingLink("marker alias parameters"))?;
            let args = self.instantiate_types(&parameters, Some(mapper))?;
            self.type_alias_instantiation(symbol, ty, &parameters, &args, None)?
        } else {
            let parameters: TypeList = self.types.interface(ty)?.type_parameters().into();
            let args = self.instantiate_types(&parameters, Some(mapper))?;
            self.create_type_reference(ty, &args)?
        };
        self.variance.markers.insert(result);
        Ok(result)
    }
    // port: tsc/internal/checker/relater.go:Checker.getTypeParameterModifiers
    pub(crate) fn type_parameter_modifiers(&self, ty: TypeId) -> Result<u32, Error> {
        let mut flags = 0;
        if let Some(symbol) = self.types.get(ty)?.symbol {
            for node in self.symbol_declarations(symbol)?.iter().flatten() {
                let view = self.ast(node)?;
                flags |= view.node(node)?.modifier_flags(view)?;
            }
        }
        Ok(flags & (mf::IN | mf::OUT | mf::CONST))
    }
    pub(crate) fn report_variance_marker(&mut self, ty: TypeId, flag: u32) {
        if [
            self.builtins.marker_super_type,
            self.builtins.marker_sub_type,
            self.builtins.marker_other_type,
        ]
        .contains(&ty)
        {
            self.variance.reliability |= flag;
        }
    }
}
