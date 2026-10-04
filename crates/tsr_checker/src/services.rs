//! The language-service queries of `tsc/internal/checker/services.go`. Each
//! exported function is a method of the checker operation over owner-bound
//! handles (ADR 0008); the internal helpers are checker methods. Results the
//! pin collects from a symbol table (`symbolsToArray`, `maps.Values`) carry no
//! order there; here they keep the table's order, which is deterministic.

use crate::handles::{SignatureRef, SymbolRef, TypeRef};
use crate::{
    calls::Resolution, construct::is_reserved_member_name, type_flags as tf, CheckerState, Error,
    Operation, SignatureId, TypeId,
};
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{
    check_flags as cf, internal_symbol_names as names, symbol_flags as sf, JsString, NodeKind,
    SymbolFlags, SymbolTableId, SyntaxKind as K,
};

/// The JSDoc of the checked files, read through the checker's host, for the
/// token navigation `GetTouchingPropertyName` performs.
struct HostJsDoc<'h>(&'h dyn crate::CheckerHost);

impl tsr_ast::JsDocProvider for HostJsDoc<'_> {
    fn jsdoc(
        &mut self,
        view: tsr_ast::AstView<'_>,
        source: NodeId,
        parent: NodeId,
    ) -> Result<tsr_ast::JSDocRoots, tsr_arena::Error> {
        self.0
            .jsdoc(view, source, parent)
            .map_err(|error| match error {
                Error::Arena(error) => error,
                _ => tsr_arena::Error::InvalidGraph,
            })
    }
}

// port: tsc/internal/checker/services.go:isKnownGenericTypeName
fn is_known_generic_type_name(name: &[u8]) -> bool {
    matches!(
        name,
        b"Array"
            | b"ArrayLike"
            | b"ReadonlyArray"
            | b"Promise"
            | b"PromiseLike"
            | b"Iterable"
            | b"IterableIterator"
            | b"AsyncIterable"
            | b"Set"
            | b"WeakSet"
            | b"ReadonlySet"
            | b"Map"
            | b"WeakMap"
            | b"ReadonlyMap"
            | b"Partial"
            | b"Required"
            | b"Readonly"
            | b"Pick"
            | b"Omit"
            | b"NonNullable"
    )
}

// port: tsc/internal/checker/utilities.go:introducesArgumentsExoticObject
fn introduces_arguments_exotic_object(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::MethodDeclaration
                | K::MethodSignature
                | K::Constructor
                | K::GetAccessor
                | K::SetAccessor
                | K::FunctionDeclaration
                | K::FunctionExpression
        )
    )
}

/// The ordered, first-wins symbol table `getSymbolsInScope` fills.
#[derive(Default)]
struct ScopeSymbols {
    names: crate::types::Set<JsString>,
    symbols: Vec<(JsString, SymbolId)>,
}

impl ScopeSymbols {
    fn insert(&mut self, name: JsString, symbol: SymbolId) {
        if self.names.insert(name.clone()) {
            self.symbols.push((name, symbol));
        }
    }
}

impl CheckerState {
    // port: tsc/internal/checker/services.go:Checker.getSymbolsInScope
    pub(crate) fn symbols_in_scope(
        &mut self,
        location: NodeId,
        meaning: SymbolFlags,
    ) -> Result<Vec<SymbolId>, Error> {
        if self.node(location)?.flags() & tsr_ast::node_flags::IN_WITH_STATEMENT != 0 {
            // We cannot answer semantic questions within a with block, do not proceed any further
            return Ok(Vec::new());
        }
        let mut symbols = ScopeSymbols::default();
        let mut is_static_symbol = false;
        let mut last_location: Option<NodeId> = None;
        let mut current = Some(location);
        while let Some(location) = current {
            let read = self.node(location)?;
            let kind = read.kind();
            let parent = read.parent();
            if kind == K::ModuleDeclaration
                && read.attributes().is_some()
                && last_location == read.attributes()
            {
                // Module declaration is not in scope inside its attributes.
                last_location = Some(location);
                current = parent;
                continue;
            }
            let view = self.ast(location)?;
            let is_global_source_file =
                tsr_ast::utilities_middle::is_global_source_file(view, location)?;
            let is_static = tsr_ast::utilities::is_static(view, location)?;
            let is_external_module = kind == K::SourceFile
                && tsr_ast::utilities::is_external_module(&*self.source_file_read(location)?);
            if !is_global_source_file {
                if let Some(locals) = self
                    .program()?
                    .bound(location)?
                    .node_binding(location)?
                    .and_then(|binding| binding.locals)
                {
                    self.copy_scope_symbols(&mut symbols, Some(locals), meaning)?;
                }
            }
            match kind.known() {
                Some(K::SourceFile | K::ModuleDeclaration) => {
                    if kind == K::ModuleDeclaration || is_external_module {
                        let exports = match self.get_symbol_of_declaration(location)? {
                            Some(symbol) => self.symbol(symbol)?.exports(),
                            None => None,
                        };
                        self.copy_locally_visible_export_symbols(
                            &mut symbols,
                            exports,
                            meaning & sf::MODULE_MEMBER,
                        )?;
                    }
                }
                Some(K::EnumDeclaration) => {
                    let exports = match self.get_symbol_of_declaration(location)? {
                        Some(symbol) => self.symbol(symbol)?.exports(),
                        None => None,
                    };
                    self.copy_scope_symbols(&mut symbols, exports, meaning & sf::ENUM_MEMBER)?;
                }
                Some(K::ClassExpression | K::ClassDeclaration | K::InterfaceDeclaration) => {
                    if kind == K::ClassExpression && read.name().is_some() {
                        if let Some(symbol) = self.node_symbol(location)? {
                            self.copy_scope_symbol(&mut symbols, symbol, meaning)?;
                        }
                    }
                    // If we didn't come from static member of class or interface,
                    // add the type parameters into the symbol table
                    // (type parameters of classDeclaration/classExpression and interface are in member property of the symbol.
                    // Note: that the memberFlags come from previous iteration.
                    if !is_static_symbol {
                        if let Some(symbol) = self.get_symbol_of_declaration(location)? {
                            let members = self.members_of_symbol(symbol)?;
                            self.copy_scope_symbols(&mut symbols, members, meaning & sf::TYPE)?;
                        }
                    }
                }
                Some(K::FunctionExpression) if read.name().is_some() => {
                    if let Some(symbol) = self.node_symbol(location)? {
                        self.copy_scope_symbol(&mut symbols, symbol, meaning)?;
                    }
                }
                _ => {}
            }
            if introduces_arguments_exotic_object(kind) {
                let arguments = self.builtins.arguments_symbol;
                self.copy_scope_symbol(&mut symbols, arguments, meaning)?;
            }
            is_static_symbol = is_static;
            last_location = Some(location);
            current = parent;
        }
        let globals = self.builtins.globals;
        self.copy_scope_symbols(&mut symbols, globals, meaning)?;
        // `this` is not a symbol, a keyword
        Ok(symbols
            .symbols
            .into_iter()
            .filter(|(name, _)| {
                name.as_bytes() != names::THIS && !is_reserved_member_name(name.as_bytes())
            })
            .map(|(_, symbol)| symbol)
            .collect())
    }

    /// `location.Symbol()`: the binder's symbol of a declaration node.
    fn node_symbol(&self, node: NodeId) -> Result<Option<SymbolId>, Error> {
        Ok(self
            .program()?
            .bound(node)?
            .node_binding(node)?
            .and_then(|binding| binding.symbol))
    }

    fn copy_scope_symbol(
        &self,
        symbols: &mut ScopeSymbols,
        symbol: SymbolId,
        meaning: SymbolFlags,
    ) -> Result<(), Error> {
        let read = self.symbol(symbol)?;
        let mut flags = read.flags();
        if let Some(export) = read.export_symbol() {
            flags |= self.symbol(export)?.flags();
        }
        if flags & meaning != 0 {
            // We will copy all symbol regardless of its reserved name because
            // symbolsToArray will check whether the key is a reserved name and
            // it will not copy symbol with reserved name to the array
            symbols.insert(self.symbol(symbol)?.name_to_owned(), symbol);
        }
        Ok(())
    }

    fn copy_scope_symbols(
        &self,
        symbols: &mut ScopeSymbols,
        source: Option<SymbolTableId>,
        meaning: SymbolFlags,
    ) -> Result<(), Error> {
        let Some(source) = source.filter(|_| meaning != 0) else {
            return Ok(());
        };
        let entries: Vec<SymbolId> = self.table(source)?.symbols().flatten().collect();
        for symbol in entries {
            self.copy_scope_symbol(symbols, symbol, meaning)?;
        }
        Ok(())
    }

    fn copy_locally_visible_export_symbols(
        &self,
        symbols: &mut ScopeSymbols,
        source: Option<SymbolTableId>,
        meaning: SymbolFlags,
    ) -> Result<(), Error> {
        let Some(source) = source.filter(|_| meaning != 0) else {
            return Ok(());
        };
        let entries: Vec<SymbolId> = self.table(source)?.symbols().flatten().collect();
        for symbol in entries {
            // Similar condition as in `resolveNameHelper`
            if self
                .declaration_of_kind(symbol, K::ExportSpecifier)?
                .is_none()
                && self
                    .declaration_of_kind(symbol, K::NamespaceExport)?
                    .is_none()
                && self.symbol(symbol)?.name_bytes() != names::DEFAULT
            {
                self.copy_scope_symbol(symbols, symbol, meaning)?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/utilities.go:symbolsToArray
    pub(crate) fn symbols_to_array(&self, table: SymbolTableId) -> Result<Vec<SymbolId>, Error> {
        Ok(self
            .table(table)?
            .iter()
            .filter(|(name, _)| !is_reserved_member_name(name))
            .filter_map(|(_, symbol)| symbol)
            .collect())
    }

    // port: tsc/internal/checker/services.go:Checker.getExportsOfModuleAsArray
    pub(crate) fn exports_of_module_as_array(
        &mut self,
        module: SymbolId,
    ) -> Result<Vec<SymbolId>, Error> {
        let table = self.module_exports(module)?;
        self.symbols_to_array(table)
    }

    // port: tsc/internal/checker/services.go:Checker.shouldTreatPropertiesOfExternalModuleAsExports
    pub(crate) fn should_treat_properties_of_external_module_as_exports(
        &self,
        ty: TypeId,
    ) -> Result<bool, Error> {
        Ok(self.types.flags(ty)? & tf::PRIMITIVE == 0
            || self.types.object_flags(ty)? & crate::object_flags::CLASS != 0
            // `isArrayOrTupleLikeType` is too expensive to use in this auto-imports hot path.
            || self.is_array_type(ty)?
            || self.is_tuple_type(ty)?)
    }

    // port: tsc/internal/checker/services.go:Checker.ForEachExportAndPropertyOfModule
    /// The pin's callback receives each symbol with its key; this returns the
    /// same pairs in the same order.
    pub(crate) fn exports_and_properties_of_module_with_keys(
        &mut self,
        module: SymbolId,
    ) -> Result<Vec<(SymbolId, JsString)>, Error> {
        let mut result = Vec::new();
        let table = self.module_exports(module)?;
        let entries: Vec<(JsString, Option<SymbolId>)> = self
            .table(table)?
            .iter()
            .map(|(name, symbol)| (JsString::from_bytes(name), symbol))
            .collect();
        for (key, symbol) in entries {
            if let Some(symbol) = symbol.filter(|_| !is_reserved_member_name(key.as_bytes())) {
                result.push((symbol, key));
            }
        }
        let export_equals = self.resolve_external_module_symbol(Some(module), false)?;
        let Some(export_equals) = export_equals.filter(|&symbol| symbol != module) else {
            return Ok(result);
        };
        let ty = self.get_type_of_symbol(export_equals)?;
        if !self.should_treat_properties_of_external_module_as_exports(ty)? {
            return Ok(result);
        }
        // forEachPropertyOfType
        let reduced = self.reduced_apparent_type(ty)?;
        if self.types.flags(reduced)? & tf::STRUCTURED_TYPE == 0 {
            return Ok(result);
        }
        self.resolve_type_members(reduced)?;
        let members = self.types.structured(reduced)?.members;
        if let Some(members) = members {
            let entries: Vec<(JsString, Option<SymbolId>)> = self
                .table(members)?
                .iter()
                .map(|(name, symbol)| (JsString::from_bytes(name), symbol))
                .collect();
            for (name, symbol) in entries {
                if let Some(symbol) = symbol {
                    if self.is_named_member(symbol, name.as_bytes())? {
                        result.push((symbol, name));
                    }
                }
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/services.go:Checker.isValidPropertyAccess
    pub(crate) fn is_valid_property_access(
        &mut self,
        node: NodeId,
        property_name: &[u8],
    ) -> Result<bool, Error> {
        let read = self.node(node)?;
        match read.kind().known() {
            Some(K::PropertyAccessExpression) => {
                let expression = read
                    .expression()
                    .ok_or(Error::MissingLink("property access expression"))?;
                let is_super = self.node(expression)?.kind() == K::SuperKeyword;
                let ty = self.check_expression(expression)?;
                let ty = self.widened_type(ty)?;
                self.is_valid_property_access_with_type(node, is_super, property_name, ty)
            }
            Some(K::QualifiedName) => {
                let left = read
                    .data_source()
                    .as_qualified_name()
                    .and_then(|name| name.left())
                    .ok_or(Error::MissingLink("qualified name left"))?;
                let ty = self.check_expression(left)?;
                let ty = self.widened_type(ty)?;
                self.is_valid_property_access_with_type(node, false, property_name, ty)
            }
            Some(K::ImportType) => {
                let ty = self.get_type_from_type_node(node)?;
                self.is_valid_property_access_with_type(node, false, property_name, ty)
            }
            _ => Err(Error::Unsupported(
                "Unexpected node kind in isValidPropertyAccess",
            )),
        }
    }

    // port: tsc/internal/checker/services.go:Checker.isValidPropertyAccessWithType
    fn is_valid_property_access_with_type(
        &mut self,
        node: NodeId,
        is_super: bool,
        property_name: &[u8],
        ty: TypeId,
    ) -> Result<bool, Error> {
        // Short-circuiting for improved performance.
        if self.types.flags(ty)? & tf::ANY != 0 {
            return Ok(true);
        }
        let Some(property) = self.constituent_property(ty, property_name, false)? else {
            return Ok(false);
        };
        self.is_access_property_accessible(node, is_super, false, ty, property)
    }

    // port: tsc/internal/checker/services.go:Checker.getAugmentedPropertiesOfType
    pub(crate) fn augmented_properties_of_type(
        &mut self,
        ty: TypeId,
    ) -> Result<Vec<SymbolId>, Error> {
        let ty = self.apparent_type(ty)?;
        let mut names = crate::types::Set::default();
        let mut table = tsr_ast::SymbolTable::default();
        for property in self.get_properties_of_type(ty)? {
            let name = self.symbol(property)?.name_to_owned();
            names.insert(name.clone());
            table.insert(name, Some(property));
        }
        let function = if !self.signatures_of_type(ty, false)?.is_empty() {
            self.query.global_types.get("CallableFunction").copied()
        } else if !self.signatures_of_type(ty, true)?.is_empty() {
            self.query.global_types.get("NewableFunction").copied()
        } else {
            None
        };
        if let Some(function) = function {
            for property in self.get_properties_of_type(function)? {
                let name = self.symbol(property)?.name_to_owned();
                if names.insert(name.clone()) {
                    table.insert(name, Some(property));
                }
            }
        }
        let table = self.alloc_symbol_table(table);
        Ok(self
            .get_named_members(Some(table), None)?
            .map(|members| members.to_vec())
            .unwrap_or_default())
    }

    // port: tsc/internal/checker/services.go:Checker.GetAllPossiblePropertiesOfTypes
    pub(crate) fn all_possible_properties_of_types(
        &mut self,
        types: &[TypeId],
    ) -> Result<Vec<SymbolId>, Error> {
        let union = self.get_union_type(types)?;
        if self.types.flags(union)? & tf::UNION == 0 {
            return self.augmented_properties_of_type(union);
        }
        let mut names = crate::types::Set::default();
        let mut result = Vec::new();
        for &member in types {
            for property in self.augmented_properties_of_type(member)? {
                let name = self.symbol(property)?.name_to_owned();
                if names.contains(&name) {
                    continue;
                }
                // May be undefined if the property is private
                if let Some(property) = self.create_compound_property(union, &name, false)? {
                    names.insert(name);
                    result.push(property);
                }
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/services.go:Checker.TryGetMemberInModuleExports
    pub(crate) fn try_get_member_in_module_exports(
        &mut self,
        member_name: &[u8],
        module: SymbolId,
    ) -> Result<Option<SymbolId>, Error> {
        let table = self.module_exports(module)?;
        Ok(self.table(table)?.get(member_name).flatten())
    }

    // port: tsc/internal/checker/services.go:Checker.TryGetMemberInModuleExportsAndProperties
    pub(crate) fn try_get_member_in_module_exports_and_properties(
        &mut self,
        member_name: &[u8],
        module: SymbolId,
    ) -> Result<Option<SymbolId>, Error> {
        if let Some(symbol) = self.try_get_member_in_module_exports(member_name, module)? {
            return Ok(Some(symbol));
        }
        let export_equals = self.resolve_external_module_symbol(Some(module), false)?;
        let Some(export_equals) = export_equals.filter(|&symbol| symbol != module) else {
            return Ok(None);
        };
        let ty = self.get_type_of_symbol(export_equals)?;
        if self.should_treat_properties_of_external_module_as_exports(ty)? {
            return self.constituent_property(ty, member_name, false);
        }
        Ok(None)
    }

    // port: tsc/internal/checker/services.go:Checker.GetExportsAndPropertiesOfModule
    pub(crate) fn exports_and_properties_of_module(
        &mut self,
        module: SymbolId,
    ) -> Result<Vec<SymbolId>, Error> {
        let mut exports = self.exports_of_module_as_array(module)?;
        let export_equals = self.resolve_external_module_symbol(Some(module), false)?;
        if let Some(export_equals) = export_equals.filter(|&symbol| symbol != module) {
            let ty = self.get_type_of_symbol(export_equals)?;
            if self.should_treat_properties_of_external_module_as_exports(ty)? {
                exports.extend(self.get_properties_of_type(ty)?);
            }
        }
        Ok(exports)
    }

    // port: tsc/internal/checker/services.go:runWithInferenceBlockedFromSourceNode
    fn run_with_inference_blocked_from_source_node<T>(
        &mut self,
        node: NodeId,
        run: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut containing_call = None;
        let mut current = Some(node);
        while let Some(id) = current {
            let read = self.node(id)?;
            if tsr_ast::utilities_middle::is_call_like_expression(self.ast(id)?, &read)? {
                containing_call = Some(id);
                break;
            }
            current = read.parent();
        }
        if let Some(containing_call) = containing_call {
            let mut to_mark_skip = Some(node);
            while let Some(id) = to_mark_skip {
                self.calls.skip_direct_inference_nodes.insert(id);
                to_mark_skip = self.node(id)?.parent();
                if to_mark_skip.is_none() || to_mark_skip == Some(containing_call) {
                    break;
                }
            }
        }
        self.calls.inference_partially_blocked = true;
        let result = self.run_without_resolved_signature_caching(node, run);
        self.calls.inference_partially_blocked = false;
        self.calls.skip_direct_inference_nodes.clear();
        result
    }

    // port: tsc/internal/checker/services.go:runWithoutResolvedSignatureCaching
    fn run_without_resolved_signature_caching<T>(
        &mut self,
        node: NodeId,
        run: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut ancestor = self.call_like_or_function_like_ancestor(Some(node))?;
        if ancestor.is_none() {
            return run(self);
        }
        let mut cached_signatures: Vec<(NodeId, Option<Resolution>)> = Vec::new();
        let mut cached_types: Vec<(crate::links::ValueSymbolKey, Option<TypeId>)> = Vec::new();
        while let Some(id) = ancestor {
            cached_signatures.push((id, self.calls.resolved.remove(&id)));
            if matches!(
                self.node(id)?.kind().known(),
                Some(K::FunctionExpression | K::ArrowFunction)
            ) {
                if let Some(symbol) = self.get_symbol_of_declaration(id)? {
                    let key = self.value_symbol_key(symbol)?;
                    let links = self.value_symbol_links.get_or_default(key);
                    cached_types.push((key, links.resolved_type.take()));
                }
            }
            let parent = self.node(id)?.parent();
            ancestor = self.call_like_or_function_like_ancestor(parent)?;
        }
        let result = run(self);
        for (id, resolution) in cached_signatures {
            match resolution {
                Some(resolution) => {
                    self.calls.resolved.insert(id, resolution);
                }
                None => {
                    self.calls.resolved.remove(&id);
                }
            }
        }
        for (key, ty) in cached_types {
            self.value_symbol_links.get_or_default(key).resolved_type = ty;
        }
        result
    }

    fn call_like_or_function_like_ancestor(
        &self,
        mut node: Option<NodeId>,
    ) -> Result<Option<NodeId>, Error> {
        while let Some(id) = node {
            let read = self.node(id)?;
            if tsr_ast::utilities_middle::is_call_like_or_function_like_expression(
                self.ast(id)?,
                &read,
            )? {
                return Ok(Some(id));
            }
            node = read.parent();
        }
        Ok(None)
    }

    // port: tsc/internal/checker/inference.go:Checker.isFromInferenceBlockedSource
    // port: tsc/internal/checker/inference.go:Checker.isSkipDirectInferenceNode
    pub(crate) fn is_from_inference_blocked_source(&self, ty: TypeId) -> Result<bool, Error> {
        if self.calls.skip_direct_inference_nodes.is_empty() {
            return Ok(false);
        }
        let Some(symbol) = self.types.get(ty)?.symbol else {
            return Ok(false);
        };
        Ok(self
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .any(|declaration| {
                self.calls
                    .skip_direct_inference_nodes
                    .contains(&declaration)
            }))
    }

    // port: tsc/internal/checker/services.go:Checker.GetContextualType
    pub(crate) fn services_contextual_type(
        &mut self,
        node: NodeId,
        context_flags: u32,
    ) -> Result<Option<TypeId>, Error> {
        if context_flags & crate::context_flags::IGNORE_NODE_INFERENCES != 0 {
            return self.run_with_inference_blocked_from_source_node(node, |checker| {
                checker.contextual_expression_type_ex(node, context_flags)
            });
        }
        self.contextual_expression_type_ex(node, context_flags)
    }

    // port: tsc/internal/checker/services.go:Checker.getResolvedSignatureWorker
    fn resolved_signature_worker(
        &mut self,
        node: NodeId,
        check_mode: u32,
        argument_count: usize,
    ) -> Result<(Option<SignatureId>, Vec<SignatureId>), Error> {
        // `printer.NewEmitContext().ParseNode(node)`: the original parse tree node.
        let parsed = self.parse_tree_node(node)?;
        let saved_count = self.calls.apparent_argument_count.replace(argument_count);
        let saved_request = self.calls.candidates_request;
        let saved_out = std::mem::take(&mut self.calls.candidates_out);
        let result = match parsed {
            Some(parsed) => {
                self.calls.candidates_request = Some(parsed);
                let saved_mode = std::mem::replace(&mut self.expression_mode, check_mode);
                let result = self.resolved_call_signature_for_request(parsed);
                self.expression_mode = saved_mode;
                result.map(Some)
            }
            None => Ok(None),
        };
        let candidates = std::mem::replace(&mut self.calls.candidates_out, saved_out);
        self.calls.candidates_request = saved_request;
        self.calls.apparent_argument_count = saved_count;
        Ok((result?, candidates))
    }

    /// `getResolvedSignature` with a `candidatesOutArray`: a cached signature
    /// does not answer, because the candidates must be computed again.
    fn resolved_call_signature_for_request(&mut self, node: NodeId) -> Result<SignatureId, Error> {
        // With a candidates array the pin resolves again even over a cached
        // signature, keeping the resolution start (it resets it only for an
        // uncached node); marking the node as resolving does the same here.
        if let Some(Resolution::Signature(signature)) = self.calls.resolved.get(&node) {
            if *signature != self.builtins.resolving_signature {
                let resolving = self.builtins.resolving_signature;
                self.calls
                    .resolved
                    .insert(node, Resolution::Signature(resolving));
            }
        }
        self.resolved_call_signature(node)
    }

    /// `EmitContext.ParseNode`: the node itself for a parse-tree node, else
    /// its original parse-tree node.
    fn parse_tree_node(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        if self.node(node)?.flags() & tsr_ast::node_flags::SYNTHESIZED == 0 {
            return Ok(Some(node));
        }
        Ok(None)
    }

    // port: tsc/internal/checker/services.go:GetResolvedSignatureForSignatureHelp
    pub(crate) fn resolved_signature_for_signature_help(
        &mut self,
        node: NodeId,
        argument_count: usize,
    ) -> Result<(Option<SignatureId>, Vec<SignatureId>), Error> {
        self.run_without_resolved_signature_caching(node, |checker| {
            checker.resolved_signature_worker(
                node,
                // CheckModeIsForSignatureHelp
                16,
                argument_count,
            )
        })
    }

    // port: tsc/internal/checker/services.go:Checker.GetCandidateSignaturesForStringLiteralCompletions
    pub(crate) fn candidate_signatures_for_string_literal_completions(
        &mut self,
        call: NodeId,
        editing_argument: NodeId,
    ) -> Result<Vec<SignatureId>, Error> {
        // first, get candidates when inference is blocked from the source node.
        let mut candidates = self
            .run_with_inference_blocked_from_source_node(editing_argument, |checker| {
                Ok(checker.resolved_signature_worker(call, 0, 0)?.1)
            })?;
        let seen: crate::types::Set<SignatureId> = candidates.iter().copied().collect();
        // next, get candidates where the source node is considered for inference.
        let others = self.run_without_resolved_signature_caching(editing_argument, |checker| {
            Ok(checker.resolved_signature_worker(call, 0, 0)?.1)
        })?;
        for candidate in others {
            if !seen.contains(&candidate) {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    // port: tsc/internal/checker/services.go:Checker.GetRootSymbols
    pub(crate) fn root_symbols(&mut self, symbol: SymbolId) -> Result<Vec<SymbolId>, Error> {
        let roots = self.immediate_root_symbols(symbol)?;
        if roots.is_empty() {
            return Ok(vec![symbol]);
        }
        let mut result = Vec::new();
        for root in roots {
            result.extend(self.root_symbols(root)?);
        }
        Ok(result)
    }

    // port: tsc/internal/checker/services.go:Checker.getImmediateRootSymbols
    fn immediate_root_symbols(&mut self, symbol: SymbolId) -> Result<Vec<SymbolId>, Error> {
        let read = self.symbol(symbol)?;
        let (flags, check_flags) = (read.flags(), read.check_flags());
        if check_flags & cf::SYNTHETIC != 0 {
            let containing = self
                .value_symbol_links
                .try_get(self.value_symbol_key(symbol)?)
                .and_then(|links| links.containing_type)
                .ok_or(Error::MissingLink("synthetic property containing type"))?;
            let name = self.symbol(symbol)?.name_to_owned();
            let mut result = Vec::new();
            for part in self
                .types
                .compound_types(containing)?
                .clone()
                .iter()
                .copied()
            {
                if let Some(property) = self.constituent_property(part, name.as_bytes(), false)? {
                    result.push(property);
                }
            }
            return Ok(result);
        }
        if flags & sf::TRANSIENT != 0 {
            if let Some(&(left, right)) = self.bindings.spread_links.get(&symbol) {
                return Ok(vec![left, right]);
            }
            if let Some(origin) = self
                .mapped_symbol_links
                .try_get(symbol)
                .and_then(|links| links.synthetic_origin)
            {
                return Ok(vec![origin]);
            }
            if let Some(target) = self.try_get_target(symbol)? {
                return Ok(vec![target]);
            }
        }
        Ok(Vec::new())
    }

    // port: tsc/internal/checker/services.go:Checker.tryGetTarget
    fn try_get_target(&self, symbol: SymbolId) -> Result<Option<SymbolId>, Error> {
        let mut target = None;
        let mut next = symbol;
        loop {
            let following = match self
                .value_symbol_links
                .try_get(self.value_symbol_key(next)?)
            {
                Some(links) => links.target,
                None => self
                    .module_aliases
                    .export_types
                    .get(&next)
                    .map(|&(target, _)| target),
            };
            let Some(following) = following else {
                break;
            };
            target = Some(following);
            next = following;
        }
        Ok(target)
    }

    // port: tsc/internal/checker/services.go:Checker.GetMappedTypeSymbolOfProperty
    pub(crate) fn mapped_type_symbol_of_property(
        &self,
        symbol: SymbolId,
    ) -> Result<Option<SymbolId>, Error> {
        let Some(links) = self
            .value_symbol_links
            .try_get(self.value_symbol_key(symbol)?)
        else {
            return Ok(None);
        };
        match links.containing_type {
            Some(containing) => Ok(self.types.get(containing)?.symbol),
            None => Ok(None),
        }
    }

    // port: tsc/internal/checker/services.go:Checker.GetExportSymbolOfSymbol
    pub(crate) fn export_symbol_of_symbol(&self, symbol: SymbolId) -> Result<SymbolId, Error> {
        let export = self.symbol(symbol)?.export_symbol().unwrap_or(symbol);
        Ok(self.get_merged_symbol(export))
    }

    // port: tsc/internal/checker/services.go:Checker.GetExportSpecifierLocalTargetSymbol
    pub(crate) fn export_specifier_local_target_symbol(
        &mut self,
        node: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        const MEANING: SymbolFlags = sf::VALUE | sf::TYPE | sf::NAMESPACE | sf::ALIAS;
        let read = self.node(node)?;
        match read.kind().known() {
            Some(K::ExportSpecifier) => {
                let declaration = self
                    .node(read.parent().ok_or(Error::MissingLink("named exports"))?)?
                    .parent()
                    .ok_or(Error::MissingLink("export declaration"))?;
                if let Some(specifier) = self.node(declaration)?.module_specifier() {
                    return self.export_declaration_member(declaration, node, specifier);
                }
                let name = read
                    .property_name()
                    .or(read.name())
                    .ok_or(Error::MissingLink("export specifier name"))?;
                if self.node(name)?.kind() == K::StringLiteral {
                    // Skip for invalid syntax like this: export { "x" }
                    return Ok(None);
                }
                self.resolve_entity_name(name, MEANING, true)
            }
            Some(K::Identifier) => self.resolve_entity_name(node, MEANING, true),
            _ => Err(Error::Unsupported(
                "Unhandled case in getExportSpecifierLocalTargetSymbol",
            )),
        }
    }

    /// `getExternalModuleMember(declaration, specifier, false)` for an export
    /// specifier of a re-exporting declaration.
    fn export_declaration_member(
        &mut self,
        declaration: NodeId,
        node: NodeId,
        module_specifier: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        self.prepare_module_attributes(module_specifier)?;
        let Some(module) =
            self.resolve_external_module_name(declaration, module_specifier, false)?
        else {
            return Ok(None);
        };
        self.external_module_member(module, node, module_specifier, false)
    }

    // port: tsc/internal/checker/services.go:Checker.GetShorthandAssignmentValueSymbol
    pub(crate) fn shorthand_assignment_value_symbol(
        &mut self,
        location: Option<NodeId>,
    ) -> Result<Option<SymbolId>, Error> {
        let Some(location) = location else {
            return Ok(None);
        };
        let read = self.node(location)?;
        if read.kind() != K::ShorthandPropertyAssignment {
            return Ok(None);
        }
        let name = read.name().ok_or(Error::MissingLink("shorthand name"))?;
        self.resolve_entity_name(name, sf::VALUE | sf::ALIAS, true)
    }

    // port: tsc/internal/checker/services.go:Checker.GetSymbolsOfParameterPropertyDeclaration
    pub(crate) fn symbols_of_parameter_property_declaration(
        &mut self,
        parameter: NodeId,
        parameter_name: &[u8],
    ) -> Result<(SymbolId, SymbolId), Error> {
        let constructor = self
            .node(parameter)?
            .parent()
            .ok_or(Error::MissingLink("parameter property constructor"))?;
        let class = self
            .node(constructor)?
            .parent()
            .ok_or(Error::MissingLink("parameter property class"))?;
        let locals = self
            .program()?
            .bound(constructor)?
            .node_binding(constructor)?
            .and_then(|binding| binding.locals);
        let parameter_symbol = self.lookup_symbol_resolving(locals, parameter_name, sf::VALUE)?;
        let class_symbol = self
            .node_symbol(class)?
            .ok_or(Error::MissingLink("parameter property class symbol"))?;
        let members = self.members_of_symbol(class_symbol)?;
        let property_symbol = self.lookup_symbol_resolving(members, parameter_name, sf::VALUE)?;
        match (parameter_symbol, property_symbol) {
            (Some(parameter), Some(property)) => Ok((parameter, property)),
            _ => Err(Error::Unsupported(
                "There should exist two symbols, one as property declaration and one as parameter declaration",
            )),
        }
    }

    // port: tsc/internal/checker/services.go:Checker.IsDeclarationUsed
    pub(crate) fn is_declaration_used(
        &mut self,
        source: NodeId,
        identifier: NodeId,
        jsx_elements_present: bool,
        jsx_mode_needs_explicit_import: bool,
    ) -> Result<bool, Error> {
        if jsx_elements_present && jsx_mode_needs_explicit_import {
            let namespace = self.jsx_namespace(Some(source))?;
            let fragment_factory = self.jsx_fragment_factory_name(source)?;
            let text = self.node_text(identifier)?.into_js_string();
            if text == namespace {
                return Ok(true);
            }
            if !fragment_factory.is_empty() && text == fragment_factory {
                return Ok(true);
            }
        }
        let Some(symbol) = self.get_symbol_at_location(identifier)? else {
            return Ok(true);
        };
        self.is_symbol_referenced_in_file(source, identifier, symbol)
    }

    /// `GetJsxFragmentFactory`: the leftmost name of the fragment factory entity.
    fn jsx_fragment_factory_name(&mut self, location: NodeId) -> Result<JsString, Error> {
        let Some(entity) = self.jsx_fragment_factory_entity(Some(location))? else {
            return Ok(JsString::default());
        };
        let first = tsr_ast::utilities_middle::get_first_identifier(self.ast(entity)?, entity)?;
        Ok(self.node_text(first)?.into_js_string())
    }

    // port: tsc/internal/checker/services.go:Checker.IsSymbolReferencedInFile
    pub(crate) fn is_symbol_referenced_in_file(
        &mut self,
        source: NodeId,
        definition: NodeId,
        symbol: SymbolId,
    ) -> Result<bool, Error> {
        let text = self.node_text(definition)?.into_js_string();
        for token in self.possible_symbol_reference_nodes(source, text.as_bytes())? {
            if self.node(token)?.kind() != K::Identifier
                || token == definition
                || self.node_text(token)?.as_bytes() != text.as_bytes()
            {
                continue;
            }
            if self.symbol_referenced_by(token, symbol)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/services.go:Checker.GetReferencesToSymbolInFile
    pub(crate) fn references_to_symbol_in_file(
        &mut self,
        source: NodeId,
        symbol: SymbolId,
    ) -> Result<Vec<NodeId>, Error> {
        let name = self.symbol(symbol)?.name_to_owned();
        let mut result = Vec::new();
        for token in self.possible_symbol_reference_nodes(source, name.as_bytes())? {
            if self.node(token)?.kind() != K::Identifier
                || self.node_text(token)?.as_bytes() != name.as_bytes()
            {
                continue;
            }
            if self.symbol_referenced_by(token, symbol)? {
                result.push(token);
            }
        }
        Ok(result)
    }

    /// The shared test of `IsSymbolReferencedInFile` and
    /// `GetReferencesToSymbolInFile`: the identifier's symbol, its shorthand
    /// value symbol or its export specifier's local target is `symbol`.
    fn symbol_referenced_by(&mut self, token: NodeId, symbol: SymbolId) -> Result<bool, Error> {
        let reference = self.get_symbol_at_location(token)?;
        if reference == Some(symbol) {
            return Ok(true);
        }
        let parent = self.node(token)?.parent();
        if let Some(parent) = parent {
            let kind = self.node(parent)?.kind();
            if kind == K::ShorthandPropertyAssignment
                && self.shorthand_assignment_value_symbol(Some(parent))? == Some(symbol)
            {
                return Ok(true);
            }
            if kind == K::ExportSpecifier
                && self.local_symbol_for_export_specifier(token, reference, parent)? == Some(symbol)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/services.go:Checker.getLocalSymbolForExportSpecifier
    fn local_symbol_for_export_specifier(
        &mut self,
        reference_location: NodeId,
        reference_symbol: Option<SymbolId>,
        export_specifier: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        if self.is_export_specifier_alias(reference_location, export_specifier)? {
            if let Some(symbol) = self.export_specifier_local_target_symbol(export_specifier)? {
                return Ok(Some(symbol));
            }
        }
        Ok(reference_symbol)
    }

    // port: tsc/internal/checker/services.go:isExportSpecifierAlias
    fn is_export_specifier_alias(
        &self,
        reference_location: NodeId,
        export_specifier: NodeId,
    ) -> Result<bool, Error> {
        let read = self.node(export_specifier)?;
        if let Some(property_name) = read.property_name() {
            // Given `export { foo as bar } [from "someModule"]`: It's an alias at `foo`, but at `bar` it's a new symbol.
            return Ok(property_name == reference_location);
        }
        // `export { foo } from "foo"` is a re-export.
        // `export { foo };` is not a re-export, it creates an alias for the local variable `foo`.
        let declaration = self
            .node(read.parent().ok_or(Error::MissingLink("named exports"))?)?
            .parent()
            .ok_or(Error::MissingLink("export declaration"))?;
        Ok(self.node(declaration)?.module_specifier().is_none())
    }

    // port: tsc/internal/checker/services.go:getPossibleSymbolReferenceNodes
    fn possible_symbol_reference_nodes(
        &self,
        source: NodeId,
        symbol_name: &[u8],
    ) -> Result<Vec<NodeId>, Error> {
        let positions = self.possible_symbol_reference_positions(source, symbol_name)?;
        let view = self.ast(source)?;
        let host = &*self.program()?.host;
        let mut jsdoc = HostJsDoc(host);
        let mut navigator = tsr_astnav::Navigator::new(view, source, &mut jsdoc);
        let mut result = Vec::new();
        for position in positions {
            let location = navigator
                .get_touching_property_name(position)
                .map_err(|_| Error::MissingLink("touching property name"))?;
            if location != source {
                result.push(location);
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/services.go:getPossibleSymbolReferencePositions
    fn possible_symbol_reference_positions(
        &self,
        source: NodeId,
        symbol_name: &[u8],
    ) -> Result<Vec<i64>, Error> {
        let mut positions = Vec::new();
        // Be resilient in the face of a symbol with no name or zero length name
        if symbol_name.is_empty() {
            return Ok(positions);
        }
        let view = self.ast(source)?;
        let file = view.source_file(source)?;
        let text = file.text().as_bytes();
        let container = self.node(source)?;
        let (start, end) = (
            usize::try_from(container.pos()).unwrap_or(0),
            usize::try_from(container.end()).unwrap_or(0),
        );
        let find = |from: usize| -> Option<usize> {
            text.get(from..)?
                .windows(symbol_name.len())
                .position(|window| window == symbol_name)
                .map(|offset| from + offset)
        };
        let mut position = find(start);
        while let Some(found) = position {
            if found >= end {
                break;
            }
            // We found a match.  Make sure it's not part of a larger word (i.e. the char
            // before and after it have to be a non-identifier char).
            let end_position = found + symbol_name.len();
            if (found == 0 || !tsr_scanner::is_identifier_part(i32::from(text[found - 1])))
                && (end_position == text.len()
                    || !tsr_scanner::is_identifier_part(i32::from(text[end_position])))
            {
                // Found a real match.  Keep searching.
                positions.push(i64::try_from(found).unwrap_or(i64::MAX));
            }
            let start_index = found + symbol_name.len() + 1;
            if start_index > text.len() {
                break;
            }
            position = find(start_index);
        }
        Ok(positions)
    }

    // port: tsc/internal/checker/services.go:Checker.getUninstantiatedSignatures
    fn uninstantiated_signatures(&mut self, node: NodeId) -> Result<Vec<SignatureId>, Error> {
        let read = self.node(node)?;
        let (target, construct) = match read.kind().known() {
            Some(K::CallExpression | K::Decorator) => (read.expression(), false),
            Some(K::NewExpression) => (read.expression(), true),
            Some(K::JsxSelfClosingElement | K::JsxOpeningElement) => {
                let tag = read.tag_name().ok_or(Error::MissingLink("JSX tag name"))?;
                if self.is_jsx_intrinsic_tag_name(tag)? {
                    return Ok(Vec::new());
                }
                (Some(tag), false)
            }
            Some(K::TaggedTemplateExpression) => (
                read.data_source()
                    .as_tagged_template_expression()
                    .and_then(|data| data.tag()),
                false,
            ),
            _ => return Ok(Vec::new()),
        };
        let target = target.ok_or(Error::MissingLink("call target"))?;
        let ty = self.get_type_of_expression(target)?;
        self.signatures_of_type(ty, construct)
    }

    // port: tsc/internal/checker/services.go:Checker.getTypeParameterConstraintForPositionAcrossSignatures
    fn type_parameter_constraint_for_position_across_signatures(
        &mut self,
        signatures: &[SignatureId],
        position: usize,
    ) -> Result<TypeId, Error> {
        let mut constraints = Vec::new();
        for &signature in signatures {
            let parameters = self.signatures.get(signature)?.type_parameters.clone();
            let Some(&parameter) = parameters.as_deref().and_then(|p| p.get(position)) else {
                continue;
            };
            if let Some(constraint) = self.constraint_of_type_parameter(parameter)? {
                constraints.push(constraint);
            }
        }
        self.get_union_type(&constraints)
    }

    // port: tsc/internal/checker/services.go:Checker.GetTypeArgumentConstraint
    // port: tsc/internal/checker/services.go:Checker.getTypeArgumentConstraint
    pub(crate) fn type_argument_constraint(
        &mut self,
        node: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        if !tsr_ast::utilities::is_type_node(&self.node(node)?) {
            return Ok(None);
        }
        let Some(parent) = self.node(node)?.parent() else {
            return Ok(None);
        };
        let parent_read = self.node(parent)?;
        let mut position = None;
        if tsr_ast::utilities_middle::has_type_arguments(&parent_read) {
            let arguments = self.source_list(parent, parent_read.type_argument_list())?;
            position = arguments.iter().position(|&argument| argument == node);
        }
        let Some(position) = position else {
            return Ok(None);
        };
        // The node could be a type argument of a call, a `new` expression, a decorator, an
        // instantiation expression, or a generic type instantiation.
        if tsr_ast::utilities_middle::is_call_like_expression(self.ast(parent)?, &parent_read)? {
            let signatures = self.uninstantiated_signatures(parent)?;
            return self
                .type_parameter_constraint_for_position_across_signatures(&signatures, position)
                .map(Some);
        }
        let grandparent = parent_read.parent();
        if let Some(grandparent) = grandparent {
            if self.node(grandparent)?.kind() == K::Decorator {
                let signatures = self.uninstantiated_signatures(grandparent)?;
                return self
                    .type_parameter_constraint_for_position_across_signatures(&signatures, position)
                    .map(Some);
            }
            if parent_read.kind() == K::ExpressionWithTypeArguments
                && self.node(grandparent)?.kind() == K::ExpressionStatement
            {
                let expression = parent_read
                    .expression()
                    .ok_or(Error::MissingLink("instantiation expression"))?;
                let uninstantiated = self.check_expression(expression)?;
                let calls = self.signatures_of_type(uninstantiated, false)?;
                let call = self
                    .type_parameter_constraint_for_position_across_signatures(&calls, position)?;
                let constructs = self.signatures_of_type(uninstantiated, true)?;
                let construct = self.type_parameter_constraint_for_position_across_signatures(
                    &constructs,
                    position,
                )?;
                // An instantiation expression instantiates both call and construct signatures, so
                // if both exist type arguments must be assignable to both constraints.
                if self.types.flags(construct)? & tf::NEVER != 0 {
                    return Ok(Some(call));
                }
                if self.types.flags(call)? & tf::NEVER != 0 {
                    return Ok(Some(construct));
                }
                return self.get_intersection_type(&[call, construct]).map(Some);
            }
        }
        if matches!(
            parent_read.kind().known(),
            Some(K::TypeReference | K::ImportType | K::ExpressionWithTypeArguments)
        ) || tsr_ast::utilities_middle::is_type_reference_type(&parent_read)
        {
            let parameters = self.type_parameters_for_type_reference_or_import(parent)?;
            let Some(&parameter) = parameters.get(position) else {
                return Ok(None);
            };
            if let Some(constraint) = self.constraint_of_type_parameter(parameter)? {
                let arguments = self.effective_type_arguments(parent, &parameters)?;
                let mapper = self.new_type_mapper(&parameters, &arguments)?;
                return self.instantiate_type(constraint, Some(mapper)).map(Some);
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeParametersForTypeReferenceOrImport
    pub(crate) fn type_parameters_for_type_reference_or_import(
        &mut self,
        node: NodeId,
    ) -> Result<Vec<TypeId>, Error> {
        let ty = self.get_type_from_type_node(node)?;
        if !self.is_error_type(ty)? {
            if let Some(symbol) = self.query.resolved_symbols.try_get(node).copied().flatten() {
                return Ok(self
                    .type_parameters_for_type_and_symbol(ty, symbol)?
                    .to_vec());
            }
        }
        Ok(Vec::new())
    }

    // port: tsc/internal/checker/services.go:Checker.IsTypeInvalidDueToUnionDiscriminant
    pub(crate) fn is_type_invalid_due_to_union_discriminant(
        &mut self,
        contextual_type: TypeId,
        object: NodeId,
    ) -> Result<bool, Error> {
        let list = self.node(object)?.property_list();
        for property in self.source_list(object, list)? {
            let name_type = match self.node(property)?.name() {
                Some(name) if self.node(name)?.kind() == K::JsxNamespacedName => {
                    let text = self.jsx_name_text(name)?;
                    Some(self.get_string_literal_type(text)?)
                }
                Some(name) => Some(self.literal_type_from_property_name(name)?),
                None => None,
            };
            let name = match name_type {
                Some(name_type)
                    if self.types.flags(name_type)? & tf::STRING_OR_NUMBER_LITERAL_OR_UNIQUE
                        != 0 =>
                {
                    self.index_property_name(name_type)?
                }
                _ => None,
            };
            let expected = match name.filter(|name| !name.is_empty()) {
                Some(name) => self.property_type(contextual_type, name.as_bytes())?,
                None => None,
            };
            if let Some(expected) = expected {
                if self.is_literal_type(expected)? {
                    let actual = self.get_type_at_location(property)?;
                    if !self.is_type_related_to(
                        actual,
                        expected,
                        crate::RelationKind::Assignable,
                    )? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/services.go:Checker.GetJsxIntrinsicTagNamesAt
    pub(crate) fn jsx_intrinsic_tag_names_at(
        &mut self,
        location: NodeId,
    ) -> Result<Vec<SymbolId>, Error> {
        let intrinsics = self.jsx_type(b"IntrinsicElements", location)?;
        self.get_properties_of_type(intrinsics)
    }

    // port: tsc/internal/checker/services.go:Checker.GetTypeParameterAtPosition
    pub(crate) fn type_parameter_at_position(
        &mut self,
        signature: SignatureId,
        position: usize,
    ) -> Result<TypeId, Error> {
        let ty = self
            .parameter_type_at(signature, position)?
            .unwrap_or(self.builtins.any_type);
        if self.types.flags(ty)? & tf::INDEX != 0 {
            let target = self.types.index_type(ty)?.target;
            if self.types.flags(target)? & tf::TYPE_PARAMETER != 0
                && self.types.type_parameter(target)?.is_this_type
            {
                if let Some(constraint) = self.base_constraint_of_type(target)? {
                    return self.get_index_type(constraint, 0);
                }
            }
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/services.go:Checker.GetContextualTypeForArrayLiteralAtPosition
    pub(crate) fn contextual_type_for_array_literal_at_position(
        &mut self,
        contextual_array_type: Option<TypeId>,
        array_literal: NodeId,
        position: i64,
    ) -> Result<Option<TypeId>, Error> {
        let Some(contextual_array_type) = contextual_array_type else {
            return Ok(None);
        };
        let (mut first_spread, mut last_spread) = (None, None);
        let mut element_index = 0;
        let elements = self.source_list(array_literal, self.node(array_literal)?.element_list())?;
        for (index, &element) in elements.iter().enumerate() {
            let read = self.node(element)?;
            if i64::from(read.pos()) < position {
                element_index += 1;
            }
            if read.kind() == K::SpreadElement {
                if first_spread.is_none() {
                    first_spread = Some(index);
                }
                last_spread = Some(index);
            }
        }
        // The array may be incomplete, so we don't know its final length.
        self.contextual_element_type(
            contextual_array_type,
            element_index,
            None,
            first_spread,
            last_spread,
        )
    }

    // port: tsc/internal/checker/services.go:Checker.GetFirstTypeArgumentFromKnownType
    pub(crate) fn first_type_argument_from_known_type(
        &mut self,
        ty: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        let record = self.types.get(ty)?;
        let (object_flags, symbol, alias) = (record.object_flags, record.symbol, record.alias);
        if object_flags & crate::object_flags::REFERENCE != 0 {
            if let Some(symbol) = symbol {
                let name = self.symbol(symbol)?.name_to_owned();
                if is_known_generic_type_name(name.as_bytes()) {
                    let global = self.resolve_name(None, name.as_bytes(), sf::TYPE, None, false)?;
                    let target = self.types.target(ty)?;
                    if global.is_some() && global == self.types.get(target)?.symbol {
                        return Ok(self.get_type_arguments(ty)?.first().copied());
                    }
                }
            }
        }
        if let Some(alias) = alias {
            let alias = self.types.alias(alias)?.clone();
            let name = self.symbol(alias.symbol)?.name_to_owned();
            if is_known_generic_type_name(name.as_bytes()) {
                let global = self.resolve_name(None, name.as_bytes(), sf::TYPE, None, false)?;
                if global == Some(alias.symbol) {
                    return Ok(alias.type_arguments.first().copied());
                }
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/services.go:Checker.GetPropertySymbolsFromContextualType
    pub(crate) fn property_symbols_from_contextual_type(
        &mut self,
        node: NodeId,
        contextual_type: TypeId,
        union_symbol_ok: bool,
    ) -> Result<Vec<SymbolId>, Error> {
        let name = match self.node(node)?.name() {
            Some(name) => {
                tsr_ast::utilities_targets::try_get_text_of_property_name(self.ast(name)?, name)?
                    .map(|text| JsString::from_bytes(text.as_slice()))
            }
            None => None,
        };
        let Some(name) = name.filter(|name| !name.is_empty()) else {
            return Ok(Vec::new());
        };
        if self.types.flags(contextual_type)? & tf::UNION == 0 {
            return Ok(self
                .constituent_property(contextual_type, name.as_bytes(), false)?
                .into_iter()
                .collect());
        }
        let members = self.types.compound_types(contextual_type)?.to_vec();
        let parent = self.node(node)?.parent();
        let mut filtered = members.clone();
        if let Some(parent) = parent {
            if matches!(
                self.node(parent)?.kind().known(),
                Some(K::ObjectLiteralExpression | K::JsxAttributes)
            ) {
                let mut kept = Vec::new();
                for member in filtered {
                    if !self.is_type_invalid_due_to_union_discriminant(member, parent)? {
                        kept.push(member);
                    }
                }
                filtered = kept;
            }
        }
        let mut discriminated = Vec::new();
        for &member in &filtered {
            if let Some(property) = self.constituent_property(member, name.as_bytes(), false)? {
                discriminated.push(property);
            }
        }
        if union_symbol_ok && (discriminated.is_empty() || discriminated.len() == members.len()) {
            if let Some(symbol) =
                self.constituent_property(contextual_type, name.as_bytes(), false)?
            {
                return Ok(vec![symbol]);
            }
        }
        if filtered.is_empty() && discriminated.is_empty() {
            // Bad discriminant -- do again without discriminating
            let mut result = Vec::new();
            for member in members {
                if let Some(property) = self.constituent_property(member, name.as_bytes(), false)? {
                    result.push(property);
                }
            }
            return Ok(result);
        }
        // by eliminating duplicates we might even end up with a single symbol
        // that helps with displaying better quick infos on properties of union types
        let mut seen = crate::types::Set::default();
        discriminated.retain(|symbol| seen.insert(*symbol));
        Ok(discriminated)
    }

    // port: tsc/internal/checker/services.go:Checker.GetPropertySymbolOfDestructuringAssignment
    pub(crate) fn property_symbol_of_destructuring_assignment(
        &mut self,
        location: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        let parent = self
            .node(location)?
            .parent()
            .ok_or(Error::MissingLink("destructuring property"))?;
        let pattern = self
            .node(parent)?
            .parent()
            .ok_or(Error::MissingLink("destructuring pattern"))?;
        if tsr_ast::utilities_positions::is_array_literal_or_object_literal_destructuring_pattern(
            self.ast(pattern)?,
            pattern,
        )? {
            // Get the type of the object or array literal and then look for property of given name in the type
            if let Some(ty) = self.type_of_assignment_pattern(pattern)? {
                let text = self.node_text(location)?.into_js_string();
                return self.constituent_property(ty, text.as_bytes(), false);
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/services.go:Checker.getTypeOfAssignmentPattern
    fn type_of_assignment_pattern(&mut self, expression: NodeId) -> Result<Option<TypeId>, Error> {
        let parent = self
            .node(expression)?
            .parent()
            .ok_or(Error::MissingLink("assignment pattern parent"))?;
        let parent_read = self.node(parent)?;
        // If this is from "for of"
        //     for ( { a } of elems) {
        //     }
        if parent_read.kind() == K::ForOfStatement {
            let iterated = self.check_right_hand_side_of_for_of(parent)?;
            return self
                .check_destructuring_assignment(expression, iterated, 0, false)
                .map(Some);
        }
        // If this is from "for" initializer
        //     for ({a } = elems[0];.....) { }
        if parent_read.kind() == K::BinaryExpression {
            let right = parent_read
                .data_source()
                .as_binary_expression()
                .ok_or(tsr_arena::Error::InvalidGraph)?
                .right()
                .ok_or(Error::MissingLink("assignment right"))?;
            let iterated = self.get_type_of_expression(right)?;
            return self
                .check_destructuring_assignment(expression, iterated, 0, false)
                .map(Some);
        }
        // If this is from nested object binding pattern
        //     for ({ skills: { primary, secondary } } = multiRobot, i = 0; i < 1; i++) {
        if parent_read.kind() == K::PropertyAssignment {
            let object = parent_read
                .parent()
                .ok_or(Error::MissingLink("property assignment object"))?;
            let object_type = self
                .type_of_assignment_pattern(object)?
                .unwrap_or(self.builtins.error_type);
            let list = self.node(object)?.property_list();
            let properties = self.source_list(object, list)?;
            let index = properties
                .iter()
                .position(|&property| property == parent)
                .ok_or(Error::MissingLink("destructuring property index"))?;
            return self.check_object_assignment_property(
                parent,
                object_type,
                &properties,
                index,
                None,
                false,
            );
        }
        // Array literal assignment - array destructuring pattern
        //    [{ property1: p1, property2 }] = elems;
        let array_type = self
            .type_of_assignment_pattern(parent)?
            .unwrap_or(self.builtins.error_type);
        let undefined = self.builtins.undefined_type;
        let element_type = self.check_iterated_type_or_element_type(
            crate::iteration::ALLOW_SYNC | crate::iteration::DESTRUCTURING_FLAG,
            array_type,
            undefined,
            Some(parent),
        )?;
        let list = self.node(parent)?.element_list();
        let elements = self.source_list(parent, list)?;
        let index = elements
            .iter()
            .position(|&element| element == expression)
            .ok_or(Error::MissingLink("destructuring element index"))?;
        self.check_array_assignment_element(
            expression,
            array_type,
            index,
            element_type,
            0,
            &elements,
            list,
        )
    }
}

impl Operation<'_> {
    fn signature_refs(&self, signatures: &[SignatureId]) -> Vec<SignatureRef> {
        signatures
            .iter()
            .map(|&signature| self.signature_ref(signature))
            .collect()
    }

    fn optional_type(&self, ty: Option<TypeId>) -> Option<TypeRef> {
        ty.map(|ty| self.type_ref(ty))
    }

    // port: tsc/internal/checker/services.go:Checker.GetSymbolsInScope
    pub fn get_symbols_in_scope(
        &mut self,
        location: NodeId,
        meaning: SymbolFlags,
    ) -> Result<Vec<SymbolRef>, Error> {
        let symbols = self.state_mut().symbols_in_scope(location, meaning)?;
        self.symbol_refs(&symbols)
    }

    // port: tsc/internal/checker/services.go:Checker.GetExportsOfModule
    pub fn get_exports_of_module(&mut self, module: SymbolRef) -> Result<Vec<SymbolRef>, Error> {
        let module = self.check_symbol_ref(module)?;
        let symbols = self.state_mut().exports_of_module_as_array(module)?;
        self.symbol_refs(&symbols)
    }

    /// `ForEachExportAndPropertyOfModule`: the symbols and keys its callback receives.
    pub fn for_each_export_and_property_of_module(
        &mut self,
        module: SymbolRef,
    ) -> Result<Vec<(SymbolRef, JsString)>, Error> {
        let module = self.check_symbol_ref(module)?;
        let pairs = self
            .state_mut()
            .exports_and_properties_of_module_with_keys(module)?;
        pairs
            .into_iter()
            .map(|(symbol, key)| Ok((self.symbol_ref(symbol)?, key)))
            .collect()
    }

    // port: tsc/internal/checker/services.go:Checker.IsValidPropertyAccess
    pub fn is_valid_property_access(
        &mut self,
        node: NodeId,
        property_name: &[u8],
    ) -> Result<bool, Error> {
        self.state_mut()
            .is_valid_property_access(node, property_name)
    }

    /// `GetAllPossiblePropertiesOfTypes`.
    pub fn get_all_possible_properties_of_types(
        &mut self,
        types: &[TypeRef],
    ) -> Result<Vec<SymbolRef>, Error> {
        let types = types
            .iter()
            .map(|&ty| self.check_type(ty))
            .collect::<Result<Vec<_>, _>>()?;
        let symbols = self.state_mut().all_possible_properties_of_types(&types)?;
        self.symbol_refs(&symbols)
    }

    // port: tsc/internal/checker/services.go:Checker.IsUnknownSymbol
    pub fn is_unknown_symbol(&self, symbol: SymbolRef) -> Result<bool, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        Ok(symbol == self.state().builtins.unknown_symbol)
    }

    // port: tsc/internal/checker/services.go:Checker.IsUndefinedSymbol
    pub fn is_undefined_symbol(&self, symbol: SymbolRef) -> Result<bool, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        Ok(symbol == self.state().builtins.undefined_symbol)
    }

    // port: tsc/internal/checker/services.go:Checker.IsArgumentsSymbol
    pub fn is_arguments_symbol(&self, symbol: SymbolRef) -> Result<bool, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        Ok(symbol == self.state().builtins.arguments_symbol)
    }

    // port: tsc/internal/checker/services.go:Checker.GetNonOptionalType
    pub fn get_non_optional_type(&mut self, ty: TypeRef) -> Result<TypeRef, Error> {
        let ty = self.check_type(ty)?;
        let result = self.state_mut().remove_optional_type_marker(ty)?;
        Ok(self.type_ref(result))
    }

    /// `getIndexTypeOfType`: the value type of the applicable index info.
    fn index_type_of_type(&mut self, ty: TypeRef, key: TypeId) -> Result<Option<TypeRef>, Error> {
        let ty = self.check_type(ty)?;
        let state = self.state_mut();
        let value = match state.index_info_of_type(ty, key)? {
            Some(info) => Some(state.signatures.index_info(info)?.value_type),
            None => None,
        };
        Ok(self.optional_type(value))
    }

    // port: tsc/internal/checker/services.go:Checker.GetStringIndexType
    pub fn get_string_index_type(&mut self, ty: TypeRef) -> Result<Option<TypeRef>, Error> {
        let key = self.state().builtins.string_type;
        self.index_type_of_type(ty, key)
    }

    // port: tsc/internal/checker/services.go:Checker.GetNumberIndexType
    pub fn get_number_index_type(&mut self, ty: TypeRef) -> Result<Option<TypeRef>, Error> {
        let key = self.state().builtins.number_type;
        self.index_type_of_type(ty, key)
    }

    // port: tsc/internal/checker/services.go:Checker.GetElementTypeOfArrayType
    pub fn get_element_type_of_array_type(
        &mut self,
        ty: TypeRef,
    ) -> Result<Option<TypeRef>, Error> {
        let ty = self.check_type(ty)?;
        let state = self.state_mut();
        if !state.is_array_type(ty)? {
            return Ok(None);
        }
        let element = state.get_type_arguments(ty)?.first().copied();
        Ok(self.optional_type(element))
    }

    // port: tsc/internal/checker/services.go:Checker.GetCallSignatures
    pub fn get_call_signatures(&mut self, ty: TypeRef) -> Result<Vec<SignatureRef>, Error> {
        let ty = self.check_type(ty)?;
        let signatures = self.state_mut().signatures_of_type(ty, false)?;
        Ok(self.signature_refs(&signatures))
    }

    // port: tsc/internal/checker/services.go:Checker.GetConstructSignatures
    pub fn get_construct_signatures(&mut self, ty: TypeRef) -> Result<Vec<SignatureRef>, Error> {
        let ty = self.check_type(ty)?;
        let signatures = self.state_mut().signatures_of_type(ty, true)?;
        Ok(self.signature_refs(&signatures))
    }

    // port: tsc/internal/checker/services.go:Checker.GetApparentProperties
    pub fn get_apparent_properties(&mut self, ty: TypeRef) -> Result<Vec<SymbolRef>, Error> {
        let ty = self.check_type(ty)?;
        let symbols = self.state_mut().augmented_properties_of_type(ty)?;
        self.symbol_refs(&symbols)
    }

    /// `TryGetMemberInModuleExportsAndProperties`.
    pub fn try_get_member_in_module_exports_and_properties(
        &mut self,
        member_name: &[u8],
        module: SymbolRef,
    ) -> Result<Option<SymbolRef>, Error> {
        let module = self.check_symbol_ref(module)?;
        let symbol = self
            .state_mut()
            .try_get_member_in_module_exports_and_properties(member_name, module)?;
        self.optional_symbol(symbol)
    }

    /// `TryGetMemberInModuleExports`.
    pub fn try_get_member_in_module_exports(
        &mut self,
        member_name: &[u8],
        module: SymbolRef,
    ) -> Result<Option<SymbolRef>, Error> {
        let module = self.check_symbol_ref(module)?;
        let symbol = self
            .state_mut()
            .try_get_member_in_module_exports(member_name, module)?;
        self.optional_symbol(symbol)
    }

    /// `GetContextualType` with the pin's `ContextFlags` bits.
    pub fn get_contextual_type(
        &mut self,
        node: NodeId,
        context_flags: u32,
    ) -> Result<Option<TypeRef>, Error> {
        let ty = self
            .state_mut()
            .services_contextual_type(node, context_flags)?;
        Ok(self.optional_type(ty))
    }

    /// `GetResolvedSignatureForSignatureHelp`: the signature and the
    /// candidates its resolution chose among.
    pub fn get_resolved_signature_for_signature_help(
        &mut self,
        node: NodeId,
        argument_count: usize,
    ) -> Result<(Option<SignatureRef>, Vec<SignatureRef>), Error> {
        let (signature, candidates) = self
            .state_mut()
            .resolved_signature_for_signature_help(node, argument_count)?;
        Ok((
            signature.map(|signature| self.signature_ref(signature)),
            self.signature_refs(&candidates),
        ))
    }

    /// The binder's symbol attached to a declaration (without name resolution).
    pub fn bound_symbol_of_node(&self, node: NodeId) -> Result<Option<SymbolRef>, Error> {
        self.state()
            .node_symbol(node)?
            .map(|symbol| self.symbol_ref(symbol))
            .transpose()
    }

    /// Non-reporting resolution of the program's implicit JSX/helper imports.
    pub fn implicit_import_symbol(
        &mut self,
        source: NodeId,
        name: &[u8],
    ) -> Result<Option<SymbolRef>, Error> {
        let symbol = self
            .state_mut()
            .implicit_import_symbol(source, JsString::from_bytes(name))?;
        self.optional_symbol(symbol)
    }

    // port: tsc/internal/checker/services.go:Checker.SkipAlias
    pub fn skip_alias(&mut self, symbol: SymbolRef) -> Result<SymbolRef, Error> {
        let id = self.check_symbol_ref(symbol)?;
        let state = self.state_mut();
        if state.symbol(id)?.flags() & sf::ALIAS == 0 {
            return Ok(symbol);
        }
        let target = state.resolve_alias(id)?;
        self.symbol_ref(target)
    }

    /// `GetRootSymbols`.
    pub fn get_root_symbols(&mut self, symbol: SymbolRef) -> Result<Vec<SymbolRef>, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        let roots = self.state_mut().root_symbols(symbol)?;
        self.symbol_refs(&roots)
    }

    /// `GetMappedTypeSymbolOfProperty`.
    pub fn get_mapped_type_symbol_of_property(
        &self,
        symbol: SymbolRef,
    ) -> Result<Option<SymbolRef>, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        let mapped = self.state().mapped_type_symbol_of_property(symbol)?;
        self.optional_symbol(mapped)
    }

    /// `GetExportSymbolOfSymbol`.
    pub fn get_export_symbol_of_symbol(&self, symbol: SymbolRef) -> Result<SymbolRef, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        let export = self.state().export_symbol_of_symbol(symbol)?;
        self.symbol_ref(export)
    }

    /// `GetExportSpecifierLocalTargetSymbol`.
    pub fn get_export_specifier_local_target_symbol(
        &mut self,
        node: NodeId,
    ) -> Result<Option<SymbolRef>, Error> {
        let symbol = self
            .state_mut()
            .export_specifier_local_target_symbol(node)?;
        self.optional_symbol(symbol)
    }

    /// `GetShorthandAssignmentValueSymbol`.
    pub fn get_shorthand_assignment_value_symbol(
        &mut self,
        location: Option<NodeId>,
    ) -> Result<Option<SymbolRef>, Error> {
        let symbol = self
            .state_mut()
            .shorthand_assignment_value_symbol(location)?;
        self.optional_symbol(symbol)
    }

    /// `GetSymbolsOfParameterPropertyDeclaration`: the parameter's symbol and
    /// the property's.
    pub fn get_symbols_of_parameter_property_declaration(
        &mut self,
        parameter: NodeId,
        parameter_name: &[u8],
    ) -> Result<(SymbolRef, SymbolRef), Error> {
        let (parameter, property) = self
            .state_mut()
            .symbols_of_parameter_property_declaration(parameter, parameter_name)?;
        Ok((self.symbol_ref(parameter)?, self.symbol_ref(property)?))
    }

    /// `IsDeclarationUsed`.
    pub fn is_declaration_used(
        &mut self,
        source: NodeId,
        identifier: NodeId,
        jsx_elements_present: bool,
        jsx_mode_needs_explicit_import: bool,
    ) -> Result<bool, Error> {
        self.state_mut().is_declaration_used(
            source,
            identifier,
            jsx_elements_present,
            jsx_mode_needs_explicit_import,
        )
    }

    /// `IsSymbolReferencedInFile`.
    pub fn is_symbol_referenced_in_file(
        &mut self,
        source: NodeId,
        definition: NodeId,
        symbol: SymbolRef,
    ) -> Result<bool, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.state_mut()
            .is_symbol_referenced_in_file(source, definition, symbol)
    }

    /// `GetReferencesToSymbolInFile`.
    pub fn get_references_to_symbol_in_file(
        &mut self,
        source: NodeId,
        symbol: SymbolRef,
    ) -> Result<Vec<NodeId>, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.state_mut()
            .references_to_symbol_in_file(source, symbol)
    }

    /// `GetTypeArgumentConstraint`.
    pub fn get_type_argument_constraint(&mut self, node: NodeId) -> Result<Option<TypeRef>, Error> {
        let ty = self.state_mut().type_argument_constraint(node)?;
        Ok(self.optional_type(ty))
    }

    /// `IsTypeInvalidDueToUnionDiscriminant`.
    pub fn is_type_invalid_due_to_union_discriminant(
        &mut self,
        contextual_type: TypeRef,
        object: NodeId,
    ) -> Result<bool, Error> {
        let contextual_type = self.check_type(contextual_type)?;
        self.state_mut()
            .is_type_invalid_due_to_union_discriminant(contextual_type, object)
    }

    /// `GetExportsAndPropertiesOfModule`.
    pub fn get_exports_and_properties_of_module(
        &mut self,
        module: SymbolRef,
    ) -> Result<Vec<SymbolRef>, Error> {
        let module = self.check_symbol_ref(module)?;
        let symbols = self.state_mut().exports_and_properties_of_module(module)?;
        self.symbol_refs(&symbols)
    }

    /// `GetJsxIntrinsicTagNamesAt`.
    pub fn get_jsx_intrinsic_tag_names_at(
        &mut self,
        location: NodeId,
    ) -> Result<Vec<SymbolRef>, Error> {
        let symbols = self.state_mut().jsx_intrinsic_tag_names_at(location)?;
        self.symbol_refs(&symbols)
    }

    // port: tsc/internal/checker/services.go:Checker.GetContextualTypeForJsxAttribute
    pub fn get_contextual_type_for_jsx_attribute(
        &mut self,
        attribute: NodeId,
    ) -> Result<Option<TypeRef>, Error> {
        let ty = self
            .state_mut()
            .contextual_type_for_jsx_attribute(attribute, 0)?;
        Ok(self.optional_type(ty))
    }

    /// `GetCandidateSignaturesForStringLiteralCompletions`.
    pub fn get_candidate_signatures_for_string_literal_completions(
        &mut self,
        call: NodeId,
        editing_argument: NodeId,
    ) -> Result<Vec<SignatureRef>, Error> {
        let candidates = self
            .state_mut()
            .candidate_signatures_for_string_literal_completions(call, editing_argument)?;
        Ok(self.signature_refs(&candidates))
    }

    // port: tsc/internal/checker/services.go:Checker.GetTypeAtPosition
    pub fn get_type_at_position(
        &mut self,
        signature: SignatureRef,
        position: usize,
    ) -> Result<TypeRef, Error> {
        let signature = self.check_signature(signature)?;
        let state = self.state_mut();
        let ty = state
            .parameter_type_at(signature, position)?
            .unwrap_or(state.builtins.any_type);
        Ok(self.type_ref(ty))
    }

    /// `GetTypeParameterAtPosition`.
    pub fn get_type_parameter_at_position(
        &mut self,
        signature: SignatureRef,
        position: usize,
    ) -> Result<TypeRef, Error> {
        let signature = self.check_signature(signature)?;
        let ty = self
            .state_mut()
            .type_parameter_at_position(signature, position)?;
        Ok(self.type_ref(ty))
    }

    /// `GetContextualTypeForArrayLiteralAtPosition`.
    pub fn get_contextual_type_for_array_literal_at_position(
        &mut self,
        contextual_array_type: Option<TypeRef>,
        array_literal: NodeId,
        position: i64,
    ) -> Result<Option<TypeRef>, Error> {
        let contextual_array_type = contextual_array_type
            .map(|ty| self.check_type(ty))
            .transpose()?;
        let ty = self
            .state_mut()
            .contextual_type_for_array_literal_at_position(
                contextual_array_type,
                array_literal,
                position,
            )?;
        Ok(self.optional_type(ty))
    }

    /// `GetFirstTypeArgumentFromKnownType`.
    pub fn get_first_type_argument_from_known_type(
        &mut self,
        ty: TypeRef,
    ) -> Result<Option<TypeRef>, Error> {
        let ty = self.check_type(ty)?;
        let result = self.state_mut().first_type_argument_from_known_type(ty)?;
        Ok(self.optional_type(result))
    }

    /// `GetPropertySymbolsFromContextualType`.
    pub fn get_property_symbols_from_contextual_type(
        &mut self,
        node: NodeId,
        contextual_type: TypeRef,
        union_symbol_ok: bool,
    ) -> Result<Vec<SymbolRef>, Error> {
        let contextual_type = self.check_type(contextual_type)?;
        let symbols = self.state_mut().property_symbols_from_contextual_type(
            node,
            contextual_type,
            union_symbol_ok,
        )?;
        self.symbol_refs(&symbols)
    }

    /// `GetPropertySymbolOfDestructuringAssignment`.
    pub fn get_property_symbol_of_destructuring_assignment(
        &mut self,
        location: NodeId,
    ) -> Result<Option<SymbolRef>, Error> {
        let symbol = self
            .state_mut()
            .property_symbol_of_destructuring_assignment(location)?;
        self.optional_symbol(symbol)
    }

    // port: tsc/internal/checker/services.go:Checker.GetSignatureFromDeclaration
    pub fn get_signature_from_declaration(&mut self, node: NodeId) -> Result<SignatureRef, Error> {
        let signature = self.state_mut().signature_from_declaration(node)?;
        Ok(self.signature_ref(signature))
    }
}
