//! The generated-name constructors of `printer.NodeFactory` (`factory.go`).
//! As with [`EmitContext::new_unique_name_ex`], the factory the pinned
//! `NodeFactory` embeds is passed explicitly; it must carry this context's
//! [`EmitContext::factory_hooks`].

use crate::emit_context::next_auto_generate_id;
use crate::generated_identifier_flags as g;
use crate::namegenerator::format_generated_name;
use crate::{AutoGenerateInfo, AutoGenerateOptions, EmitContext};
use tsr_ast::{utilities::is_member_name, Factory, FactoryMethods, JsString, NodeAccess, NodeId};

impl EmitContext {
    /// Allocates a new temp variable name, but does not record it in the
    /// environment.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewTempVariable
    pub fn new_temp_variable(&mut self, factory: &mut dyn Factory) -> NodeId {
        self.new_temp_variable_ex(factory, AutoGenerateOptions::default())
    }

    /// Allocates a new temp variable name, but does not record it in the
    /// environment.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewTempVariableEx
    pub fn new_temp_variable_ex(
        &mut self,
        factory: &mut dyn Factory,
        options: AutoGenerateOptions,
    ) -> NodeId {
        self.new_generated_identifier(factory, g::AUTO, JsString::default(), None, options)
    }

    /// Allocates a new loop variable name.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewLoopVariable
    pub fn new_loop_variable(&mut self, factory: &mut dyn Factory) -> NodeId {
        self.new_loop_variable_ex(factory, AutoGenerateOptions::default())
    }

    /// Allocates a new loop variable name.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewLoopVariableEx
    pub fn new_loop_variable_ex(
        &mut self,
        factory: &mut dyn Factory,
        options: AutoGenerateOptions,
    ) -> NodeId {
        self.new_generated_identifier(factory, g::LOOP, JsString::default(), None, options)
    }

    /// Allocates a new unique name based on the provided text.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewUniqueName
    pub fn new_unique_name(&mut self, factory: &mut dyn Factory, text: JsString) -> NodeId {
        self.new_unique_name_ex(factory, text, AutoGenerateOptions::default())
    }

    /// Allocates a new unique name based on the provided node.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewGeneratedNameForNode
    pub fn new_generated_name_for_node(
        &mut self,
        factory: &mut dyn Factory,
        node: NodeId,
    ) -> NodeId {
        self.new_generated_name_for_node_ex(factory, node, AutoGenerateOptions::default())
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.newGeneratedPrivateIdentifier
    fn new_generated_private_identifier(
        &mut self,
        factory: &mut dyn Factory,
        kind: g::Flags,
        mut text: JsString,
        node: Option<NodeId>,
        options: AutoGenerateOptions,
    ) -> NodeId {
        let id = next_auto_generate_id();

        if text.is_empty() {
            text = match node {
                None => JsString::from_bytes(format!("(auto@{})", id.get()).into_bytes()),
                Some(node) if is_member_name(&factory.node(node)) => {
                    let read = factory.node(node);
                    if let Some(data) = read.as_identifier() {
                        data.text_owned()
                    } else {
                        read.as_private_identifier()
                            .expect("IsMemberName is an identifier or private identifier")
                            .text_owned()
                    }
                }
                Some(node) => {
                    let root = self.generated_name_root(factory, node, id);
                    JsString::from_bytes(
                        format!("(generated@{})", factory.node(root).runtime_id()).into_bytes(),
                    )
                }
            };
            text = format_generated_name(
                true, /*privateName*/
                options.prefix.as_bytes(),
                text.as_bytes(),
                options.suffix.as_bytes(),
            );
        } else if !text.as_bytes().starts_with(b"#") {
            let mut message = b"First character of private identifier must be #: ".to_vec();
            message.extend_from_slice(text.as_bytes());
            panic!("{}", String::from_utf8_lossy(&message));
        }

        let name = factory.new_private_identifier(text);
        self.set_auto_generate_info(
            name,
            AutoGenerateInfo {
                id,
                flags: kind | (options.flags & !g::KIND_MASK),
                prefix: options.prefix,
                suffix: options.suffix,
                node,
            },
        );
        name
    }

    /// Allocates a new unique private name based on the provided text.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewUniquePrivateName
    pub fn new_unique_private_name(&mut self, factory: &mut dyn Factory, text: JsString) -> NodeId {
        self.new_unique_private_name_ex(factory, text, AutoGenerateOptions::default())
    }

    /// Allocates a new unique private name based on the provided text.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewUniquePrivateNameEx
    pub fn new_unique_private_name_ex(
        &mut self,
        factory: &mut dyn Factory,
        text: JsString,
        options: AutoGenerateOptions,
    ) -> NodeId {
        self.new_generated_private_identifier(factory, g::UNIQUE, text, None, options)
    }

    /// Allocates a new unique private name based on the provided node.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewGeneratedPrivateNameForNode
    pub fn new_generated_private_name_for_node(
        &mut self,
        factory: &mut dyn Factory,
        node: NodeId,
    ) -> NodeId {
        self.new_generated_private_name_for_node_ex(factory, node, AutoGenerateOptions::default())
    }

    /// Allocates a new unique private name based on the provided node.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewGeneratedPrivateNameForNodeEx
    pub fn new_generated_private_name_for_node_ex(
        &mut self,
        factory: &mut dyn Factory,
        node: NodeId,
        mut options: AutoGenerateOptions,
    ) -> NodeId {
        if !options.prefix.is_empty() || !options.suffix.is_empty() {
            options.flags |= g::OPTIMISTIC;
        }

        self.new_generated_private_identifier(
            factory,
            g::NODE,
            JsString::default(),
            Some(node),
            options,
        )
    }
}
