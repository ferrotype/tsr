//! A native `NewNodeBuilder` lifetime, borrowed from a checker operation.
//! Generated syntax and its emit metadata live together. Raw IDs are usable
//! only through this builder's checked view; no checker-local type ID escapes.

use super::{Error, Operation, TypeRef};
use tsr_arena::{ArenaId, NodeId};
use tsr_ast::AstView;

pub struct TypeNodeBuilder<'operation> {
    owner: ArenaId,
    builder: crate::node_builder::NodeBuilder<'operation>,
}

impl Operation<'_> {
    // port: tsc/internal/checker/printer.go:Checker.TypeToString
    /// The context-free display with `TypeToString`'s default flags.
    pub fn type_to_string_default(&mut self, ty: TypeRef) -> Result<tsr_ast::JsString, Error> {
        let ty = self.check_type(ty)?;
        self.state_mut()
            .type_to_string_at(ty, None, crate::type_display::DEFAULT_FLAGS)
    }

    // port: tsc/internal/checker/printer.go:Checker.TypeToStringEx
    /// `TypeToStringEx` with the hover verbosity; its signals are written back.
    pub fn type_to_string_ex(
        &mut self,
        ty: TypeRef,
        enclosing: Option<NodeId>,
        flags: crate::TypeFormatFlags,
        verbosity: Option<&mut crate::VerbosityContext>,
    ) -> Result<tsr_ast::JsString, Error> {
        let ty = self.check_type(ty)?;
        self.state_mut()
            .type_to_string_ex(ty, enclosing, flags, verbosity)
    }

    // port: tsc/internal/checker/printer.go:Checker.SymbolToString
    pub fn symbol_to_string(
        &mut self,
        symbol: super::SymbolRef,
    ) -> Result<tsr_ast::JsString, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.state_mut().symbol_to_string(symbol)
    }

    // port: tsc/internal/checker/printer.go:Checker.SignatureToStringEx
    pub fn signature_to_string_ex(
        &mut self,
        signature: super::SignatureRef,
        enclosing: Option<NodeId>,
        flags: crate::TypeFormatFlags,
        verbosity: Option<&mut crate::VerbosityContext>,
    ) -> Result<tsr_ast::JsString, Error> {
        let signature = self.check_signature(signature)?;
        self.state_mut()
            .signature_to_string_ex(signature, enclosing, flags, verbosity)
    }

    // port: tsc/internal/checker/printer.go:Checker.TypeParameterToStringEx
    pub fn type_parameter_to_string_ex(
        &mut self,
        ty: TypeRef,
        enclosing: Option<NodeId>,
        verbosity: Option<&mut crate::VerbosityContext>,
    ) -> Result<tsr_ast::JsString, Error> {
        let ty = self.check_type(ty)?;
        self.state_mut()
            .type_parameter_to_string_ex(ty, enclosing, verbosity)
    }

    /// `SymbolToStringEx`, including lexical qualification and computed names.
    // port: tsc/internal/checker/printer.go:Checker.SymbolToStringEx
    pub fn symbol_to_string_at(
        &mut self,
        symbol: super::SymbolRef,
        enclosing: Option<NodeId>,
        meaning: tsr_ast::SymbolFlags,
        flags: crate::SymbolFormatFlags,
    ) -> Result<tsr_ast::JsString, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.state_mut()
            .symbol_to_string_at(symbol, enclosing, meaning, flags)
    }

    /// Creates the explicit builder used by callers of Go's `NewNodeBuilder`.
    /// This is separate from the checker's cached diagnostic-display builder.
    /// Its identifier-to-symbol map is the builder's own (`id_to_symbol`).
    // port: tsc/internal/checker/nodebuilder.go:NewNodeBuilder
    // port: tsc/internal/checker/nodebuilder.go:NewNodeBuilderEx
    // port: tsc/internal/checker/nodebuilder.go:Checker.getNodeBuilderEx
    pub fn node_builder(&mut self) -> TypeNodeBuilder<'_> {
        TypeNodeBuilder {
            owner: self.checker(),
            builder: crate::node_builder::NodeBuilder::new(self.state_mut(), 0),
        }
    }

    /// `TypeToStringEx` with an explicit enclosing declaration. The declaration
    /// must belong to a source retained by this checker. `None` is the native
    /// context-free display operation.
    pub fn type_to_string_at(
        &mut self,
        ty: TypeRef,
        enclosing: Option<NodeId>,
        flags: crate::TypeFormatFlags,
    ) -> Result<tsr_ast::JsString, Error> {
        let ty = self.check_type(ty)?;
        self.state_mut().type_to_string_at(ty, enclosing, flags)
    }
}

/// The context of one request: the enclosing declaration and the builder flags.
#[derive(Clone, Copy, Debug, Default)]
pub struct BuilderRequest {
    pub enclosing: Option<NodeId>,
    pub flags: tsr_nodebuilder::Flags,
    pub internal_flags: tsr_nodebuilder::InternalFlags,
}

impl TypeNodeBuilder<'_> {
    fn check_owner(&self, owner: ArenaId) -> Result<(), Error> {
        if owner != self.owner {
            return Err(tsr_arena::Error::WrongOwner.into());
        }
        Ok(())
    }

    fn type_id(&self, ty: TypeRef) -> Result<crate::TypeId, Error> {
        self.check_owner(ty.owner)?;
        self.builder.checker.types.get(ty.id)?;
        Ok(ty.id)
    }

    fn symbol_id(&self, symbol: super::SymbolRef) -> Result<tsr_arena::SymbolId, Error> {
        self.check_owner(symbol.owner)?;
        self.builder.checker.symbol(symbol.id)?;
        Ok(symbol.id)
    }

    /// Runs `action` as one request (`enterContext` .. `exitContext`); an
    /// unsuccessful native result is absent, never replaced with `any`.
    fn request(
        &mut self,
        request: BuilderRequest,
        action: impl FnOnce(&mut crate::node_builder::NodeBuilder<'_>) -> Result<NodeId, Error>,
    ) -> Result<Option<NodeId>, Error> {
        self.builder
            .prepare_context(request.enclosing, request.flags, request.internal_flags)?;
        let node = action(&mut self.builder)?;
        Ok(self.builder.exit_context(node))
    }

    fn request_slice(
        &mut self,
        request: BuilderRequest,
        action: impl FnOnce(&mut crate::node_builder::NodeBuilder<'_>) -> Result<Vec<NodeId>, Error>,
    ) -> Result<Option<Vec<NodeId>>, Error> {
        self.builder
            .prepare_context(request.enclosing, request.flags, request.internal_flags)?;
        let nodes = action(&mut self.builder)?;
        Ok(self.builder.exit_context_slice(nodes))
    }

    /// Runs one `TypeToTypeNode` request, keeping its syntax alive for inspection
    /// or printing and subsequent requests on this builder. An unsuccessful
    /// native builder result remains absent; it is not replaced with `any`.
    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.TypeToTypeNode
    pub fn type_to_type_node(
        &mut self,
        ty: TypeRef,
        enclosing: Option<NodeId>,
        flags: tsr_nodebuilder::Flags,
        internal_flags: tsr_nodebuilder::InternalFlags,
    ) -> Result<Option<NodeId>, Error> {
        let ty = self.type_id(ty)?;
        let request = BuilderRequest {
            enclosing,
            flags,
            internal_flags,
        };
        self.request(request, |b| b.type_node(ty))
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.SerializeTypeForDeclaration
    pub fn serialize_type_for_declaration(
        &mut self,
        declaration: NodeId,
        symbol: Option<super::SymbolRef>,
        request: BuilderRequest,
    ) -> Result<Option<NodeId>, Error> {
        let symbol = symbol.map(|s| self.symbol_id(s)).transpose()?;
        self.request(request, |b| {
            b.serialize_declaration_type(Some(declaration), None, symbol, true)
        })
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.SerializeTypeForExpression
    pub fn serialize_type_for_expression(
        &mut self,
        expression: NodeId,
        request: BuilderRequest,
    ) -> Result<Option<NodeId>, Error> {
        self.request(request, |b| b.serialize_expression_type(expression))
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.SerializeReturnTypeForSignature
    /// The return type node of the signature `declaration` declares; `None`
    /// also when the builder serializes no return type.
    pub fn serialize_return_type_for_signature(
        &mut self,
        declaration: NodeId,
        request: BuilderRequest,
    ) -> Result<Option<NodeId>, Error> {
        self.builder
            .prepare_context(request.enclosing, request.flags, request.internal_flags)?;
        let signature = self
            .builder
            .checker
            .signature_from_declaration(declaration)?;
        let result = self
            .builder
            .with_signature_scope(signature, |b| b.serialize_signature_return(signature, true))?;
        if let Some(node) = result {
            return Ok(self.builder.exit_context(node));
        }
        self.builder.exit_context_slice(Vec::new());
        Ok(None)
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.SerializeTypeParametersForSignature
    pub fn serialize_type_parameters_for_signature(
        &mut self,
        declaration: NodeId,
        request: BuilderRequest,
    ) -> Result<Option<Vec<NodeId>>, Error> {
        self.request_slice(request, |b| b.declaration_type_parameters(declaration))
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.SignatureToSignatureDeclaration
    pub fn signature_to_signature_declaration(
        &mut self,
        signature: super::SignatureRef,
        kind: tsr_ast::SyntaxKind,
        request: BuilderRequest,
    ) -> Result<Option<NodeId>, Error> {
        self.check_owner(signature.owner)?;
        self.builder.checker.signatures.get(signature.id)?;
        self.request(request, |b| {
            b.signature_node(signature.id, kind, None, None)
        })
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.SymbolToParameterDeclaration
    pub fn symbol_to_parameter_declaration(
        &mut self,
        symbol: super::SymbolRef,
        request: BuilderRequest,
    ) -> Result<Option<NodeId>, Error> {
        let symbol = self.symbol_id(symbol)?;
        self.request(request, |b| b.parameter_node(symbol))
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.TypeParameterToDeclaration
    pub fn type_parameter_to_declaration(
        &mut self,
        parameter: TypeRef,
        request: BuilderRequest,
    ) -> Result<Option<NodeId>, Error> {
        let parameter = self.type_id(parameter)?;
        self.request(request, |b| b.type_parameter_node(parameter))
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.IndexInfoToIndexSignatureDeclaration
    pub fn index_info_to_index_signature_declaration(
        &mut self,
        info: crate::IndexInfoRef,
        request: BuilderRequest,
    ) -> Result<Option<NodeId>, Error> {
        self.check_owner(info.owner)?;
        self.builder.checker.signatures.index_info(info.id)?;
        self.request(request, |b| b.index_signature_node(info.id))
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.SymbolToExpression
    pub fn symbol_to_expression(
        &mut self,
        symbol: super::SymbolRef,
        meaning: tsr_ast::SymbolFlags,
        request: BuilderRequest,
    ) -> Result<Option<NodeId>, Error> {
        let symbol = self.symbol_id(symbol)?;
        let enclosing = request.enclosing;
        self.request(request, |b| {
            b.symbol_expression_with_meaning(symbol, enclosing, meaning)
        })
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.SymbolToEntityName
    pub fn symbol_to_entity_name(
        &mut self,
        symbol: super::SymbolRef,
        meaning: tsr_ast::SymbolFlags,
        request: BuilderRequest,
    ) -> Result<Option<NodeId>, Error> {
        let symbol = self.symbol_id(symbol)?;
        self.request(request, |b| b.symbol_name_node(symbol, meaning, false))
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.SymbolToNode
    pub fn symbol_to_node(
        &mut self,
        symbol: super::SymbolRef,
        meaning: tsr_ast::SymbolFlags,
        request: BuilderRequest,
    ) -> Result<Option<NodeId>, Error> {
        let symbol = self.symbol_id(symbol)?;
        self.request(request, |b| b.symbol_display_node(symbol, meaning, true))
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.SymbolToTypeParameterDeclarations
    pub fn symbol_to_type_parameter_declarations(
        &mut self,
        symbol: super::SymbolRef,
        request: BuilderRequest,
    ) -> Result<Option<Vec<NodeId>>, Error> {
        let symbol = self.symbol_id(symbol)?;
        self.request_slice(request, |b| b.symbol_type_parameter_declarations(symbol))
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.TypePredicateToTypePredicateNode
    pub fn type_predicate_to_type_predicate_node(
        &mut self,
        predicate: crate::TypePredicateRef,
        request: BuilderRequest,
    ) -> Result<Option<NodeId>, Error> {
        self.check_owner(predicate.owner)?;
        self.builder.checker.signatures.predicate(predicate.id)?;
        self.request(request, |b| b.predicate_node(predicate.id))
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.TryJSTypeNodeToTypeNode
    /// The reused form of a JSDoc type annotation, or `None` when it cannot be reused.
    pub fn try_js_type_node_to_type_node(
        &mut self,
        node: NodeId,
        request: BuilderRequest,
    ) -> Result<Option<NodeId>, Error> {
        self.builder
            .prepare_context(request.enclosing, request.flags, request.internal_flags)?;
        if let Some(result) = self.builder.try_js_type_node_to_type_node(node)? {
            return Ok(self.builder.exit_context(result));
        }
        self.builder.exit_context_slice(Vec::new());
        Ok(None)
    }

    /// The symbol an identifier the builder created names, if it recorded one.
    pub fn id_to_symbol(&self, identifier: NodeId) -> Option<super::SymbolRef> {
        self.builder
            .id_to_symbol
            .get(&identifier)
            .copied()
            .flatten()
            .map(|id| super::SymbolRef {
                owner: self.owner,
                id,
            })
    }

    pub fn view(&self) -> AstView<'_> {
        self.builder.ast.view()
    }

    // port: tsc/internal/checker/nodebuilder.go:NodeBuilder.EmitContext
    pub fn emit_context(&self) -> &tsr_printer::EmitContext {
        &self.builder.emit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tsr_arena::{CheckerIdentity, Counters, Generation};
    use tsr_nodebuilder::{flags as nf, internal_flags as inf};
    use tsr_printer::{EmitTextWriter, Printer, PrinterOptions, TextWriter};

    fn owner(counters: &Counters) -> Arc<crate::CheckerOwner> {
        Arc::new(
            crate::CheckerOwner::new(
                CheckerIdentity::new(Generation::new(counters), counters),
                counters,
                crate::CheckerOptions::default(),
            )
            .unwrap(),
        )
    }

    fn print(builder: &TypeNodeBuilder<'_>, node: NodeId) -> Vec<u8> {
        let mut writer = TextWriter::new(b"", 0);
        Printer::new(
            PrinterOptions {
                remove_comments: true,
                ..Default::default()
            },
            builder.emit_context(),
        )
        .write(builder.view(), node, None, &mut writer, None)
        .unwrap();
        writer.text().to_vec()
    }

    #[test]
    fn builder_requests_reset_flags_and_preserve_previous_nodes_until_drop() {
        let counters = Counters::new();
        let owner = owner(&counters);
        let mut op = owner.operation().unwrap();
        let typ = op.string_literal_type(b"text").unwrap();
        let before = counters.snapshot();
        {
            let mut builder = op.node_builder();
            let first = builder
                .type_to_type_node(
                    typ,
                    None,
                    nf::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE,
                    inf::NONE,
                )
                .unwrap()
                .unwrap();
            let second = builder
                .type_to_type_node(typ, None, nf::NONE, inf::NONE)
                .unwrap()
                .unwrap();
            assert_eq!(print(&builder, first), b"'text'");
            assert_eq!(print(&builder, second), b"\"text\"");
        }
        assert_eq!(
            counters.snapshot(),
            before,
            "the builder releases syntax after its last borrowed view"
        );
    }

    #[test]
    fn display_rejects_foreign_type_and_symbol_before_access_and_remains_usable() {
        let counters = Counters::new();
        let local = owner(&counters);
        let foreign = owner(&counters);
        let (foreign_type, foreign_symbol) = {
            let mut op = foreign.operation().unwrap();
            let symbol = op
                .new_symbol(tsr_ast::symbol_flags::VARIABLE, b"foreign", 0)
                .unwrap();
            (
                op.builtin_type("stringType").unwrap(),
                op.symbol_ref(symbol).unwrap(),
            )
        };
        let mut op = local.operation().unwrap();
        let local_type = op.builtin_type("stringType").unwrap();
        assert_eq!(
            foreign_type.id(),
            local_type.id(),
            "the counterexample uses the same numeric slot"
        );
        assert!(matches!(
            op.type_to_string_at(foreign_type, None, 0),
            Err(Error::Arena(tsr_arena::Error::WrongOwner))
        ));
        assert!(matches!(
            op.symbol_to_string_at(foreign_symbol, None, 0, 0),
            Err(Error::Arena(tsr_arena::Error::WrongOwner))
        ));
        let mut builder = op.node_builder();
        assert!(matches!(
            builder.type_to_type_node(foreign_type, None, 0, 0),
            Err(Error::Arena(tsr_arena::Error::WrongOwner))
        ));
        let node = builder
            .type_to_type_node(local_type, None, 0, 0)
            .unwrap()
            .unwrap();
        assert_eq!(print(&builder, node), b"string");
    }
}
