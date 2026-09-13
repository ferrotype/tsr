//! Duplicate and subsequent declaration checks preserve native first-declaration authority.
use crate::{CheckerState, Error, RelationKind, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{modifier_flags as mf, symbol_flags as sf, JsString, SyntaxKind as K};
use ts_diagnostics as d;
impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkVarDeclaredNamesNotShadowed
    pub(crate) fn check_var_names_not_shadowed(&mut self, node: NodeId) -> Result<(), Error> {
        use ts_ast::node_flags as nf;
        if ts_ast::utilities::get_combined_node_flags(self.ast(node)?, node)? & nf::BLOCK_SCOPED
            != 0
            || ts_ast::utilities::is_part_of_parameter_declaration(self.ast(node)?, node)?
        {
            return Ok(());
        }
        let symbol = self
            .get_symbol_of_declaration(node)?
            .ok_or(Error::MissingLink("var declaration symbol"))?;
        if self.symbol(symbol)?.flags() & sf::FUNCTION_SCOPED_VARIABLE == 0 {
            return Ok(());
        }
        let name = self
            .ast(node)?
            .node(node)?
            .name()
            .ok_or(Error::MissingLink("var name"))?;
        let text = self.ast(name)?.node_text(name)?.into_js_string();
        let Some(local) =
            self.resolve_name(Some(node), text.as_bytes(), sf::VARIABLE, None, false)?
        else {
            return Ok(());
        };
        if local == symbol || self.symbol(local)?.flags() & sf::BLOCK_SCOPED_VARIABLE == 0 {
            return Ok(());
        }
        let Some(declaration) = self.symbol(local)?.value_declaration() else {
            return Ok(());
        };
        if ts_ast::utilities::get_combined_node_flags(self.ast(declaration)?, declaration)?
            & nf::BLOCK_SCOPED
            == 0
        {
            return Ok(());
        }
        let mut current = Some(declaration);
        let mut container = None;
        while let Some(ancestor) = current {
            let read = self.ast(ancestor)?.node(ancestor)?;
            if read.kind() == K::VariableDeclarationList {
                let statement = read
                    .parent()
                    .ok_or(Error::MissingLink("shadowed declaration statement"))?;
                let read = self.ast(statement)?.node(statement)?;
                if read.kind() == K::VariableStatement {
                    container = read.parent();
                }
                break;
            }
            current = read.parent();
        }
        let shared_scope = match container {
            None => false,
            Some(container) => {
                let read = self.ast(container)?.node(container)?;
                match read.kind().known() {
                    Some(K::ModuleBlock | K::ModuleDeclaration | K::SourceFile) => true,
                    Some(K::Block) => match read.parent() {
                        Some(parent) => ts_ast::utilities::is_function_like(Some(
                            &self.ast(parent)?.node(parent)?,
                        )),
                        None => false,
                    },
                    _ => false,
                }
            }
        };
        if !shared_scope {
            let name = self.symbol_to_string(local)?;
            self.error_at(Some(node), d::Cannot_initialize_outer_scoped_variable_0_in_the_same_scope_as_block_scoped_declaration_1, vec![name.clone(), name])?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkObjectTypeForDuplicateDeclarations
    pub(crate) fn check_object_duplicate_declarations(
        &mut self,
        node: NodeId,
        check_private: bool,
    ) -> Result<(), Error> {
        let members = self.source_list(node, self.ast(node)?.node(node)?.member_list())?;
        let mut instance = crate::types::Map::default();
        let mut static_names = crate::types::Map::default();
        let mut private = crate::types::Map::default();
        let ambient = self.ast(node)?.node(node)?.flags() & ts_ast::node_flags::AMBIENT != 0;
        for member in members {
            let read = self.ast(member)?.node(member)?;
            if read.kind() == K::Constructor {
                let parameters = self.source_list(member, read.parameter_list())?;
                for parameter in parameters {
                    if ts_ast::utilities::is_parameter_property_declaration(
                        self.ast(parameter)?,
                        parameter,
                        member,
                    )? {
                        let name = self
                            .ast(parameter)?
                            .node(parameter)?
                            .name()
                            .ok_or(Error::MissingLink("parameter property name"))?;
                        if !self.is_binding_pattern(name)? {
                            let symbol = self
                                .get_symbol_of_declaration(parameter)?
                                .ok_or(Error::MissingLink("parameter property symbol"))?;
                            self.record_property_or_accessor(
                                node,
                                symbol,
                                1,
                                false,
                                &mut instance,
                            )?;
                        }
                    }
                }
                continue;
            }
            let is_static = read.modifier_flags(self.ast(member)?)? & mf::STATIC != 0;
            let kind = read.kind();
            let name = read.name();
            let auto = read.modifier_flags(self.ast(member)?)? & mf::ACCESSOR != 0;
            let symbol = self.get_symbol_of_declaration(member)?;
            if !ambient && is_static {
                if let Some(symbol) = symbol {
                    if self.symbol(symbol)?.name_bytes() == b"prototype" {
                        let name = self.symbol(symbol)?.name_to_owned();
                        let owner = self
                            .get_symbol_of_declaration(node)?
                            .ok_or(Error::MissingLink("duplicate owner"))?;
                        let owner = self.symbol_to_string(owner)?;
                        self.error_at(self.ast(member)?.node(member)?.name(),d::Static_property_0_conflicts_with_built_in_property_Function_0_of_constructor_function_1,vec![name,owner])?;
                    }
                }
            }
            if let Some(symbol) = symbol {
                let record_kind =
                    if kind == K::PropertyDeclaration && !auto || kind == K::PropertySignature {
                        1
                    } else if matches!(kind.known(), Some(K::GetAccessor | K::SetAccessor))
                        || kind == K::PropertyDeclaration && auto
                    {
                        2
                    } else {
                        0
                    };
                if record_kind != 0 {
                    self.record_property_or_accessor(
                        node,
                        symbol,
                        record_kind,
                        is_static,
                        if is_static {
                            &mut static_names
                        } else {
                            &mut instance
                        },
                    )?;
                }
                if check_private
                    && name
                        .map(|name| {
                            self.ast(name)?
                                .node(name)
                                .map(|n| n.kind() == K::PrivateIdentifier)
                                .map_err(Error::from)
                        })
                        .transpose()?
                        .unwrap_or(false)
                {
                    let name = self.symbol(symbol)?.name_to_owned();
                    let state = private.get(&name).copied().unwrap_or(0u8);
                    if state != 3 {
                        let state = state | if is_static { 2 } else { 1 };
                        private.insert(name.clone(), state);
                        if state == 3 {
                            self.report_duplicate_members(node,&name,None,d::Duplicate_identifier_0_Static_and_instance_elements_cannot_share_the_same_private_name)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn record_property_or_accessor(
        &mut self,
        node: NodeId,
        symbol: SymbolId,
        kind: u8,
        is_static: bool,
        names: &mut crate::types::Map<JsString, u8>,
    ) -> Result<(), Error> {
        if self.symbol_declarations(symbol)?.len() <= 1 {
            return Ok(());
        }
        let name = self.symbol(symbol)?.name_to_owned();
        let state = names.get(&name).copied().unwrap_or(0);
        if state == 0 {
            names.insert(name, kind);
        } else if state == 1 || state == 2 && kind != 2 {
            self.report_duplicate_members(node, &name, Some(is_static), d::Duplicate_identifier_0)?;
            names.insert(name, 3);
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.reportDuplicateMemberErrors
    fn report_duplicate_members(
        &mut self,
        node: NodeId,
        name: &JsString,
        static_filter: Option<bool>,
        message: &'static d::Message,
    ) -> Result<(), Error> {
        for member in self.source_list(node, self.ast(node)?.node(node)?.member_list())? {
            if self.ast(member)?.node(member)?.kind() == K::Constructor {
                for parameter in
                    self.source_list(member, self.ast(member)?.node(member)?.parameter_list())?
                {
                    if !ts_ast::utilities::is_parameter_property_declaration(
                        self.ast(parameter)?,
                        parameter,
                        member,
                    )? {
                        continue;
                    }
                    let parameter_name = self
                        .ast(parameter)?
                        .node(parameter)?
                        .name()
                        .ok_or(Error::MissingLink("duplicate parameter name"))?;
                    if self.is_binding_pattern(parameter_name)? {
                        continue;
                    }
                    let symbol = self
                        .get_symbol_of_declaration(parameter)?
                        .ok_or(Error::MissingLink("duplicate parameter symbol"))?;
                    if self.symbol(symbol)?.name_bytes() == name.as_bytes() {
                        let display = self.symbol_to_string(symbol)?;
                        self.error_at(Some(parameter_name), message, vec![display])?;
                    }
                }
            } else if let Some(symbol) = self.get_symbol_of_declaration(member)? {
                if self.symbol(symbol)?.name_bytes() == name.as_bytes()
                    && static_filter
                        .map(|is_static| {
                            self.ast(member)?
                                .node(member)?
                                .modifier_flags(self.ast(member)?)
                                .map(|flags| (flags & mf::STATIC != 0) == is_static)
                                .map_err(Error::from)
                        })
                        .transpose()?
                        .unwrap_or(true)
                {
                    let display = self.symbol_to_string(symbol)?;
                    self.error_at(
                        self.ast(member)?.node(member)?.name(),
                        message,
                        vec![display],
                    )?;
                }
            }
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.areDeclarationFlagsIdentical
    fn declaration_flags_identical(&self, left: NodeId, right: NodeId) -> Result<bool, Error> {
        let a = self.ast(left)?.node(left)?;
        let b = self.ast(right)?.node(right)?;
        if a.kind() == K::Parameter && b.kind() == K::VariableDeclaration
            || a.kind() == K::VariableDeclaration && b.kind() == K::Parameter
        {
            return Ok(true);
        }
        if a.question_token(self.ast(left)?)?.is_some()
            != b.question_token(self.ast(right)?)?.is_some()
        {
            return Ok(false);
        }
        let flags =
            mf::PRIVATE | mf::PROTECTED | mf::ASYNC | mf::ABSTRACT | mf::READONLY | mf::STATIC;
        Ok(a.modifier_flags(self.ast(left)?)? & flags
            == b.modifier_flags(self.ast(right)?)? & flags)
    }
    pub(crate) fn check_variable_declaration_flags(
        &mut self,
        node: NodeId,
        symbol: SymbolId,
        primary: bool,
    ) -> Result<(), Error> {
        let mut mismatch = false;
        if primary {
            for declaration in self
                .symbol_declarations(symbol)?
                .to_vec()
                .into_iter()
                .flatten()
            {
                if declaration != node
                    && ts_ast::utilities_middle::is_variable_like(
                        &self.ast(declaration)?.node(declaration)?,
                    )
                    && !self.declaration_flags_identical(declaration, node)?
                {
                    mismatch = true;
                    break;
                }
            }
        } else if let Some(first) = self.symbol(symbol)?.value_declaration() {
            mismatch = !self.declaration_flags_identical(node, first)?;
        }
        if mismatch {
            let name = self.ast(node)?.node(node)?.name();
            let text = ts_scanner::declaration_name_to_string(self.ast(node)?, name)?;
            self.error_at(
                name,
                d::All_declarations_of_0_must_have_identical_modifiers,
                vec![text],
            )?;
        }
        Ok(())
    }
    pub(crate) fn auto_to_any(&self, ty: TypeId) -> Result<TypeId, Error> {
        Ok(if ty == self.builtins.auto_type {
            self.builtins.any_type
        } else if self.query.global_types.get("autoArrayType") == Some(&ty) {
            *self
                .query
                .global_types
                .get("anyArrayType")
                .ok_or(Error::MissingLink("any array global"))?
        } else {
            ty
        })
    }
    // port: tsc/internal/checker/checker.go:Checker.checkVariableLikeDeclaration
    pub(crate) fn check_secondary_variable(
        &mut self,
        node: NodeId,
        symbol: SymbolId,
        primary_type: TypeId,
    ) -> Result<(), Error> {
        let raw = self.type_for_variable_like_raw(node, true, 0)?;
        let declared = self.widen_type_for_variable_like(node, raw, false)?;
        let declared = self.auto_to_any(declared)?;
        if !self.is_error_type(primary_type)?
            && !self.is_error_type(declared)?
            && !self.is_type_related_to(primary_type, declared, RelationKind::Identity)?
            && self.symbol(symbol)?.flags() & sf::ASSIGNMENT == 0
        {
            let first = self.symbol(symbol)?.value_declaration();
            self.error_subsequent_declaration(first, primary_type, node, declared)?;
        }
        if let Some(initializer) = self.ast(node)?.node(node)?.initializer() {
            let source = self.check_expression_cached(initializer)?;
            self.check_expression_related_with_elaboration(
                source,
                declared,
                RelationKind::Assignable,
                Some(node),
                Some(initializer),
                None,
            )?;
        }
        self.check_variable_declaration_flags(node, symbol, false)
    }
    // port: tsc/internal/checker/checker.go:Checker.errorNextVariableOrPropertyDeclarationMustHaveSameType
    fn error_subsequent_declaration(
        &mut self,
        first: Option<NodeId>,
        first_type: TypeId,
        next: NodeId,
        next_type: TypeId,
    ) -> Result<(), Error> {
        let read = self.ast(next)?.node(next)?;
        let name = read.name();
        let property = matches!(
            read.kind().known(),
            Some(K::PropertyDeclaration | K::PropertySignature)
        );
        let message = if property {
            d::Subsequent_property_declarations_must_have_the_same_type_Property_0_must_be_of_type_1_but_here_has_type_2
        } else {
            d::Subsequent_variable_declarations_must_have_the_same_type_Variable_0_must_be_of_type_1_but_here_has_type_2
        };
        let name_text = ts_scanner::declaration_name_to_string(self.ast(next)?, name)?;
        let a = self.type_to_string(first_type, crate::type_display::DEFAULT_FLAGS)?;
        let b = self.type_to_string(next_type, crate::type_display::DEFAULT_FLAGS)?;
        if let Some(index) = self.error_at(name, message, vec![name_text.clone(), a, b])? {
            if let Some(first) = first {
                let info = self.diagnostic_for_node(
                    Some(first),
                    d::X_0_was_also_declared_here,
                    vec![name_text],
                )?;
                self.add_related_diagnostic(index, info)?;
            }
        }
        Ok(())
    }
}
