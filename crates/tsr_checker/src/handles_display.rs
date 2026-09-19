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
    /// `SymbolToStringEx`, including lexical qualification and computed names.
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

impl TypeNodeBuilder<'_> {
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
        if ty.owner != self.owner {
            return Err(tsr_arena::Error::WrongOwner.into());
        }
        self.builder.checker.types.get(ty.id)?;
        self.builder
            .prepare_context(enclosing, flags, internal_flags)?;
        let node = self.builder.type_node(ty.id)?;
        Ok((!self.builder.encountered_error).then_some(node))
    }

    pub fn view(&self) -> AstView<'_> {
        self.builder.ast.view()
    }

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
        .write(builder.view(), node, None, &mut writer)
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
