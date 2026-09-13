//! Source type parameters, enclosing scopes and lazy defaults. Type parameters
//! are declared before their constraints/defaults are requested, preserving
//! recursive generic identities and the pinned checker's resolution order.

use crate::{CheckerState, Error, TypeId, TypeList};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{node_flags as nf, SyntaxKind as K};

#[derive(Default)]
pub(crate) struct TypeAliasLinks {
    pub(crate) parameters: Option<TypeList>,
    pub(crate) instantiations: crate::types::Map<crate::CacheKey, TypeId>,
}

impl CheckerState {
    pub(crate) fn source_children(&self, node: NodeId) -> Result<Vec<NodeId>, Error> {
        use std::ops::ControlFlow;
        struct Collector<'a> {
            view: ts_ast::AstView<'a>,
            nodes: Vec<NodeId>,
            error: Option<ts_arena::Error>,
        }
        impl ts_ast::ChildVisitor for Collector<'_> {
            fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
                self.nodes.push(node);
                ControlFlow::Continue(())
            }
            fn visit_list(&mut self, list: ts_ast::NodeListId) -> ControlFlow<()> {
                match self.view.list(list) {
                    Ok(list) => self.visit_node_slice(list.nodes()),
                    Err(error) => {
                        self.error = Some(error);
                        ControlFlow::Break(())
                    }
                }
            }
            fn visit_node_slice(&mut self, nodes: ts_ast::NodeSlice) -> ControlFlow<()> {
                match self.view.node_slice(nodes) {
                    Ok(nodes) => {
                        self.nodes.extend(nodes.iter().flatten());
                        ControlFlow::Continue(())
                    }
                    Err(error) => {
                        self.error = Some(error);
                        ControlFlow::Break(())
                    }
                }
            }
        }
        let view = self.ast(node)?;
        let mut collector = Collector {
            view,
            nodes: Vec::new(),
            error: None,
        };
        let _ = view.node(node)?.for_each_child(&mut collector);
        match collector.error {
            Some(error) => Err(error.into()),
            None => Ok(collector.nodes),
        }
    }

    pub(crate) fn source_list(
        &self,
        owner: NodeId,
        list: Option<ts_ast::NodeListId>,
    ) -> Result<Vec<NodeId>, Error> {
        let Some(list) = list else {
            return Ok(Vec::new());
        };
        let view = self.ast(owner)?;
        view.node_slice(view.list(list)?.nodes())?
            .iter()
            .map(|node| node.ok_or(Error::MissingLink("source list element")))
            .collect()
    }

    // port: tsc/internal/checker/checker.go:Checker.getDeclaredTypeOfTypeParameter
    pub(crate) fn get_declared_type_of_type_parameter(
        &mut self,
        symbol: SymbolId,
    ) -> Result<TypeId, Error> {
        if let Some(Some(ty)) = self.query.declared_types.try_get(symbol) {
            return Ok(*ty);
        }
        let ty = self.new_type_parameter(Some(symbol))?;
        *self.query.declared_types.get_or_default(symbol) = Some(ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeParametersFromDeclaration
    pub(crate) fn type_parameters_from_declaration(
        &mut self,
        node: NodeId,
    ) -> Result<TypeList, Error> {
        let read = self.ast(node)?.node(node)?;
        if read.flags() & nf::JAVA_SCRIPT_FILE != 0 {
            if let Some(signature) = self.signature_of_full_signature(node)? {
                return Ok(self
                    .signatures
                    .get(signature)?
                    .type_parameters
                    .clone()
                    .unwrap_or_default());
            }
        }
        let read = self.ast(node)?.node(node)?;
        let nodes = self.source_list(node, read.type_parameter_list())?;
        let mut result = Vec::with_capacity(nodes.len());
        for node in nodes {
            let symbol = self
                .get_symbol_of_declaration(node)?
                .ok_or(Error::MissingLink("type parameter symbol"))?;
            let ty = self.get_declared_type_of_type_parameter(symbol)?;
            if !result.contains(&ty) {
                result.push(ty);
            }
        }
        Ok(result.into())
    }

    // port: tsc/internal/checker/checker.go:Checker.getLocalTypeParametersOfClassOrInterfaceOrTypeAlias
    // port: tsc/internal/checker/checker.go:Checker.appendLocalTypeParametersOfClassOrInterfaceOrTypeAlias
    pub(crate) fn get_local_type_parameters(
        &mut self,
        symbol: SymbolId,
    ) -> Result<TypeList, Error> {
        let declarations = self.symbol_declarations(symbol)?.to_vec();
        let mut result = Vec::new();
        for node in declarations.into_iter().flatten() {
            if matches!(
                self.ast(node)?.node(node)?.kind().known(),
                Some(
                    K::InterfaceDeclaration
                        | K::ClassDeclaration
                        | K::ClassExpression
                        | K::TypeAliasDeclaration
                        | K::JSTypeAliasDeclaration
                )
            ) {
                for &parameter in self.type_parameters_from_declaration(node)?.iter() {
                    if !result.contains(&parameter) {
                        result.push(parameter);
                    }
                }
            }
        }
        Ok(result.into())
    }

    // port: tsc/internal/checker/checker.go:Checker.getOuterTypeParameters
    pub(crate) fn get_outer_type_parameters(
        &mut self,
        node: NodeId,
        include_this: bool,
    ) -> Result<TypeList, Error> {
        let mut ancestors = Vec::new();
        let mut parent = self.ast(node)?.node(node)?.parent();
        while let Some(node) = parent {
            let read = self.ast(node)?.node(node)?;
            if matches!(
                read.kind().known(),
                Some(
                    K::ClassDeclaration
                        | K::ClassExpression
                        | K::InterfaceDeclaration
                        | K::CallSignature
                        | K::ConstructSignature
                        | K::MethodSignature
                        | K::FunctionType
                        | K::ConstructorType
                        | K::FunctionDeclaration
                        | K::MethodDeclaration
                        | K::FunctionExpression
                        | K::ArrowFunction
                        | K::TypeAliasDeclaration
                        | K::JSTypeAliasDeclaration
                        | K::MappedType
                        | K::ConditionalType
                )
            ) {
                ancestors.push(node);
            }
            parent = read.parent();
        }
        let mut result = Vec::new();
        for node in ancestors.into_iter().rev() {
            let kind = self.ast(node)?.node(node)?.kind();
            if kind == K::MappedType {
                let ty = self.source_mapped_type(node)?;
                result.push(self.mapped_parameter(ty)?);
                continue;
            }
            if kind == K::ConditionalType {
                result.extend_from_slice(&self.infer_type_parameters(node)?);
                continue;
            }
            for &parameter in self.type_parameters_from_declaration(node)?.iter() {
                if !result.contains(&parameter) {
                    result.push(parameter);
                }
            }
            if include_this
                && matches!(
                    kind.known(),
                    Some(K::ClassDeclaration | K::ClassExpression | K::InterfaceDeclaration)
                )
            {
                let symbol = self
                    .get_symbol_of_declaration(node)?
                    .ok_or(Error::MissingLink("enclosing type symbol"))?;
                let ty = self.get_declared_type_of_symbol(symbol)?;
                if let Some(this) = self.types.interface(ty)?.this_type {
                    result.push(this);
                }
            }
        }
        Ok(result.into())
    }

    // port: tsc/internal/checker/checker.go:Checker.hasTypeParameterDefault
    fn type_parameter_default_node(&self, ty: TypeId) -> Result<Option<NodeId>, Error> {
        let Some(symbol) = self.types.get(ty)?.symbol else {
            return Ok(None);
        };
        for node in self.symbol_declarations(symbol)?.iter().flatten() {
            let read = self.ast(node)?.node(node)?;
            if let Some(parameter) = read.data_source().as_type_parameter_declaration() {
                if let Some(default) = parameter.default_type() {
                    return Ok(Some(default));
                }
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getMinTypeArgumentCount
    pub(crate) fn min_type_argument_count(&self, parameters: &[TypeId]) -> Result<usize, Error> {
        let mut minimum = 0;
        for (index, &parameter) in parameters.iter().enumerate() {
            if self.type_parameter_default_node(parameter)?.is_none() {
                minimum = index + 1;
            }
        }
        Ok(minimum)
    }

    // port: tsc/internal/checker/checker.go:Checker.getResolvedTypeParameterDefault
    pub(crate) fn resolved_type_parameter_default(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let parameter = self.types.type_parameter(ty)?;
        if let Some(default) = parameter.resolved_default_type {
            if default == self.builtins.resolving_default_type {
                self.types.type_parameter_mut(ty)?.resolved_default_type =
                    Some(self.builtins.circular_constraint_type);
                return Ok(self.builtins.circular_constraint_type);
            }
            return Ok(default);
        }
        let target = parameter.target;
        let mapper = parameter.mapper;
        self.types.type_parameter_mut(ty)?.resolved_default_type =
            Some(self.builtins.resolving_default_type);
        let result = (|| {
            if let Some(target) = target {
                let target_default = self.resolved_type_parameter_default(target)?;
                self.instantiate_type(target_default, mapper)
            } else if let Some(node) = self.type_parameter_default_node(ty)? {
                self.get_type_from_type_node(node)
            } else {
                Ok(self.builtins.no_constraint_type)
            }
        })();
        match result {
            Ok(default) => {
                let resolving = self.builtins.resolving_default_type;
                let slot = &mut self.types.type_parameter_mut(ty)?.resolved_default_type;
                if *slot == Some(resolving) {
                    *slot = Some(default);
                }
                Ok(slot.expect("default was assigned before resolution"))
            }
            Err(error) => {
                self.types.type_parameter_mut(ty)?.resolved_default_type = None;
                Err(error)
            }
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.fillMissingTypeArguments
    pub(crate) fn fill_missing_type_arguments(
        &mut self,
        arguments: &[TypeId],
        parameters: &[TypeId],
        in_js: bool,
    ) -> Result<TypeList, Error> {
        if parameters.is_empty() {
            return Ok([].into());
        }
        if !in_js && arguments.len() >= parameters.len() {
            return Ok(arguments.into());
        }
        let mut result = vec![self.builtins.error_type; parameters.len()];
        let count = arguments.len().min(parameters.len());
        result[..count].copy_from_slice(&arguments[..count]);
        for index in count..parameters.len() {
            let default = self.resolved_type_parameter_default(parameters[index])?;
            result[index] = if default == self.builtins.no_constraint_type
                || default == self.builtins.circular_constraint_type
            {
                if in_js {
                    self.builtins.any_type
                } else {
                    self.builtins.unknown_type
                }
            } else {
                let mapper = self.new_type_mapper(parameters, &result)?;
                self.instantiate_type(default, Some(mapper))?
            };
        }
        Ok(result.into())
    }
}
