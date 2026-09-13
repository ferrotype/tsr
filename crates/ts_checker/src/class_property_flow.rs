//! Constructor/static-block property inference uses the same flow engine as a
//! source property read, with a retained synthetic `this.name` reference.
use crate::{type_facts as facts, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{modifier_flags as mf, Factory, FactoryMethods, JsString, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.isPropertyInitializedInConstructor
    pub(crate) fn property_initialized_in_constructor(
        &mut self,
        name: NodeId,
        ty: TypeId,
        constructor: NodeId,
    ) -> Result<bool, Error> {
        let initial = self.add_type_optionality(ty, false, true)?;
        let flow = self.class_property_reference_flow(name, ty, initial, constructor)?;
        Ok(!self.class_type_contains_undefined(flow)?)
    }

    // port: tsc/internal/checker/checker.go:Checker.isPropertyInitializedInStaticBlocks
    pub(crate) fn property_initialized_in_static_blocks(
        &mut self,
        name: NodeId,
        ty: TypeId,
        blocks: &[NodeId],
        start: i32,
        end: i32,
    ) -> Result<bool, Error> {
        for &block in blocks {
            let pos = self.ast(block)?.node(block)?.pos();
            if pos >= start && pos <= end {
                let initial = self.get_union_type(&[ty, self.builtins.undefined_type])?;
                let flow =
                    self.class_property_reference_flow_ex(name, ty, initial, block, false)?;
                if !self.class_type_contains_undefined(flow)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/flow.go:Checker.getFlowTypeInConstructor
    // port: tsc/internal/checker/flow.go:Checker.getFlowTypeInStaticBlocks
    pub(crate) fn infer_class_property_flow(
        &mut self,
        symbol: SymbolId,
        containers: &[NodeId],
    ) -> Result<Option<TypeId>, Error> {
        let name = self.symbol(symbol)?.name_to_owned();
        let name = if name.as_bytes().starts_with(b"\xfe#") {
            let offset = name
                .as_bytes()
                .iter()
                .position(|&byte| byte == b'@')
                .map_or(0, |index| index + 1);
            self.factory
                .new_private_identifier(JsString::from_bytes(&name.as_bytes()[offset..]))
        } else {
            self.factory.new_identifier(name)
        };
        for &container in containers {
            let mut initial = self.builtins.undefined_type;
            if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
                let read = self.ast(declaration)?.node(declaration)?;
                let options = self.program()?.host.options();
                let auto = read.kind() == K::PropertyDeclaration
                    && read.type_node().is_none()
                    && read.initializer().is_none()
                    && options.strict_option_value(options.no_implicit_any);
                if !auto || read.modifier_flags(self.ast(declaration)?)? & mf::AMBIENT != 0 {
                    if let Some(base) = self.type_of_property_in_base_class(symbol)? {
                        initial = base;
                    }
                }
            }
            let flow = self.class_property_reference_flow(
                name,
                self.builtins.auto_type,
                initial,
                container,
            )?;
            let auto_array = self.auto_array_type()?;
            let options = self.program()?.host.options();
            if options.strict_option_value(options.no_implicit_any)
                && (flow == self.builtins.auto_type || flow == auto_array)
            {
                let display = self.type_to_string(flow, crate::type_display::DEFAULT_FLAGS)?;
                let name = self.symbol_to_string(symbol)?;
                self.error_at(
                    self.symbol(symbol)?.value_declaration(),
                    d::Member_0_implicitly_has_an_1_type,
                    vec![name, display],
                )?;
            }
            let parts = if self.types.flags(flow)? & tf::UNION != 0 {
                self.types.compound_types(flow)?.to_vec()
            } else {
                vec![flow]
            };
            let mut nullable = true;
            for part in parts {
                if self.type_facts(part, facts::IS_UNDEFINED_OR_NULL)? == 0 {
                    nullable = false;
                    break;
                }
            }
            if nullable {
                continue;
            }
            return Ok(Some(if flow == self.builtins.auto_type {
                self.builtins.any_type
            } else if flow == auto_array {
                self.any_array_type()?
            } else {
                flow
            }));
        }
        Ok(None)
    }

    fn class_property_reference_flow(
        &mut self,
        name: NodeId,
        declared: TypeId,
        initial: TypeId,
        container: NodeId,
    ) -> Result<TypeId, Error> {
        self.class_property_reference_flow_ex(name, declared, initial, container, true)
    }

    fn class_property_reference_flow_ex(
        &mut self,
        name: NodeId,
        declared: TypeId,
        initial: TypeId,
        container: NodeId,
        computed: bool,
    ) -> Result<TypeId, Error> {
        let flow = self
            .program()?
            .bound(container)?
            .node_binding(container)?
            .and_then(|binding| binding.return_flow_node);
        let Some(flow) = flow else {
            return Ok(declared);
        };
        self.retain_flow_source(container)?;
        let this = self.factory.new_keyword_expression(K::ThisKeyword.into());
        let reference = if computed && self.ast(name)?.node(name)?.kind() == K::ComputedPropertyName
        {
            let expression = self
                .ast(name)?
                .node(name)?
                .expression()
                .ok_or(Error::MissingLink("computed property name expression"))?;
            self.factory
                .new_element_access_expression(Some(this), None, Some(expression), 0)
        } else {
            self.factory
                .new_property_access_expression(Some(this), None, Some(name), 0)
        };
        self.factory.set_node_parent(this, Some(reference));
        self.factory.set_node_parent(reference, Some(container));
        self.flow.synthetic.insert(reference, (container, flow));
        self.flow_type_of_reference_with_container(reference, declared, initial, None)
    }
}
