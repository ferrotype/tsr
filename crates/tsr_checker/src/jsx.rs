//! JSX elements, attributes, children and the JSX namespace
//! (`tsc/internal/checker/jsx.go`). An element resolves like a call whose one
//! argument is its attributes object; the namespace, factory and implicit
//! runtime import are resolved once per location or file and cached, a failed
//! lookup under the unknown symbol.

use crate::{
    inference::priority, object_flags as of, signature_flags as sg, type_flags as tf, CheckerState,
    Error, InferenceId, LiteralValue, RelationKind, SignatureId, TypeId,
};
use std::sync::Arc;
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{
    symbol_flags as sf, Diagnostic, Factory, FactoryMethods, JsString, SymbolTable, SyntaxKind as K,
};
use tsr_diagnostics as d;

fn required<T>(value: Option<T>, name: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(name))
}

/// `JsxFlags`: how an intrinsic tag resolved against `JSX.IntrinsicElements`.
pub(crate) mod jsx_flags {
    pub const INTRINSIC_NAMED_ELEMENT: u32 = 1 << 0;
    pub const INTRINSIC_INDEXED_ELEMENT: u32 = 1 << 1;
}

/// `JsxReferenceKind`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum JsxReferenceKind {
    Component,
    Function,
    Mixed,
}

/// The name `getNameFromJsxElementAttributesContainer` finds: `None` is
/// `InternalSymbolNameMissing`, an empty name a container without properties.
type JsxPropertyName = Option<JsString>;

#[derive(Default)]
struct JsxFileLinks {
    fragment_type: Option<TypeId>,
    local_namespace: Option<JsString>,
    local_factory: Option<NodeId>,
    local_fragment_namespace: Option<JsString>,
    local_fragment_factory: Option<NodeId>,
}

/// `jsxElementLinks`, the JSX half of `sourceFileLinks`, and the checker's
/// `_jsxNamespace` and `_jsxFactoryEntity`.
#[derive(Default)]
pub(crate) struct JsxState {
    flags: crate::types::Map<NodeId, u32>,
    attributes_types: crate::types::Map<NodeId, TypeId>,
    /// The resolved `JSX` namespace per location; the unknown symbol marks a
    /// location whose own lookup failed.
    namespaces: crate::types::Map<NodeId, SymbolId>,
    /// The implicit runtime import per file; the unknown symbol marks none.
    implicit_imports: crate::types::Map<NodeId, SymbolId>,
    first_tags: crate::types::Map<NodeId, Option<NodeId>>,
    /// `symbolNodeLinks.resolvedSymbol` of intrinsic opening and closing tags.
    intrinsic_tag_symbols: crate::types::Map<NodeId, SymbolId>,
    files: crate::types::Map<NodeId, JsxFileLinks>,
    namespace: Option<JsString>,
    factory_entity: Option<NodeId>,
}

/// `getInvalidTextualChildDiagnostic`: the text-child message is built on
/// first use and shared by every text child of one element.
pub(crate) struct JsxTextChildMessage {
    tag_name: NodeId,
    property: JsString,
    target: TypeId,
    args: Option<Vec<JsString>>,
}

// port: tsc/internal/checker/relater.go:isHyphenatedJsxName
pub(crate) fn is_hyphenated_jsx_name(name: &[u8]) -> bool {
    name.contains(&b'-')
}

/// `JsxElaborationElement`.
struct JsxElaborationElement {
    error_node: NodeId,
    inner_expression: Option<NodeId>,
    name_type: TypeId,
    text: bool,
}

impl CheckerState {
    // port: tsc/internal/checker/relater.go:isIgnoredJsxProperty
    pub(crate) fn is_ignored_jsx_property(
        &self,
        source: TypeId,
        property: SymbolId,
    ) -> Result<bool, Error> {
        Ok(
            self.types.get(source)?.object_flags & of::JSX_ATTRIBUTES != 0
                && is_hyphenated_jsx_name(self.symbol(property)?.name_bytes()),
        )
    }

    // port: tsc/internal/checker/utilities.go:isJsxIntrinsicTagName
    pub(crate) fn is_jsx_intrinsic_tag_name(&self, tag: NodeId) -> Result<bool, Error> {
        let read = self.node(tag)?;
        Ok(match read.kind().known() {
            Some(K::Identifier) => {
                tsr_scanner::is_intrinsic_jsx_name(self.node_text(tag)?.as_bytes())
            }
            Some(K::JsxNamespacedName) => true,
            _ => false,
        })
    }

    /// `Node.Text()` of a JSX tag or attribute name: an identifier's text, or
    /// `namespace:name`.
    pub(crate) fn jsx_name_text(&self, node: NodeId) -> Result<JsString, Error> {
        let read = self.node(node)?;
        if read.kind() == K::JsxNamespacedName {
            let data = read
                .data_source()
                .as_jsx_namespaced_name()
                .ok_or(Error::MissingLink("JSX namespace payload"))?;
            let namespace = required(data.namespace(), "JSX namespace")?;
            let name = required(data.name(), "JSX namespace name")?;
            let mut text = self.node_text(namespace)?.as_bytes().to_vec();
            text.push(b':');
            text.extend_from_slice(self.node_text(name)?.as_bytes());
            return Ok(JsString::from_bytes(text));
        }
        Ok(self.node_text(node)?.into_js_string())
    }

    fn jsx_source_text(&self, node: NodeId) -> Result<JsString, Error> {
        Ok(tsr_scanner::get_text_of_node(self.ast(node)?, node)?)
    }

    fn jsx_tag_name(&self, node: NodeId) -> Result<NodeId, Error> {
        required(self.node(node)?.tag_name(), "JSX tag name")
    }

    fn jsx_attributes_node(&self, node: NodeId) -> Result<NodeId, Error> {
        required(self.node(node)?.attributes(), "JSX attributes")
    }

    fn jsx_children(&self, node: NodeId) -> Result<Vec<NodeId>, Error> {
        self.source_list(node, self.node(node)?.children_list())
    }

    // port: tsc/internal/ast/utilities.go:GetSemanticJsxChildren
    fn semantic_jsx_children(&self, children: &[NodeId]) -> Result<Vec<NodeId>, Error> {
        let mut result = Vec::with_capacity(children.len());
        for &child in children {
            let read = self.node(child)?;
            let keep = match read.kind().known() {
                Some(K::JsxExpression) => read.expression().is_some(),
                Some(K::JsxText) => !tsr_ast::utilities_middle::is_whitespace_only_jsx_text(&read),
                _ => true,
            };
            if keep {
                result.push(child);
            }
        }
        Ok(result)
    }

    fn jsx_source_file(&self, node: NodeId) -> Result<NodeId, Error> {
        required(
            tsr_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?,
            "JSX source file",
        )
    }

    /// The `factory` argument of the file's `@jsx` or `@jsxFrag` pragma, when
    /// the pragma is present.
    fn jsx_pragma_factory(&self, file: NodeId, name: &[u8]) -> Result<Option<JsString>, Error> {
        let source = self.source_file_read(file)?;
        let pragmas = source.pragmas()?;
        let pragma = tsr_ast::utilities_middle::get_pragma_from_source_file(pragmas.iter(), name);
        Ok(pragma.map(|pragma| {
            JsString::from_bytes(tsr_ast::utilities_middle::get_pragma_argument(
                Some(pragma),
                &JsString::from_bytes(b"factory".as_slice()),
            ))
        }))
    }

    fn jsx_no_implicit_any(&self) -> Result<bool, Error> {
        let options = self.program()?.host.options();
        Ok(options.strict_option_value(options.no_implicit_any))
    }

    // port: tsc/internal/checker/jsx.go:Checker.checkJsxElement
    pub(crate) fn check_jsx_element(&mut self, node: NodeId) -> Result<TypeId, Error> {
        self.defer_checker_node(node)?;
        self.jsx_element_type_at(node)
    }

    // port: tsc/internal/checker/jsx.go:Checker.checkJsxElementDeferred
    pub(crate) fn check_jsx_element_deferred(&mut self, node: NodeId) -> Result<(), Error> {
        let data = self.node(node)?;
        let element = data
            .data_source()
            .as_jsx_element()
            .ok_or(Error::MissingLink("JSX element payload"))?;
        let opening = required(element.opening_element(), "JSX opening element")?;
        let closing = required(element.closing_element(), "JSX closing element")?;
        self.check_jsx_opening_like_element_or_opening_fragment(opening)?;
        // Perform resolution on the closing tag so that rename/go to definition/etc work
        let tag = self.jsx_tag_name(closing)?;
        if self.is_jsx_intrinsic_tag_name(tag)? {
            self.intrinsic_tag_symbol(closing)?;
        } else {
            self.check_expression(tag)?;
        }
        self.check_jsx_children(node, 0)?;
        Ok(())
    }

    // port: tsc/internal/checker/jsx.go:Checker.checkJsxExpression
    pub(crate) fn check_jsx_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        self.check_grammar_jsx_expression(node)?;
        let read = self.node(node)?;
        let Some(expression) = read.expression() else {
            return Ok(self.builtins.error_type);
        };
        let spread = read
            .data_source()
            .as_jsx_expression()
            .is_some_and(|data| data.dot_dot_dot_token().is_some());
        let ty = self.check_expression_ex(expression, self.expression_mode)?;
        if spread && ty != self.builtins.any_type && !self.is_array_type(ty)? {
            self.error_at(
                Some(node),
                d::JSX_spread_child_must_be_an_array_type,
                vec![],
            )?;
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/jsx.go:Checker.checkJsxSelfClosingElement
    pub(crate) fn check_jsx_self_closing_element(&mut self, node: NodeId) -> Result<TypeId, Error> {
        self.defer_checker_node(node)?;
        self.jsx_element_type_at(node)
    }

    // port: tsc/internal/checker/jsx.go:Checker.checkJsxSelfClosingElementDeferred
    pub(crate) fn check_jsx_self_closing_element_deferred(
        &mut self,
        node: NodeId,
    ) -> Result<(), Error> {
        self.check_jsx_opening_like_element_or_opening_fragment(node)
    }

    // port: tsc/internal/checker/jsx.go:Checker.checkJsxFragment
    pub(crate) fn check_jsx_fragment(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let opening = self
            .node(node)?
            .data_source()
            .as_jsx_fragment()
            .and_then(|data| data.opening_fragment());
        let opening = required(opening, "JSX opening fragment")?;
        self.check_jsx_opening_like_element_or_opening_fragment(opening)?;
        // by default, jsx:'react' will use jsxFactory = React.createElement and jsxFragmentFactory = React.Fragment
        // if jsxFactory compiler option is provided, ensure jsxFragmentFactory compiler option or @jsxFrag pragma is provided too
        let file = self.jsx_source_file(node)?;
        let options = self.program()?.host.options();
        let transform = options.jsx_transform_enabled();
        let factory = !options.jsx_factory.is_empty();
        let fragment_factory = !options.jsx_fragment_factory.is_empty();
        if transform
            && (factory || self.jsx_pragma_factory(file, b"jsx")?.is_some())
            && !fragment_factory
            && self.jsx_pragma_factory(file, b"jsxfrag")?.is_none()
        {
            let message = if factory {
                d::The_jsxFragmentFactory_compiler_option_must_be_provided_to_use_JSX_fragments_with_the_jsxFactory_compiler_option
            } else {
                d::An_jsxFrag_pragma_is_required_when_using_an_jsx_pragma_with_JSX_fragments
            };
            self.error_at(Some(node), message, vec![])?;
        }
        self.check_jsx_children(node, 0)?;
        let ty = self.jsx_element_type_at(node)?;
        Ok(if self.is_error_type(ty)? {
            self.builtins.any_type
        } else {
            ty
        })
    }

    // port: tsc/internal/checker/jsx.go:Checker.checkJsxAttributes
    pub(crate) fn check_jsx_attributes(&mut self, node: NodeId) -> Result<TypeId, Error> {
        self.defer_checker_node(node)?;
        let parent = required(self.node(node)?.parent(), "JSX attributes parent")?;
        self.create_jsx_attributes_type(parent, self.expression_mode)
    }

    // port: tsc/internal/checker/jsx.go:Checker.checkJsxOpeningLikeElementOrOpeningFragment
    fn check_jsx_opening_like_element_or_opening_fragment(
        &mut self,
        node: NodeId,
    ) -> Result<(), Error> {
        let opening_like =
            tsr_ast::utilities_middle::is_jsx_opening_like_element(&self.node(node)?);
        if opening_like {
            self.check_grammar_jsx_element(node)?;
        }
        self.check_jsx_preconditions(node)?;
        self.mark_jsx_alias_referenced(node)?;
        let signature = self.resolved_call_signature(node)?;
        self.check_deprecated_signature(signature, node)?;
        if opening_like {
            if let Some(constraint) = self.jsx_element_type_type_at(node)? {
                let tag = self.jsx_tag_name(node)?;
                let tag_type = if self.is_jsx_intrinsic_tag_name(tag)? {
                    let text = self.jsx_name_text(tag)?;
                    self.get_string_literal_type(text)?
                } else {
                    self.check_expression(tag)?
                };
                let (related, diagnostic) = self.check_type_related_ex(
                    tag_type,
                    constraint,
                    RelationKind::Assignable,
                    Some(tag),
                    Some(d::Its_type_0_is_not_a_valid_JSX_element_type),
                )?;
                if !related {
                    if let Some(diagnostic) = diagnostic {
                        let text = self.jsx_source_text(tag)?;
                        self.add_diagnostic(Diagnostic::chain(
                            Some(Arc::new(diagnostic)),
                            d::X_0_cannot_be_used_as_a_JSX_component,
                            vec![text],
                        ))?;
                    }
                }
            } else {
                let kind = self.jsx_reference_kind(node)?;
                let returned = self.return_type_of_signature(signature)?;
                self.check_jsx_return_assignable_to_appropriate_bound(kind, returned, node)?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/jsx.go:Checker.checkJsxPreconditions
    fn check_jsx_preconditions(&mut self, node: NodeId) -> Result<(), Error> {
        // Preconditions for using JSX
        if self.program()?.host.options().jsx == tsr_core::JsxEmit::NONE {
            self.error_at(
                Some(node),
                d::Cannot_use_JSX_unless_the_jsx_flag_is_provided,
                vec![],
            )?;
        }
        // getJsxType answers errorType rather than nil, so the pin never
        // reports the implicit-any JSX.Element error here; the lookup runs.
        if self.jsx_no_implicit_any()? {
            self.jsx_element_type_at(node)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/jsx.go:Checker.checkJsxReturnAssignableToAppropriateBound
    fn check_jsx_return_assignable_to_appropriate_bound(
        &mut self,
        kind: JsxReferenceKind,
        instance: TypeId,
        node: NodeId,
    ) -> Result<(), Error> {
        let tag = self.jsx_tag_name(node)?;
        let (constraint, message) = match kind {
            JsxReferenceKind::Function => (
                Some(self.jsx_stateless_element_type_at(node)?),
                d::Its_return_type_0_is_not_a_valid_JSX_element,
            ),
            JsxReferenceKind::Component => (
                self.jsx_element_class_type_at(node)?,
                d::Its_instance_type_0_is_not_a_valid_JSX_element,
            ),
            JsxReferenceKind::Mixed => {
                let stateless = self.jsx_stateless_element_type_at(node)?;
                let Some(class) = self.jsx_element_class_type_at(node)? else {
                    return Ok(());
                };
                (
                    Some(self.get_union_type(&[stateless, class])?),
                    d::Its_element_type_0_is_not_a_valid_JSX_element,
                )
            }
        };
        let Some(constraint) = constraint else {
            return Ok(());
        };
        let (_, diagnostic) = self.check_type_related_ex(
            instance,
            constraint,
            RelationKind::Assignable,
            Some(tag),
            Some(message),
        )?;
        if let Some(diagnostic) = diagnostic {
            let text = self.jsx_source_text(tag)?;
            self.add_diagnostic(Diagnostic::chain(
                Some(Arc::new(diagnostic)),
                d::X_0_cannot_be_used_as_a_JSX_component,
                vec![text],
            ))?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/jsx.go:Checker.inferJsxTypeArguments
    pub(crate) fn infer_jsx_type_arguments(
        &mut self,
        node: NodeId,
        signature: SignatureId,
        mode: u32,
        context: InferenceId,
    ) -> Result<Vec<TypeId>, Error> {
        let parameter = self.effective_first_argument_for_jsx_signature(signature, node)?;
        let attributes = self.jsx_attributes_node(node)?;
        let checked = self.check_call_argument_ex(attributes, parameter, Some(context), mode)?;
        self.infer_types(context, checked, parameter, priority::NONE, false)?;
        let count = self
            .signatures
            .get(signature)?
            .type_parameters
            .as_ref()
            .map_or(0, |parameters| parameters.len());
        let mut inferred = Vec::with_capacity(count);
        for index in 0..count {
            inferred.push(self.inferred_type(context, index)?);
        }
        Ok(inferred)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getContextualTypeForJsxExpression
    pub(crate) fn contextual_type_for_jsx_expression(
        &mut self,
        node: NodeId,
        flags: u32,
    ) -> Result<Option<TypeId>, Error> {
        let parent = required(self.node(node)?.parent(), "JSX expression parent")?;
        let parent_read = self.node(parent)?;
        if tsr_ast::utilities::is_jsx_attribute_like(&parent_read) {
            return self.contextual_expression_type_ex(node, flags);
        }
        if parent_read.kind() == K::JsxElement {
            return self.contextual_type_for_child_jsx_expression(parent, node, flags);
        }
        Ok(None)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getContextualTypeForJsxAttribute
    pub(crate) fn contextual_type_for_jsx_attribute(
        &mut self,
        attribute: NodeId,
        flags: u32,
    ) -> Result<Option<TypeId>, Error> {
        // When we trying to resolve JsxOpeningLikeElement as a stateless function element, we will already give its attributes a contextual type
        // which is a type of the parameter of the signature we are trying out.
        // If there is no contextual type (e.g. we are trying to resolve stateful component), get attributes type from resolving element's tagName
        let read = self.node(attribute)?;
        let parent = required(read.parent(), "JSX attribute parent")?;
        if read.kind() == K::JsxAttribute {
            let name = required(read.name(), "JSX attribute name")?;
            let Some(attributes) = self.apparent_contextual_expression_type_ex(parent, flags)?
            else {
                return Ok(None);
            };
            if self.types.flags(attributes)? & tf::ANY != 0 {
                return Ok(None);
            }
            let text = self.jsx_name_text(name)?;
            return self.type_of_property_of_contextual_type(attributes, text.as_bytes());
        }
        self.contextual_expression_type_ex(parent, flags)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getContextualJsxElementAttributesType
    pub(crate) fn contextual_jsx_element_attributes_type(
        &mut self,
        node: NodeId,
        flags: u32,
    ) -> Result<Option<TypeId>, Error> {
        // ContextFlagsIgnoreNodeInferences
        if self.node(node)?.kind() == K::JsxOpeningElement && flags != 4 {
            let element = required(self.node(node)?.parent(), "JSX opening element parent")?;
            if let Some(context) = self.contextual_call_argument_ex(element, flags == 0) {
                // Contextually applied type is moved from attributes up to the outer jsx attributes so when walking up from the children they get hit
                // _However_ to hit them from the _attributes_ we must look for them here; otherwise we'll used the declared type
                // (as below) instead!
                return Ok(context.ty);
            }
        }
        self.contextual_call_argument_at(node, 0)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getContextualTypeForChildJsxExpression
    fn contextual_type_for_child_jsx_expression(
        &mut self,
        node: NodeId,
        child: NodeId,
        flags: u32,
    ) -> Result<Option<TypeId>, Error> {
        let opening = self
            .node(node)?
            .data_source()
            .as_jsx_element()
            .and_then(|data| data.opening_element());
        let attributes = self.jsx_attributes_node(required(opening, "JSX opening element")?)?;
        let attributes_type = self.apparent_contextual_expression_type_ex(attributes, flags)?;
        // JSX expression is in children of JSX Element, we will look for an "children" attribute (we get the name from JSX.ElementAttributesProperty)
        let name = self.jsx_element_children_property_name(node)?;
        let (Some(attributes_type), Some(name)) = (attributes_type, name) else {
            return Ok(None);
        };
        if self.types.flags(attributes_type)? & tf::ANY != 0 || name.is_empty() {
            return Ok(None);
        }
        let children = self.jsx_children(node)?;
        let real = self.semantic_jsx_children(&children)?;
        let index = real.iter().position(|&node| node == child);
        let Some(field) =
            self.type_of_property_of_contextual_type(attributes_type, name.as_bytes())?
        else {
            return Ok(None);
        };
        if real.len() == 1 {
            return Ok(Some(field));
        }
        #[allow(clippy::cast_precision_loss, reason = "child indexes are small")]
        let index = index.map_or(-1.0, |index| index as f64);
        self.map_type_ex(
            field,
            &mut |checker, ty| {
                if checker.is_array_like_type(ty)? {
                    let key = checker.get_number_literal_type(tsr_jsnum::Number::new(index))?;
                    checker
                        .get_indexed_access_type(ty, key, 0, None, None)
                        .map(Some)
                } else {
                    Ok(Some(ty))
                }
            },
            true,
        )
    }

    // port: tsc/internal/checker/jsx.go:Checker.discriminateContextualTypeByJSXAttributes
    pub(crate) fn discriminate_jsx_attributes_context(
        &mut self,
        node: NodeId,
        context: TypeId,
    ) -> Result<TypeId, Error> {
        if let Some(&ty) = self.bindings.discriminated_contexts.get(&(node, context)) {
            return Ok(ty);
        }
        let children_name = self.jsx_element_children_property_name(node)?;
        let mut items = Vec::new();
        for property in self.source_list(node, self.node(node)?.property_list())? {
            let Some(symbol) = self.raw_declaration_symbol(property)? else {
                continue;
            };
            let read = self.node(property)?;
            if read.kind() != K::JsxAttribute {
                continue;
            }
            let initializer = read.initializer();
            if let Some(initializer) = initializer {
                if !self.possible_object_discriminant(initializer)? {
                    continue;
                }
            }
            let name = self.symbol(symbol)?.name_to_owned();
            if self.discriminant_property(context, name.as_bytes())? {
                items.push((
                    name,
                    match initializer {
                        Some(initializer) => {
                            crate::object_discriminants::Discriminant::Expression(initializer)
                        }
                        // JsxAttribute without initializer is always true
                        None => crate::object_discriminants::Discriminant::True,
                    },
                ));
            }
        }
        let own = self.raw_declaration_symbol(node)?;
        let element = match self.node(node)?.parent() {
            Some(parent) => self.node(parent)?.parent(),
            None => None,
        };
        for property in self.get_properties_of_type(context)? {
            let read = self.symbol(property)?;
            if read.flags() & sf::OPTIONAL == 0 {
                continue;
            }
            let Some(own) = own else { continue };
            let name = read.name_to_owned();
            if let (Some(children_name), Some(element)) = (&children_name, element) {
                if name == *children_name
                    && self.node(element)?.kind() == K::JsxElement
                    && !self
                        .semantic_jsx_children(&self.jsx_children(element)?)?
                        .is_empty()
                {
                    continue;
                }
            }
            let members = self.symbol(own)?.members();
            if self.member_symbol(members, name.as_bytes())?.is_none()
                && self.discriminant_property(context, name.as_bytes())?
            {
                items.push((name, crate::object_discriminants::Discriminant::Undefined));
            }
        }
        let ty = self.discriminate_object_items(context, &items)?;
        self.bindings
            .discriminated_contexts
            .insert((node, context), ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/jsx.go:Checker.elaborateJsxComponents
    pub(crate) fn elaborate_jsx_components(
        &mut self,
        node: NodeId,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        output: &mut Vec<Diagnostic>,
    ) -> Result<bool, Error> {
        let mut reported = false;
        for property in self.source_list(node, self.node(node)?.property_list())? {
            let read = self.node(property)?;
            if read.kind() == K::JsxSpreadAttribute {
                continue;
            }
            let name = required(read.name(), "elaborated JSX attribute name")?;
            let initializer = read.initializer();
            let text = self.jsx_name_text(name)?;
            if is_hyphenated_jsx_name(text.as_bytes()) {
                continue;
            }
            let name_type = self.get_string_literal_type(text)?;
            if self.types.flags(name_type)? & tf::NEVER == 0 {
                reported = self.elaborate_element_error(
                    source,
                    target,
                    relation,
                    name,
                    initializer,
                    name_type,
                    None,
                    None,
                    output,
                )? || reported;
            }
        }
        let parent = required(self.node(node)?.parent(), "JSX attributes parent")?;
        let containing = self.node(parent)?.parent();
        let Some(containing) = containing.filter(|_| {
            self.node(parent)
                .is_ok_and(|read| read.kind() == K::JsxOpeningElement)
        }) else {
            return Ok(reported);
        };
        if self.node(containing)?.kind() != K::JsxElement {
            return Ok(reported);
        }
        let property = self
            .jsx_element_children_property_name(node)?
            .unwrap_or_else(|| JsString::from_bytes(b"children".as_slice()));
        let children_name_type = self.get_string_literal_type(property.clone())?;
        let children_target =
            self.get_indexed_access_type(target, children_name_type, 0, None, None)?;
        let children = self.jsx_children(containing)?;
        let valid = self.semantic_jsx_children(&children)?;
        if valid.is_empty() {
            return Ok(reported);
        }
        let more_than_one = valid.len() > 1;
        let iterable = self.iteration_global("Iterable", 3)?;
        let (array_like, non_array_like) = if iterable == self.builtins.empty_generic_type {
            let array_like = self.filter_type(children_target, &mut |checker, part| {
                checker.is_array_or_tuple_like_type(part)
            })?;
            let non_array_like = self.filter_type(children_target, &mut |checker, part| {
                Ok(!checker.is_array_or_tuple_like_type(part)?)
            })?;
            (array_like, non_array_like)
        } else {
            let any_iterable = self.create_iterable_type(self.builtins.any_type)?;
            let array_like = self.filter_type(children_target, &mut |checker, part| {
                checker.is_type_related_to(part, any_iterable, RelationKind::Assignable)
            })?;
            let non_array_like = self.filter_type(children_target, &mut |checker, part| {
                Ok(!checker.is_type_related_to(part, any_iterable, RelationKind::Assignable)?)
            })?;
            (array_like, non_array_like)
        };
        let tag_name = self.jsx_tag_name(parent)?;
        let mut text = JsxTextChildMessage {
            tag_name,
            property: property.clone(),
            target: children_target,
            args: None,
        };
        let opening_tag = {
            let opening = self
                .node(containing)?
                .data_source()
                .as_jsx_element()
                .and_then(|data| data.opening_element());
            self.jsx_tag_name(required(opening, "JSX opening element")?)?
        };
        if more_than_one {
            if array_like == self.builtins.never_type {
                let source_children =
                    self.get_indexed_access_type(source, children_name_type, 0, None, None)?;
                if !self.is_type_related_to(source_children, children_target, relation)? {
                    // arity mismatch
                    let target_text =
                        self.type_to_string(children_target, crate::type_display::DEFAULT_FLAGS)?;
                    let diagnostic = self.diagnostic_for_node(
                        Some(opening_tag),
                        d::This_JSX_tag_s_0_prop_expects_a_single_child_of_type_1_but_multiple_children_were_provided,
                        vec![property, target_text],
                    )?;
                    self.add_diagnostic(diagnostic.clone())?;
                    output.push(diagnostic);
                    reported = true;
                }
            } else {
                let types = self.check_jsx_children(containing, 0)?;
                let real_source = self.create_tuple_type(&types)?;
                reported = self.elaborate_jsx_children_elementwise(
                    &children,
                    real_source,
                    array_like,
                    relation,
                    &mut text,
                    output,
                )? || reported;
            }
        } else if non_array_like != self.builtins.never_type {
            let child = valid[0];
            if let Some(element) = self.jsx_child_elaboration_element(child, children_name_type)? {
                reported = self.elaborate_element_error(
                    source,
                    target,
                    relation,
                    element.error_node,
                    element.inner_expression,
                    element.name_type,
                    None,
                    element.text.then_some(&mut text),
                    output,
                )? || reported;
            }
        } else {
            let source_children =
                self.get_indexed_access_type(source, children_name_type, 0, None, None)?;
            if !self.is_type_related_to(source_children, children_target, relation)? {
                // arity mismatch
                let target_text =
                    self.type_to_string(children_target, crate::type_display::DEFAULT_FLAGS)?;
                let diagnostic = self.diagnostic_for_node(
                    Some(opening_tag),
                    d::This_JSX_tag_s_0_prop_expects_type_1_which_requires_multiple_children_but_only_a_single_child_was_provided,
                    vec![property, target_text],
                )?;
                self.add_diagnostic(diagnostic.clone())?;
                output.push(diagnostic);
                reported = true;
            }
        }
        Ok(reported)
    }

    /// The diagnostic `getInvalidTextualChildDiagnostic` describes, at `node`.
    pub(crate) fn jsx_text_child_diagnostic(
        &mut self,
        message: &mut JsxTextChildMessage,
        node: NodeId,
    ) -> Result<Diagnostic, Error> {
        if message.args.is_none() {
            let tag = self.jsx_source_text(message.tag_name)?;
            let target = self.type_to_string(message.target, crate::type_display::DEFAULT_FLAGS)?;
            message.args = Some(vec![tag, message.property.clone(), target]);
        }
        self.diagnostic_for_node(
            Some(node),
            d::X_0_components_don_t_accept_text_as_child_elements_Text_in_JSX_has_the_type_string_but_the_expected_type_of_1_is_2,
            message.args.clone().unwrap_or_default(),
        )
    }

    // port: tsc/internal/checker/jsx.go:Checker.getElaborationElementForJsxChild
    fn jsx_child_elaboration_element(
        &self,
        child: NodeId,
        name_type: TypeId,
    ) -> Result<Option<JsxElaborationElement>, Error> {
        let read = self.node(child)?;
        Ok(match read.kind().known() {
            // child is of the type of the expression
            Some(K::JsxExpression) => Some(JsxElaborationElement {
                error_node: child,
                inner_expression: read.expression(),
                name_type,
                text: false,
            }),
            Some(K::JsxText) => {
                if tsr_ast::utilities_middle::is_whitespace_only_jsx_text(&read) {
                    // Whitespace only jsx text isn't real jsx text
                    None
                } else {
                    // child is a string
                    Some(JsxElaborationElement {
                        error_node: child,
                        inner_expression: None,
                        name_type,
                        text: true,
                    })
                }
            }
            // child is of type JSX.Element
            Some(K::JsxElement | K::JsxSelfClosingElement | K::JsxFragment) => {
                Some(JsxElaborationElement {
                    error_node: child,
                    inner_expression: Some(child),
                    name_type,
                    text: false,
                })
            }
            _ => {
                return Err(Error::Unsupported(
                    "Unhandled case in getElaborationElementForJsxChild",
                ))
            }
        })
    }

    // port: tsc/internal/checker/jsx.go:Checker.generateJsxChildren
    // port: tsc/internal/checker/jsx.go:Checker.elaborateIterableOrArrayLikeTargetElementwise
    fn elaborate_jsx_children_elementwise(
        &mut self,
        children: &[NodeId],
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        text: &mut JsxTextChildMessage,
        output: &mut Vec<Diagnostic>,
    ) -> Result<bool, Error> {
        let tuple_parts = self.filter_type(target, &mut |checker, part| {
            checker.is_array_or_tuple_like_type(part)
        })?;
        let other_parts = self.filter_type(target, &mut |checker, part| {
            Ok(!checker.is_array_or_tuple_like_type(part)?)
        })?;
        // If `nonTupleOrArrayLikeTargetParts` is not `never`, then that should mean `Iterable` is defined.
        let iteration_type = if other_parts != self.builtins.never_type
            && self.types.flags(other_parts)? & tf::ANY == 0
        {
            self.iteration_types_of_iterable(other_parts, crate::iteration::FOR_OF, None)?
                .yield_type
        } else {
            None
        };
        let mut reported = false;
        // The index of a child among the children that are elements: whitespace
        // text does not count (`memberOffset` upstream).
        let mut position = 0_u32;
        for &child in children {
            let name_type =
                self.get_number_literal_type(tsr_jsnum::Number::new(f64::from(position)))?;
            let Some(element) = self.jsx_child_elaboration_element(child, name_type)? else {
                continue;
            };
            position += 1;
            let property = element.error_node;
            let next = element.inner_expression;
            let mut target_type = iteration_type;
            let indexed = if tuple_parts == self.builtins.never_type {
                None
            } else {
                self.best_match_indexed_access(source, tuple_parts, name_type)?
            };
            if let Some(indexed) = indexed {
                if self.types.flags(indexed)? & tf::INDEXED_ACCESS == 0 {
                    target_type = Some(match iteration_type {
                        Some(iteration) => self.get_union_type(&[iteration, indexed])?,
                        None => indexed,
                    });
                }
            }
            let Some(mut target_type) = target_type else {
                continue;
            };
            let Some(mut source_type) =
                self.indexed_access_or_undefined(source, name_type, 0, None, None)?
            else {
                continue;
            };
            let name = self.index_property_name(name_type)?;
            if self.is_type_related_to(source_type, target_type, relation)? {
                continue;
            }
            let elaborated = match next {
                Some(next) => self.elaborate_call_error(
                    next,
                    source_type,
                    target_type,
                    relation,
                    None,
                    output,
                )?,
                None => false,
            };
            reported = true;
            if elaborated {
                continue;
            }
            // Issue error on the prop itself, since the prop couldn't elaborate the error. Use the expression type, if available.
            let specific = match next {
                Some(next) => self.elaborate_mutable_expression(next, source_type)?,
                None => source_type,
            };
            if element.text {
                // Use the custom diagnostic factory if provided (e.g., for JSX text children with dynamic error messages)
                output.push(self.jsx_text_child_diagnostic(text, property)?);
            } else if self.options.exact_optional_property_types
                && self.maybe_type_of_kind(specific, tf::UNDEFINED)?
                && self.type_contains_missing(target_type)?
            {
                let specific = self.type_to_string(specific, crate::type_display::DEFAULT_FLAGS)?;
                let target =
                    self.type_to_string(target_type, crate::type_display::DEFAULT_FLAGS)?;
                output.push(self.diagnostic_for_node(Some(property), d::Type_0_is_not_assignable_to_type_1_with_exactOptionalPropertyTypes_Colon_true_Consider_adding_undefined_to_the_type_of_the_target, vec![specific, target])?);
            } else {
                let optional = |checker: &mut Self, ty: TypeId| -> Result<bool, Error> {
                    let Some(name) = &name else { return Ok(false) };
                    Ok(
                        match checker.constituent_property(ty, name.as_bytes(), false)? {
                            Some(symbol) => checker.symbol(symbol)?.flags() & sf::OPTIONAL != 0,
                            None => false,
                        },
                    )
                };
                let target_optional = optional(self, tuple_parts)?;
                let source_optional = optional(self, source)?;
                target_type = self.remove_missing_type(target_type, target_optional)?;
                source_type =
                    self.remove_missing_type(source_type, target_optional && source_optional)?;
                let (related, diagnostic) = self.check_type_related_ex(
                    specific,
                    target_type,
                    relation,
                    Some(property),
                    None,
                )?;
                if let Some(diagnostic) = diagnostic {
                    output.push(diagnostic);
                }
                if related && specific != source_type {
                    // If for whatever reason the expression type doesn't yield an error, make sure we still issue an error on the sourcePropType
                    let (_, diagnostic) = self.check_type_related_ex(
                        source_type,
                        target_type,
                        relation,
                        Some(property),
                        None,
                    )?;
                    if let Some(diagnostic) = diagnostic {
                        output.push(diagnostic);
                    }
                }
            }
        }
        Ok(reported)
    }

    // port: tsc/internal/checker/checker.go:Checker.isArrayOrTupleLikeType
    fn is_array_or_tuple_like_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        Ok(self.is_array_like_type(ty)? || self.tuple_like_type(ty)?)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getSuggestedSymbolForNonexistentJSXAttribute
    pub(crate) fn suggested_symbol_for_nonexistent_jsx_attribute(
        &mut self,
        name: &[u8],
        containing: TypeId,
    ) -> Result<Option<SymbolId>, Error> {
        let properties = self.get_properties_of_type(containing)?;
        let specific: Option<&[u8]> = match name {
            b"for" => Some(b"htmlFor"),
            b"class" => Some(b"className"),
            _ => None,
        };
        if let Some(specific) = specific {
            for &property in &properties {
                if self.ast_symbol_name(property)?.as_bytes() == specific {
                    return Ok(Some(property));
                }
            }
        }
        self.spelling_suggestion_for_name(name, &properties, sf::VALUE)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJSXFragmentType
    fn jsx_fragment_type(&mut self, node: NodeId) -> Result<TypeId, Error> {
        // An opening fragment is required in order for `getJsxNamespace` to give the fragment factory
        let file = self.jsx_source_file(node)?;
        if let Some(ty) = self
            .jsx
            .files
            .get(&file)
            .and_then(|links| links.fragment_type)
        {
            return Ok(ty);
        }
        let factory_name = self.jsx_namespace(Some(node))?;
        let options = self.program()?.host.options();
        // #38720/60122, allow null as jsxFragmentFactory
        let resolve = (options.jsx == tsr_core::JsxEmit::REACT
            || !options.jsx_fragment_factory.is_empty())
            && factory_name.as_bytes() != b"null";
        let module_error = options.jsx != tsr_core::JsxEmit::PRESERVE
            && options.jsx != tsr_core::JsxEmit::REACT_NATIVE;
        let ty = if resolve {
            let mut symbol = self.jsx_namespace_container_for_implicit_import(node)?;
            if symbol.is_none() {
                let mut flags = sf::VALUE;
                if !module_error {
                    flags &= !sf::ENUM;
                }
                symbol = self.resolve_name_ex(
                    Some(node),
                    factory_name.as_bytes(),
                    flags,
                    Some(d::Using_JSX_fragments_requires_fragment_factory_0_to_be_in_scope_but_it_could_not_be_found),
                    true,
                    false,
                )?;
            }
            match symbol {
                None => self.builtins.error_type,
                Some(symbol) if self.symbol(symbol)?.name_bytes() == b"Fragment" => {
                    self.get_type_of_symbol(symbol)?
                }
                Some(symbol) => {
                    let resolved = if self.symbol(symbol)?.flags() & sf::ALIAS != 0 {
                        self.resolve_alias(symbol)?
                    } else {
                        symbol
                    };
                    let exports = self.module_exports_of_symbol(resolved)?;
                    match self.lookup_symbol_resolving(
                        exports,
                        b"Fragment",
                        sf::BLOCK_SCOPED_VARIABLE,
                    )? {
                        Some(symbol) => self.get_type_of_symbol(symbol)?,
                        None => self.builtins.error_type,
                    }
                }
            }
        } else {
            self.builtins.any_type
        };
        self.jsx.files.entry(file).or_default().fragment_type = Some(ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/jsx.go:Checker.resolveJsxOpeningLikeElement
    pub(crate) fn resolve_jsx_opening_like_element(
        &mut self,
        node: NodeId,
    ) -> Result<SignatureId, Error> {
        let fragment = self.node(node)?.kind() == K::JsxOpeningFragment;
        let expression_type = if fragment {
            self.jsx_fragment_type(node)?
        } else {
            let tag = self.jsx_tag_name(node)?;
            if self.is_jsx_intrinsic_tag_name(tag)? {
                let result = self.intrinsic_attributes_type_from_jsx_opening_like_element(node)?;
                let fake = self.create_signature_for_jsx_intrinsic(node, result)?;
                let attributes = self.jsx_attributes_node(node)?;
                let parameter = self.effective_first_argument_for_jsx_signature(fake, node)?;
                let checked = self.check_call_argument_ex(attributes, parameter, None, 0)?;
                self.check_expression_related_with_elaboration(
                    checked,
                    result,
                    RelationKind::Assignable,
                    Some(tag),
                    Some(attributes),
                    None,
                )?;
                let type_arguments =
                    self.source_list(node, self.node(node)?.type_argument_list())?;
                if !type_arguments.is_empty() {
                    for &argument in &type_arguments {
                        self.check_source_element(argument)?;
                    }
                    let view = self.ast(node)?;
                    let file = self.jsx_source_file(node)?;
                    let list =
                        required(view.node(node)?.type_argument_list(), "JSX type arguments")?;
                    let loc = view.list(list)?.loc();
                    let start = tsr_scanner::skip_trivia(
                        view.source_file(file)?.text().as_bytes(),
                        loc.pos(),
                    );
                    self.add_diagnostic(Diagnostic::new(
                        Some(file),
                        tsr_core::TextRange::new(start, loc.end()),
                        d::Expected_0_type_arguments_but_got_1,
                        vec![
                            JsString::from_bytes(b"0".as_slice()),
                            JsString::from_bytes(type_arguments.len().to_string().as_bytes()),
                        ],
                    ))?;
                }
                return Ok(fake);
            }
            self.check_expression(tag)?
        };
        let apparent = self.apparent_type(expression_type)?;
        if self.is_error_type(apparent)? {
            return self.resolve_error_call(node);
        }
        let signatures = self.uninstantiated_jsx_signatures_of_type(expression_type, node)?;
        if self.is_untyped_function_call(expression_type, apparent, signatures.len(), 0)? {
            return self.resolve_untyped_call(node);
        }
        if signatures.is_empty() {
            // We found no signatures at all, which is an error
            let error_node = if fragment {
                node
            } else {
                self.jsx_tag_name(node)?
            };
            let text = self.jsx_source_text(error_node)?;
            self.error_at(
                Some(error_node),
                d::JSX_element_type_0_does_not_have_any_construct_or_call_signatures,
                vec![text],
            )?;
            return self.resolve_error_call(node);
        }
        self.resolve_typed_call(node, &signatures)
    }

    // port: tsc/internal/checker/jsx.go:Checker.checkApplicableSignatureForJsxCallLikeElement
    #[allow(
        clippy::too_many_arguments,
        reason = "Parameters preserve the upstream operation and its independently selected checking modes"
    )]
    pub(crate) fn check_applicable_signature_for_jsx_call_like_element(
        &mut self,
        node: NodeId,
        signature: SignatureId,
        relation: RelationKind,
        mode: u32,
        report: bool,
        output: &mut Vec<Diagnostic>,
    ) -> Result<bool, Error> {
        // Stateless function components can have maximum of three arguments: "props", "context", and "updater".
        // However "context" and "updater" are implicit and can't be specify by users. Only the first parameter, props,
        // can be specified by users through attributes property.
        let parameter = self.effective_first_argument_for_jsx_signature(signature, node)?;
        let fragment = self.node(node)?.kind() == K::JsxOpeningFragment;
        let attributes = if fragment {
            None
        } else {
            Some(self.jsx_attributes_node(node)?)
        };
        let attributes_type = match attributes {
            None => self.create_jsx_attributes_type(node, 0)?,
            Some(attributes) => self.check_call_argument_ex(attributes, parameter, None, mode)?,
        };
        let check_attributes_type = if mode & 4 != 0 {
            self.regular_object_literal_type(attributes_type)?
        } else {
            attributes_type
        };
        if !self.check_tag_name_does_not_expect_too_many_arguments(node, report, output)? {
            return Ok(false);
        }
        let error_node = if report {
            Some(if fragment {
                node
            } else {
                self.jsx_tag_name(node)?
            })
        } else {
            None
        };
        self.collect_expression_relation_errors(
            check_attributes_type,
            parameter,
            relation,
            error_node,
            attributes,
            None,
            output,
        )
    }

    /// `checkTagNameDoesNotExpectTooManyArguments` inside
    /// `checkApplicableSignatureForJsxCallLikeElement`.
    fn check_tag_name_does_not_expect_too_many_arguments(
        &mut self,
        node: NodeId,
        report: bool,
        output: &mut Vec<Diagnostic>,
    ) -> Result<bool, Error> {
        if self
            .jsx_namespace_container_for_implicit_import(node)?
            .is_some()
        {
            return Ok(true); // factory is implicitly jsx/jsxdev - assume it fits the bill, since we don't strongly look for the jsx/jsxs/jsxDEV factory APIs anywhere else (at least not yet)
        }
        // We assume fragments have the correct arity since the node does not have attributes
        let kind = self.node(node)?.kind();
        let tag_type = if matches!(
            kind.known(),
            Some(K::JsxOpeningElement | K::JsxSelfClosingElement)
        ) {
            let tag = self.jsx_tag_name(node)?;
            if self.is_jsx_intrinsic_tag_name(tag)?
                || self.node(tag)?.kind() == K::JsxNamespacedName
            {
                None
            } else {
                Some(self.check_expression(tag)?)
            }
        } else {
            None
        };
        let Some(tag_type) = tag_type else {
            return Ok(true);
        };
        let tag_signatures = self.signatures_of_type(tag_type, false)?;
        if tag_signatures.is_empty() {
            return Ok(true);
        }
        let Some(factory) = self.jsx_factory_entity(Some(node))? else {
            return Ok(true);
        };
        let Some(factory_symbol) =
            self.resolve_entity_name_at(factory, sf::VALUE, true, false, Some(node))?
        else {
            return Ok(true);
        };
        let factory_type = self.get_type_of_symbol(factory_symbol)?;
        let factory_signatures = self.signatures_of_type(factory_type, false)?;
        if factory_signatures.is_empty() {
            return Ok(true);
        }
        let mut has_first_parameter_signatures = false;
        let mut maximum = 0;
        // Check that _some_ first parameter expects a FC-like thing, and that some overload of the SFC expects an acceptable number of arguments
        for signature in factory_signatures {
            let first = self
                .parameter_type_at(signature, 0)?
                .unwrap_or(self.builtins.any_type);
            let parameter_signatures = self.signatures_of_type(first, false)?;
            for parameter_signature in parameter_signatures {
                has_first_parameter_signatures = true;
                if self.effective_rest_parameter(parameter_signature)? {
                    return Ok(true); // some signature has a rest param, so function components can have an arbitrary number of arguments
                }
                maximum = maximum.max(self.parameter_count(parameter_signature)?);
            }
        }
        if !has_first_parameter_signatures {
            // Not a single signature had a first parameter which expected a signature - for back compat, and
            // to guard against generic factories which won't have signatures directly, do not error
            return Ok(true);
        }
        let mut minimum = usize::MAX;
        for signature in tag_signatures {
            minimum = minimum.min(self.min_argument_count(signature)?);
        }
        if minimum <= maximum {
            return Ok(true); // some signature accepts the number of arguments the function component provides
        }
        if report {
            let tag = self.jsx_tag_name(node)?;
            // We will not report errors in this function for fragments, since we do not check them in this function
            let tag_text = self.jsx_entity_name_text(tag)?;
            let factory_text = self.jsx_entity_name_text(factory)?;
            let mut diagnostic = self.diagnostic_for_node(
                Some(tag),
                d::Tag_0_expects_at_least_1_arguments_but_the_JSX_factory_2_provides_at_most_3,
                vec![
                    tag_text.clone(),
                    JsString::from_bytes(minimum.to_string().as_bytes()),
                    factory_text,
                    JsString::from_bytes(maximum.to_string().as_bytes()),
                ],
            )?;
            if let Some(symbol) = self.get_symbol_at_location(tag)? {
                if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
                    diagnostic
                        .related_information
                        .push(Arc::new(self.diagnostic_for_node(
                            Some(declaration),
                            d::X_0_is_declared_here,
                            vec![tag_text],
                        )?));
                }
            }
            output.push(diagnostic);
        }
        Ok(false)
    }

    /// `entityNameToString` of a tag name or a factory entity.
    fn jsx_entity_name_text(&self, node: NodeId) -> Result<JsString, Error> {
        let view = self.ast(node)?;
        Ok(JsString::from_bytes(
            tsr_ast::utilities_targets::entity_name_to_string(view, node, None)?,
        ))
    }

    // port: tsc/internal/checker/jsx.go:Checker.createJsxAttributesTypeFromAttributesProperty
    pub(crate) fn create_jsx_attributes_type(
        &mut self,
        opening: NodeId,
        mode: u32,
    ) -> Result<TypeId, Error> {
        let mut all = self.options.strict_null_checks.then(SymbolTable::default);
        let mut table = SymbolTable::default();
        let mut attributes_symbol = None;
        let mut attribute_parent = opening;
        let empty = self.builtins.empty_jsx_object_type;
        let mut spread = empty;
        let mut has_spread_any = false;
        let mut intersect = None;
        let mut explicit_children = false;
        let mut object_flags = of::JSX_ATTRIBUTES;
        let children_name = self.jsx_element_children_property_name(opening)?;
        let fragment = self.node(opening)?.kind() == K::JsxOpeningFragment;
        if !fragment {
            let attributes = self.jsx_attributes_node(opening)?;
            attributes_symbol = self.raw_declaration_symbol(attributes)?;
            attribute_parent = attributes;
            let contextual = self.contextual_expression_type_ex(attributes, 0)?;
            // Create anonymous type from given attributes symbol table.
            for declaration in
                self.source_list(attributes, self.node(attributes)?.property_list())?
            {
                let member = self.raw_declaration_symbol(declaration)?;
                if self.node(declaration)?.kind() == K::JsxAttribute {
                    let member = required(member, "JSX attribute symbol")?;
                    let ty = self.check_jsx_attribute(declaration, mode)?;
                    object_flags |= self.types.get(ty)?.object_flags & of::PROPAGATING_FLAGS;
                    let member_read = self.symbol(member)?;
                    let flags = member_read.flags();
                    let name = member_read.name_to_owned();
                    let declarations = member_read.declarations();
                    let parent = member_read.parent();
                    let value = member_read.value_declaration();
                    let attribute = self.new_symbol(sf::PROPERTY | flags, name.clone())?;
                    let stored = self.symbol_mut(attribute)?;
                    stored.declarations = declarations;
                    stored.parent = parent;
                    if value.is_some() {
                        stored.value_declaration = value;
                    }
                    let links = self
                        .value_symbol_links
                        .get_or_default(self.value_symbol_key(attribute)?);
                    links.resolved_type = Some(ty);
                    links.target = Some(member);
                    table.insert(name.clone(), Some(attribute));
                    if let Some(all) = &mut all {
                        all.insert(name, Some(attribute));
                    }
                    let attribute_name =
                        required(self.node(declaration)?.name(), "JSX attribute name")?;
                    if children_name.as_ref() == Some(&self.jsx_name_text(attribute_name)?) {
                        explicit_children = true;
                    }
                    if contextual.is_some()
                        && mode & 2 != 0
                        && mode & 4 == 0
                        && self.expression_is_context_sensitive(declaration)?
                    {
                        // In CheckMode.Inferential we should always have an inference context
                        let inference = required(
                            self.call_inference_at_node(attributes)?,
                            "JSX attributes inference context",
                        )?;
                        let initializer = required(
                            self.node(declaration)?.initializer(),
                            "JSX attribute initializer",
                        )?;
                        let site = required(
                            self.node(initializer)?.expression(),
                            "JSX attribute expression",
                        )?;
                        self.add_intra_expression_inference_site(inference, site, ty)?;
                    }
                } else {
                    if !table.is_empty() {
                        let chunk = self.jsx_attributes_chunk(
                            attributes_symbol,
                            std::mem::take(&mut table),
                            &mut object_flags,
                        )?;
                        spread = self.object_spread_type(
                            spread,
                            chunk,
                            attributes_symbol,
                            object_flags,
                            false,
                        )?;
                    }
                    let expression = required(
                        self.node(declaration)?.expression(),
                        "JSX spread expression",
                    )?;
                    let ty = self.check_expression_ex(expression, mode & 2)?;
                    let ty = self.get_reduced_type(ty)?;
                    if self.types.flags(ty)? & tf::ANY != 0 {
                        has_spread_any = true;
                    }
                    if self.valid_spread_type(ty)? {
                        spread = self.object_spread_type(
                            spread,
                            ty,
                            attributes_symbol,
                            object_flags,
                            false,
                        )?;
                        if let Some(all) = &all {
                            self.check_spread_property_overrides(ty, all, declaration)?;
                        }
                    } else {
                        self.error_at(
                            Some(expression),
                            d::Spread_types_may_only_be_created_from_object_types,
                            vec![],
                        )?;
                        intersect = Some(match intersect {
                            Some(previous) => self.get_intersection_type(&[previous, ty])?,
                            None => ty,
                        });
                    }
                }
            }
            if !has_spread_any && !table.is_empty() {
                let chunk = self.jsx_attributes_chunk(
                    attributes_symbol,
                    std::mem::take(&mut table),
                    &mut object_flags,
                )?;
                spread =
                    self.object_spread_type(spread, chunk, attributes_symbol, object_flags, false)?;
            }
        }
        // Handle children attribute
        let parent = self.node(opening)?.parent();
        let children = match parent {
            Some(parent) => {
                let read = self.node(parent)?;
                let owner = match read.kind().known() {
                    // We have to check that openingElement of the parent is the one we are visiting as this may not be true for selfClosingElement
                    Some(K::JsxElement) => read
                        .data_source()
                        .as_jsx_element()
                        .and_then(|data| data.opening_element()),
                    Some(K::JsxFragment) => read
                        .data_source()
                        .as_jsx_fragment()
                        .and_then(|data| data.opening_fragment()),
                    _ => None,
                };
                if owner == Some(opening) {
                    self.jsx_children(parent)?
                } else {
                    Vec::new()
                }
            }
            None => Vec::new(),
        };
        if !self.semantic_jsx_children(&children)?.is_empty() {
            let parent = required(parent, "JSX children parent")?;
            let child_types = self.check_jsx_children(parent, mode)?;
            if let Some(name) = children_name.filter(|name| !has_spread_any && !name.is_empty()) {
                // Error if there is a attribute named "children" explicitly specified and children element.
                // This is because children element will overwrite the value from attributes.
                // Note: we will not warn "children" attribute overwritten if "children" attribute is specified in object spread.
                if explicit_children {
                    self.error_at(
                        Some(attribute_parent),
                        d::X_0_are_specified_twice_The_attribute_named_0_will_be_overwritten,
                        vec![name.clone()],
                    )?;
                }
                let mut children_context = None;
                if self.node(opening)?.kind() == K::JsxOpeningElement {
                    let attributes = self.jsx_attributes_node(opening)?;
                    if let Some(context) =
                        self.apparent_contextual_expression_type_ex(attributes, 0)?
                    {
                        children_context =
                            self.type_of_property_of_contextual_type(context, name.as_bytes())?;
                    }
                }
                // If there are children in the body of JSX element, create dummy attribute "children" with the union of children types so that it will pass the attribute checking process
                let symbol = self.new_symbol(sf::PROPERTY, name.clone())?;
                let tuple_context = match children_context {
                    Some(context) => {
                        if self.types.flags(context)? & tf::UNION != 0 {
                            let mut found = false;
                            for part in self.types.compound_types(context)?.to_vec() {
                                if self.tuple_like_type(part)? {
                                    found = true;
                                    break;
                                }
                            }
                            found
                        } else {
                            self.tuple_like_type(context)?
                        }
                    }
                    None => false,
                };
                let resolved = if child_types.len() == 1 {
                    child_types[0]
                } else if tuple_context {
                    self.create_tuple_type(&child_types)?
                } else {
                    let union = self.get_union_type(&child_types)?;
                    self.create_array_type(union, false)?
                };
                self.value_symbol_links
                    .get_or_default(self.value_symbol_key(symbol)?)
                    .resolved_type = Some(resolved);
                // Fake up a property declaration for the children
                let identifier = self.factory.new_identifier(name.clone());
                let declaration = self.factory.new_property_signature_declaration(
                    None,
                    Some(identifier),
                    None,
                    None,
                    None,
                );
                self.factory.set_node_parent(identifier, Some(declaration));
                self.factory
                    .set_node_parent(declaration, Some(attribute_parent));
                self.synthetic_scopes.bindings.insert(
                    declaration,
                    tsr_ast::NodeBinding {
                        symbol: Some(symbol),
                        ..Default::default()
                    },
                );
                self.symbol_mut(symbol)?.value_declaration = Some(declaration);
                let mut members = SymbolTable::default();
                members.insert(name, Some(symbol));
                let members = self.alloc_symbol_table(members);
                let children_type =
                    self.new_anonymous_type(attributes_symbol, Some(members), &[], &[], &[])?;
                let flags = object_flags | self.get_propagating_flags_of_types(&child_types, 0)?;
                spread = self.object_spread_type(
                    spread,
                    children_type,
                    attributes_symbol,
                    flags,
                    false,
                )?;
            }
        }
        if has_spread_any {
            return Ok(self.builtins.any_type);
        }
        if let Some(intersect) = intersect {
            if spread != empty {
                return self.get_intersection_type(&[intersect, spread]);
            }
            return Ok(intersect);
        }
        if spread == empty {
            return self.jsx_attributes_chunk(attributes_symbol, table, &mut object_flags);
        }
        Ok(spread)
    }

    /// `createJsxAttributesType` inside
    /// `createJsxAttributesTypeFromAttributesProperty`; it marks every later
    /// spread of the element fresh as well.
    fn jsx_attributes_chunk(
        &mut self,
        symbol: Option<SymbolId>,
        table: SymbolTable,
        object_flags: &mut u32,
    ) -> Result<TypeId, Error> {
        *object_flags |= of::FRESH_LITERAL;
        let members = self.alloc_symbol_table(table);
        let result = self.new_anonymous_type(symbol, Some(members), &[], &[], &[])?;
        self.types.get_mut(result)?.object_flags |=
            *object_flags | of::OBJECT_LITERAL | of::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
        Ok(result)
    }

    // port: tsc/internal/checker/jsx.go:Checker.checkJsxAttribute
    pub(crate) fn check_jsx_attribute(&mut self, node: NodeId, mode: u32) -> Result<TypeId, Error> {
        if let Some(initializer) = self.node(node)?.initializer() {
            return self.check_object_mutable_location_ex(initializer, mode);
        }
        // <Elem attr /> is sugar for <Elem attr={true} />
        Ok(self.builtins.true_type)
    }

    // port: tsc/internal/checker/jsx.go:Checker.checkJsxChildren
    fn check_jsx_children(&mut self, node: NodeId, mode: u32) -> Result<Vec<TypeId>, Error> {
        let mut types = Vec::new();
        for child in self.jsx_children(node)? {
            let read = self.node(child)?;
            // In React, JSX text that contains only whitespaces will be ignored so we don't want to type-check that
            // because then type of children property will have constituent of string type.
            if read.kind() == K::JsxText {
                if !tsr_ast::utilities_middle::is_whitespace_only_jsx_text(&read) {
                    types.push(self.builtins.string_type);
                }
            } else if read.kind() != K::JsxExpression || read.expression().is_some() {
                // empty jsx expressions don't *really* count as present children
                types.push(self.check_object_mutable_location_ex(child, mode)?);
            }
        }
        Ok(types)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getUninstantiatedJsxSignaturesOfType
    fn uninstantiated_jsx_signatures_of_type(
        &mut self,
        element: TypeId,
        caller: NodeId,
    ) -> Result<Vec<SignatureId>, Error> {
        let flags = self.types.flags(element)?;
        if flags & tf::STRING != 0 {
            return Ok(vec![self.builtins.any_signature]);
        }
        if flags & tf::STRING_LITERAL != 0 {
            let Some(intrinsic) =
                self.intrinsic_attributes_type_from_string_literal_type(element, caller)?
            else {
                let value = self.jsx_string_literal_value(element)?;
                self.error_at(
                    Some(caller),
                    d::Property_0_does_not_exist_on_type_1,
                    vec![
                        value,
                        JsString::from_bytes(b"JSX.IntrinsicElements".as_slice()),
                    ],
                )?;
                return Ok(Vec::new());
            };
            return Ok(vec![
                self.create_signature_for_jsx_intrinsic(caller, intrinsic)?
            ]);
        }
        let apparent = self.apparent_type(element)?;
        // Resolve the signatures, preferring constructor
        let mut signatures = self.signatures_of_type(apparent, true)?;
        if signatures.is_empty() {
            // No construct signatures, try call signatures
            signatures = self.signatures_of_type(apparent, false)?;
        }
        if signatures.is_empty() && self.types.flags(apparent)? & tf::UNION != 0 {
            // If each member has some combination of new/call signatures; make a union signature list for those
            let mut lists = Vec::new();
            for part in self.types.compound_types(apparent)?.to_vec() {
                lists.push(self.uninstantiated_jsx_signatures_of_type(part, caller)?);
            }
            signatures = self.union_signatures(&lists)?;
        }
        Ok(signatures)
    }

    fn jsx_string_literal_value(&self, ty: TypeId) -> Result<JsString, Error> {
        match &self.types.literal(ty)?.value {
            LiteralValue::String(text) => Ok(text.clone()),
            _ => Err(Error::MissingLink("JSX string literal value")),
        }
    }

    // port: tsc/internal/checker/jsx.go:Checker.getEffectiveFirstArgumentForJsxSignature
    pub(crate) fn effective_first_argument_for_jsx_signature(
        &mut self,
        signature: SignatureId,
        node: NodeId,
    ) -> Result<TypeId, Error> {
        if self.node(node)?.kind() == K::JsxOpeningFragment
            || self.jsx_reference_kind(node)? != JsxReferenceKind::Component
        {
            return self.jsx_props_type_from_call_signature(signature, node);
        }
        self.jsx_props_type_from_class_type(signature, node)
    }

    /// `getTypeOfFirstParameterOfSignatureWithFallback`.
    fn first_parameter_type_or(
        &mut self,
        signature: SignatureId,
        fallback: TypeId,
    ) -> Result<TypeId, Error> {
        if self
            .signatures
            .get(signature)?
            .parameters
            .as_ref()
            .is_some_and(|parameters| !parameters.is_empty())
        {
            return Ok(self
                .parameter_type_at(signature, 0)?
                .unwrap_or(self.builtins.any_type));
        }
        Ok(fallback)
    }

    /// `intersectTypes`.
    fn intersect_jsx_types(&mut self, left: TypeId, right: TypeId) -> Result<TypeId, Error> {
        self.get_intersection_type(&[left, right])
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxPropsTypeFromCallSignature
    fn jsx_props_type_from_call_signature(
        &mut self,
        signature: SignatureId,
        context: NodeId,
    ) -> Result<TypeId, Error> {
        let mut props = self.first_parameter_type_or(signature, self.builtins.unknown_type)?;
        let namespace = self.jsx_namespace_at(context)?;
        props = self.jsx_managed_attributes_from_located_attributes(context, namespace, props)?;
        let intrinsic = self.jsx_type(b"IntrinsicAttributes", context)?;
        if !self.is_error_type(intrinsic)? {
            props = self.intersect_jsx_types(intrinsic, props)?;
        }
        Ok(props)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxPropsTypeFromClassType
    fn jsx_props_type_from_class_type(
        &mut self,
        signature: SignatureId,
        context: NodeId,
    ) -> Result<TypeId, Error> {
        let namespace = self.jsx_namespace_at(context)?;
        let attributes_type = match self.jsx_element_properties_name(namespace)? {
            None => Some(self.first_parameter_type_or(signature, self.builtins.unknown_type)?),
            Some(name) if name.is_empty() => Some(self.return_type_of_signature(signature)?),
            Some(name) => {
                let ty =
                    self.jsx_props_type_for_signature_from_member(signature, name.as_bytes())?;
                if ty.is_none() {
                    let attributes = self.jsx_attributes_node(context)?;
                    if !self
                        .source_list(attributes, self.node(attributes)?.property_list())?
                        .is_empty()
                    {
                        // There is no property named 'props' on this instance type
                        self.error_at(
                            Some(context),
                            d::JSX_element_class_does_not_support_attributes_because_it_does_not_have_a_0_property,
                            vec![name],
                        )?;
                    }
                }
                ty
            }
        };
        let Some(attributes_type) = attributes_type else {
            return Ok(self.builtins.unknown_type);
        };
        let attributes_type = self.jsx_managed_attributes_from_located_attributes(
            context,
            namespace,
            attributes_type,
        )?;
        if self.types.flags(attributes_type)? & tf::ANY != 0 {
            // Props is of type 'any' or unknown
            return Ok(attributes_type);
        }
        // Normal case -- add in IntrinsicClassAttributes<T> and IntrinsicAttributes
        let mut apparent = attributes_type;
        let class_attributes = self.jsx_type(b"IntrinsicClassAttributes", context)?;
        if !self.is_error_type(class_attributes)? {
            let symbol = required(
                self.types.get(class_attributes)?.symbol,
                "JSX.IntrinsicClassAttributes symbol",
            )?;
            let parameters = self.get_local_type_parameters(symbol)?;
            let host = self.return_type_of_signature(signature)?;
            let managed = if parameters.is_empty() {
                class_attributes
            } else {
                // apply JSX.IntrinsicClassAttributes<hostClassType, ...>
                let javascript = tsr_ast::utilities::is_in_js_file(Some(&self.node(context)?));
                let arguments =
                    self.fill_missing_type_arguments(&[host], &parameters, javascript)?;
                let mapper = self.new_type_mapper(&parameters, &arguments)?;
                self.instantiate_type(class_attributes, Some(mapper))?
            };
            apparent = self.intersect_jsx_types(managed, apparent)?;
        }
        let intrinsic = self.jsx_type(b"IntrinsicAttributes", context)?;
        if !self.is_error_type(intrinsic)? {
            apparent = self.intersect_jsx_types(intrinsic, apparent)?;
        }
        Ok(apparent)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxPropsTypeForSignatureFromMember
    fn jsx_props_type_for_signature_from_member(
        &mut self,
        signature: SignatureId,
        name: &[u8],
    ) -> Result<Option<TypeId>, Error> {
        let composite = self
            .signatures
            .get(signature)?
            .composite
            .as_ref()
            .map(|composite| composite.signatures.clone());
        if let Some(parts) = composite {
            // JSX Elements using the legacy `props`-field based lookup (eg, react class components) need to treat the `props` member as an input
            // instead of an output position when resolving the signature. We need to go back to the input signatures of the composite signature,
            // get the type of `props` on each return type individually, and then _intersect them_, rather than union them (as would normally occur
            // for a union signature). It's an unfortunate quirk of looking in the output of the signature for the type we want to use for the input.
            // The default behavior of `getTypeOfFirstParameterOfSignatureWithFallback` when no `props` member name is defined is much more sane.
            let mut results = Vec::new();
            for &part in parts.iter() {
                let instance = self.return_type_of_signature(part)?;
                if self.types.flags(instance)? & tf::ANY != 0 {
                    return Ok(Some(instance));
                }
                let Some(ty) = self.property_type(instance, name)? else {
                    return Ok(None);
                };
                results.push(ty);
            }
            // Same result for both union and intersection signatures
            return self.get_intersection_type(&results).map(Some);
        }
        let instance = self.return_type_of_signature(signature)?;
        if self.types.flags(instance)? & tf::ANY != 0 {
            return Ok(Some(instance));
        }
        self.property_type(instance, name)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxManagedAttributesFromLocatedAttributes
    fn jsx_managed_attributes_from_located_attributes(
        &mut self,
        context: NodeId,
        namespace: Option<SymbolId>,
        attributes: TypeId,
    ) -> Result<TypeId, Error> {
        if let Some(managed) =
            self.jsx_namespace_type_symbol(namespace, b"LibraryManagedAttributes")?
        {
            let constructor = self.static_type_of_referenced_jsx_constructor(context)?;
            let javascript = tsr_ast::utilities::is_in_js_file(Some(&self.node(context)?));
            if let Some(result) = self.instantiate_alias_or_interface_with_defaults(
                managed,
                &[constructor, attributes],
                javascript,
            )? {
                return Ok(result);
            }
        }
        Ok(attributes)
    }

    // port: tsc/internal/checker/jsx.go:Checker.instantiateAliasOrInterfaceWithDefaults
    fn instantiate_alias_or_interface_with_defaults(
        &mut self,
        symbol: SymbolId,
        arguments: &[TypeId],
        javascript: bool,
    ) -> Result<Option<TypeId>, Error> {
        let declared = self.get_declared_type_of_symbol(symbol)?;
        // fetches interface type, or initializes symbol links type parameters
        if self.symbol(symbol)?.flags() & sf::TYPE_ALIAS != 0 {
            let parameters = self
                .query
                .type_aliases
                .try_get(symbol)
                .and_then(|links| links.parameters.clone())
                .unwrap_or_default();
            if parameters.len() >= arguments.len() {
                let arguments =
                    self.fill_missing_type_arguments(arguments, &parameters, javascript)?;
                if arguments.is_empty() {
                    return Ok(Some(declared));
                }
                return self
                    .type_alias_instantiation(symbol, declared, &parameters, &arguments, None)
                    .map(Some);
            }
        }
        if self.types.get(declared)?.object_flags & of::CLASS_OR_INTERFACE != 0 {
            let parameters = self.types.interface(declared)?.type_parameters().to_vec();
            if parameters.len() >= arguments.len() {
                let arguments =
                    self.fill_missing_type_arguments(arguments, &parameters, javascript)?;
                return self.create_type_reference(declared, &arguments).map(Some);
            }
        }
        Ok(None)
    }

    /// A type declared directly in the namespace's own exports.
    // port: tsc/internal/checker/jsx.go:Checker.getJsxLibraryManagedAttributes
    // port: tsc/internal/checker/jsx.go:Checker.getJsxElementTypeSymbol
    fn jsx_namespace_type_symbol(
        &mut self,
        namespace: Option<SymbolId>,
        name: &[u8],
    ) -> Result<Option<SymbolId>, Error> {
        let Some(namespace) = namespace else {
            return Ok(None);
        };
        let exports = self.symbol(namespace)?.exports();
        self.lookup_symbol_resolving(exports, name, sf::TYPE)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxElementPropertiesName
    fn jsx_element_properties_name(
        &mut self,
        namespace: Option<SymbolId>,
    ) -> Result<JsxPropertyName, Error> {
        self.name_from_jsx_element_attributes_container(b"ElementAttributesProperty", namespace)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxElementChildrenPropertyName
    /// The children property name at `location`, whose JSX namespace it reads.
    pub(crate) fn jsx_element_children_property_name(
        &mut self,
        location: NodeId,
    ) -> Result<JsxPropertyName, Error> {
        let namespace = self.jsx_namespace_at(location)?;
        let jsx = self.program()?.host.options().jsx;
        if jsx == tsr_core::JsxEmit::REACT_JSX || jsx == tsr_core::JsxEmit::REACT_JSX_DEV {
            // In these JsxEmit modes the children property is fixed to 'children'
            return Ok(Some(JsString::from_bytes(b"children".as_slice())));
        }
        self.name_from_jsx_element_attributes_container(b"ElementChildrenAttribute", namespace)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getNameFromJsxElementAttributesContainer
    fn name_from_jsx_element_attributes_container(
        &mut self,
        container: &'static [u8],
        namespace: Option<SymbolId>,
    ) -> Result<JsxPropertyName, Error> {
        // JSX.ElementAttributesProperty | JSX.ElementChildrenAttribute [symbol]
        if let Some(symbol) = self.jsx_namespace_type_symbol(namespace, container)? {
            let ty = self.get_declared_type_of_symbol(symbol)?;
            let properties = self.get_properties_of_type(ty)?;
            // Element Attributes has zero properties, so the element attributes type will be the class instance type
            if properties.is_empty() {
                return Ok(Some(JsString::default()));
            }
            if properties.len() == 1 {
                return Ok(Some(self.symbol(properties[0])?.name_to_owned()));
            }
            if let Some(declaration) = self.symbol_declarations(symbol)?.first().flatten() {
                // More than one property on ElementAttributesProperty is an error
                self.error_at(
                    Some(declaration),
                    d::The_global_type_JSX_0_may_not_have_more_than_one_property,
                    vec![JsString::from_bytes(container)],
                )?;
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getStaticTypeOfReferencedJsxConstructor
    fn static_type_of_referenced_jsx_constructor(
        &mut self,
        context: NodeId,
    ) -> Result<TypeId, Error> {
        if self.node(context)?.kind() == K::JsxOpeningFragment {
            return self.jsx_fragment_type(context);
        }
        let tag = self.jsx_tag_name(context)?;
        if self.is_jsx_intrinsic_tag_name(tag)? {
            let result = self.intrinsic_attributes_type_from_jsx_opening_like_element(context)?;
            let fake = self.create_signature_for_jsx_intrinsic(context, result)?;
            return self.isolated_signature_type(fake);
        }
        let tag_type = self.check_expression_cached(tag)?;
        if self.types.flags(tag_type)? & tf::STRING_LITERAL != 0 {
            let Some(result) =
                self.intrinsic_attributes_type_from_string_literal_type(tag_type, context)?
            else {
                return Ok(self.builtins.error_type);
            };
            let fake = self.create_signature_for_jsx_intrinsic(context, result)?;
            return self.isolated_signature_type(fake);
        }
        Ok(tag_type)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getIntrinsicAttributesTypeFromStringLiteralType
    fn intrinsic_attributes_type_from_string_literal_type(
        &mut self,
        ty: TypeId,
        location: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        // If the elemType is a stringLiteral type, we can then provide a check to make sure that the string literal type is one of the Jsx intrinsic element type
        let elements = self.jsx_type(b"IntrinsicElements", location)?;
        if self.is_error_type(elements)? {
            // If we need to report an error, we already done so here. So just return any to prevent any more error downstream
            return Ok(Some(self.builtins.any_type));
        }
        let value = self.jsx_string_literal_value(ty)?;
        if let Some(property) = self.constituent_property(elements, value.as_bytes(), false)? {
            return self.get_type_of_symbol(property).map(Some);
        }
        match self.index_info_of_type(elements, self.builtins.string_type)? {
            Some(info) => Ok(Some(self.signatures.index_info(info)?.value_type)),
            None => Ok(None),
        }
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxReferenceKind
    fn jsx_reference_kind(&mut self, node: NodeId) -> Result<JsxReferenceKind, Error> {
        let tag = self.jsx_tag_name(node)?;
        if self.is_jsx_intrinsic_tag_name(tag)? {
            return Ok(JsxReferenceKind::Mixed);
        }
        let ty = self.check_expression(tag)?;
        let ty = self.apparent_type(ty)?;
        if !self.signatures_of_type(ty, true)?.is_empty() {
            return Ok(JsxReferenceKind::Component);
        }
        if !self.signatures_of_type(ty, false)?.is_empty() {
            return Ok(JsxReferenceKind::Function);
        }
        Ok(JsxReferenceKind::Mixed)
    }

    // port: tsc/internal/checker/jsx.go:Checker.createSignatureForJSXIntrinsic
    fn create_signature_for_jsx_intrinsic(
        &mut self,
        node: NodeId,
        result: TypeId,
    ) -> Result<SignatureId, Error> {
        let mut element = self.builtins.error_type;
        if let Some(namespace) = self.jsx_namespace_at(node)? {
            let exports = self.module_exports_of_symbol(namespace)?;
            if let Some(symbol) = self.lookup_symbol_resolving(exports, b"Element", sf::TYPE)? {
                element = self.get_declared_type_of_symbol(symbol)?;
            }
        }
        let parameter = self.new_parameter(b"props", result)?;
        self.signatures.new_signature(
            sg::NONE,
            None,
            None,
            None,
            Some([parameter].into()),
            Some(element),
            None,
            1,
        )
    }

    // port: tsc/internal/checker/jsx.go:Checker.getIntrinsicAttributesTypeFromJsxOpeningLikeElement
    fn intrinsic_attributes_type_from_jsx_opening_like_element(
        &mut self,
        node: NodeId,
    ) -> Result<TypeId, Error> {
        if let Some(&ty) = self.jsx.attributes_types.get(&node) {
            return Ok(ty);
        }
        let symbol = self.intrinsic_tag_symbol(node)?;
        let flags = self.jsx.flags.get(&node).copied().unwrap_or(0);
        let mut ty = self.builtins.error_type;
        if flags & jsx_flags::INTRINSIC_NAMED_ELEMENT != 0 {
            ty = match symbol {
                Some(symbol) => self.get_type_of_symbol(symbol)?,
                None => self.builtins.error_type,
            };
        } else if flags & jsx_flags::INTRINSIC_INDEXED_ELEMENT != 0 {
            let elements = self.jsx_type(b"IntrinsicElements", node)?;
            let tag = self.jsx_tag_name(node)?;
            let text = self.jsx_name_text(tag)?;
            let key = self.get_string_literal_type(text)?;
            if let Some(info) = self.applicable_index_info(elements, key)? {
                ty = self.signatures.index_info(info)?.value_type;
            }
        }
        self.jsx.attributes_types.insert(node, ty);
        Ok(ty)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getIntrinsicTagSymbol
    /// Looks up an intrinsic tag name and returns a symbol that either points to an intrinsic
    /// property (the element's flags then say IntrinsicNamedElement) or an intrinsic string
    /// index signature (IntrinsicIndexedElement). It may be the unknown symbol when both fail.
    pub(crate) fn intrinsic_tag_symbol(&mut self, node: NodeId) -> Result<Option<SymbolId>, Error> {
        if let Some(&symbol) = self.jsx.intrinsic_tag_symbols.get(&node) {
            return Ok(Some(symbol));
        }
        let elements = self.jsx_type(b"IntrinsicElements", node)?;
        let symbol = if self.is_error_type(elements)? {
            if self.jsx_no_implicit_any()? {
                self.error_at(
                    Some(node),
                    d::JSX_element_implicitly_has_type_any_because_no_interface_JSX_0_exists,
                    vec![JsString::from_bytes(b"IntrinsicElements".as_slice())],
                )?;
            }
            Some(self.builtins.unknown_symbol)
        } else {
            // Property case
            let tag = self.jsx_tag_name(node)?;
            if !matches!(
                self.node(tag)?.kind().known(),
                Some(K::Identifier | K::JsxNamespacedName)
            ) {
                return Err(Error::Unsupported("Invalid tag name"));
            }
            let name = self.jsx_name_text(tag)?;
            if let Some(property) = self.constituent_property(elements, name.as_bytes(), false)? {
                *self.jsx.flags.entry(node).or_default() |= jsx_flags::INTRINSIC_NAMED_ELEMENT;
                Some(property)
            } else {
                // Intrinsic string indexer case
                let key = self.get_string_literal_type(name.clone())?;
                if let Some(index) = self.applicable_index_symbol(elements, key)? {
                    *self.jsx.flags.entry(node).or_default() |=
                        jsx_flags::INTRINSIC_INDEXED_ELEMENT;
                    Some(index)
                } else if self
                    .property_or_index_type(elements, name.as_bytes())?
                    .is_some()
                {
                    *self.jsx.flags.entry(node).or_default() |=
                        jsx_flags::INTRINSIC_INDEXED_ELEMENT;
                    self.types.get(elements)?.symbol
                } else {
                    // Wasn't found
                    self.error_at(
                        Some(node),
                        d::Property_0_does_not_exist_on_type_1,
                        vec![
                            name,
                            JsString::from_bytes(b"JSX.IntrinsicElements".as_slice()),
                        ],
                    )?;
                    Some(self.builtins.unknown_symbol)
                }
            }
        };
        if let Some(symbol) = symbol {
            self.jsx.intrinsic_tag_symbols.insert(node, symbol);
        }
        Ok(symbol)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxStatelessElementTypeAt
    fn jsx_stateless_element_type_at(&mut self, location: NodeId) -> Result<TypeId, Error> {
        // getJsxElementTypeAt never answers nil.
        let element = self.jsx_element_type_at(location)?;
        self.get_union_type(&[element, self.builtins.null_type])
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxElementClassTypeAt
    fn jsx_element_class_type_at(&mut self, location: NodeId) -> Result<Option<TypeId>, Error> {
        let ty = self.jsx_type(b"ElementClass", location)?;
        Ok((!self.is_error_type(ty)?).then_some(ty))
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxElementTypeAt
    fn jsx_element_type_at(&mut self, location: NodeId) -> Result<TypeId, Error> {
        self.jsx_type(b"Element", location)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxElementTypeTypeAt
    fn jsx_element_type_type_at(&mut self, location: NodeId) -> Result<Option<TypeId>, Error> {
        let namespace = self.jsx_namespace_at(location)?;
        if namespace.is_none() {
            return Ok(None);
        }
        let Some(symbol) = self.jsx_namespace_type_symbol(namespace, b"ElementType")? else {
            return Ok(None);
        };
        let javascript = tsr_ast::utilities::is_in_js_file(Some(&self.node(location)?));
        let Some(ty) =
            self.instantiate_alias_or_interface_with_defaults(symbol, &[], javascript)?
        else {
            return Ok(None);
        };
        Ok((!self.is_error_type(ty)?).then_some(ty))
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxType
    pub(crate) fn jsx_type(&mut self, name: &[u8], location: NodeId) -> Result<TypeId, Error> {
        if let Some(namespace) = self.jsx_namespace_at(location)? {
            if let Some(exports) = self.module_exports_of_symbol(namespace)? {
                if let Some(symbol) = self.lookup_symbol_resolving(Some(exports), name, sf::TYPE)? {
                    return self.get_declared_type_of_symbol(symbol);
                }
            }
        }
        Ok(self.builtins.error_type)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxNamespaceAt
    fn jsx_namespace_at(&mut self, location: NodeId) -> Result<Option<SymbolId>, Error> {
        let unknown = self.builtins.unknown_symbol;
        let cached = self.jsx.namespaces.get(&location).copied();
        if let Some(namespace) = cached.filter(|&namespace| namespace != unknown) {
            return Ok(Some(namespace));
        }
        if cached != Some(unknown) {
            let mut resolved = self.jsx_namespace_container_for_implicit_import(location)?;
            if resolved.is_none_or(|symbol| symbol == unknown) {
                let name = self.jsx_namespace(Some(location))?;
                resolved = self.resolve_name_ex(
                    Some(location),
                    name.as_bytes(),
                    sf::NAMESPACE,
                    None,
                    false,
                    false,
                )?;
            }
            if let Some(resolved) = resolved {
                let resolved = self.resolve_module_symbol(Some(resolved), false)?;
                let exports = match resolved {
                    Some(resolved) => self.module_exports_of_symbol(resolved)?,
                    None => None,
                };
                let candidate = self.lookup_symbol_resolving(exports, b"JSX", sf::NAMESPACE)?;
                let candidate = self.resolve_module_symbol(candidate, false)?;
                if let Some(candidate) = candidate.filter(|&candidate| candidate != unknown) {
                    self.jsx.namespaces.insert(location, candidate);
                    return Ok(Some(candidate));
                }
            }
            self.jsx.namespaces.insert(location, unknown);
        }
        // JSX global fallback
        let global = self.resolve_name(None, b"JSX", sf::NAMESPACE, None, false)?;
        let global = self.resolve_module_symbol(global, false)?;
        Ok(global.filter(|&symbol| symbol != unknown))
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxNamespace
    pub(crate) fn jsx_namespace(&mut self, location: Option<NodeId>) -> Result<JsString, Error> {
        if let Some(location) = location {
            let file = self.jsx_source_file(location)?;
            if self.node(location)?.kind() == K::JsxOpeningFragment {
                if let Some(namespace) = self
                    .jsx
                    .files
                    .get(&file)
                    .and_then(|links| links.local_fragment_namespace.clone())
                {
                    return Ok(namespace);
                }
                if let Some(factory) = self.jsx_pragma_factory(file, b"jsxfrag")? {
                    let parsed = self.parse_isolated_entity_name(factory.as_bytes())?;
                    let links = self.jsx.files.entry(file).or_default();
                    links.local_fragment_factory = parsed.as_ref().map(|(node, _)| *node);
                    if let Some((_, namespace)) = parsed {
                        links.local_fragment_namespace = Some(namespace.clone());
                        return Ok(namespace);
                    }
                }
                if let Some(entity) = self.jsx_fragment_factory_entity(Some(location))? {
                    let namespace = self.first_identifier_text(entity)?;
                    let links = self.jsx.files.entry(file).or_default();
                    links.local_fragment_factory = Some(entity);
                    links.local_fragment_namespace = Some(namespace.clone());
                    return Ok(namespace);
                }
            } else if let Some(namespace) = self.local_jsx_namespace(file)? {
                self.jsx.files.entry(file).or_default().local_namespace = Some(namespace.clone());
                return Ok(namespace);
            }
        }
        if self.jsx.namespace.is_none() {
            let options = self.program()?.host.options();
            let factory = options.jsx_factory.clone();
            let react = options.react_namespace.clone();
            let mut namespace = JsString::from_bytes(b"React".as_slice());
            if !factory.is_empty() {
                let parsed = self.parse_isolated_entity_name(factory.as_bytes())?;
                self.jsx.factory_entity = parsed.as_ref().map(|(node, _)| *node);
                if let Some((_, first)) = parsed {
                    namespace = first;
                }
            } else if !react.is_empty() {
                namespace = react;
            }
            self.jsx.namespace = Some(namespace);
        }
        let namespace = self.jsx.namespace.clone().unwrap_or_default();
        if self.jsx.factory_entity.is_none() {
            let left = self.factory.new_identifier(namespace.clone());
            let right = self
                .factory
                .new_identifier(JsString::from_bytes(b"createElement".as_slice()));
            let entity = self.factory.new_qualified_name(Some(left), Some(right));
            self.factory.set_node_parent(left, Some(entity));
            self.factory.set_node_parent(right, Some(entity));
            self.jsx.factory_entity = Some(entity);
        }
        Ok(namespace)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getLocalJsxNamespace
    fn local_jsx_namespace(&mut self, file: NodeId) -> Result<Option<JsString>, Error> {
        if let Some(namespace) = self
            .jsx
            .files
            .get(&file)
            .and_then(|links| links.local_namespace.clone())
        {
            return Ok(Some(namespace));
        }
        if let Some(factory) = self.jsx_pragma_factory(file, b"jsx")? {
            let parsed = self.parse_isolated_entity_name(factory.as_bytes())?;
            let links = self.jsx.files.entry(file).or_default();
            links.local_factory = parsed.as_ref().map(|(node, _)| *node);
            if let Some((_, namespace)) = parsed {
                links.local_namespace = Some(namespace.clone());
                return Ok(Some(namespace));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxFactoryEntity
    pub(crate) fn jsx_factory_entity(
        &mut self,
        location: Option<NodeId>,
    ) -> Result<Option<NodeId>, Error> {
        if let Some(location) = location {
            self.jsx_namespace(Some(location))?;
            let file = self.jsx_source_file(location)?;
            if let Some(factory) = self
                .jsx
                .files
                .get(&file)
                .and_then(|links| links.local_factory)
            {
                return Ok(Some(factory));
            }
        }
        Ok(self.jsx.factory_entity)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxFragmentFactoryEntity
    pub(crate) fn jsx_fragment_factory_entity(
        &mut self,
        location: Option<NodeId>,
    ) -> Result<Option<NodeId>, Error> {
        if let Some(location) = location {
            let file = self.jsx_source_file(location)?;
            if let Some(factory) = self
                .jsx
                .files
                .get(&file)
                .and_then(|links| links.local_fragment_factory)
            {
                return Ok(Some(factory));
            }
            if let Some(factory) = self.jsx_pragma_factory(file, b"jsxfrag")? {
                let parsed = self.parse_isolated_entity_name(factory.as_bytes())?;
                let entity = parsed.map(|(node, _)| node);
                self.jsx
                    .files
                    .entry(file)
                    .or_default()
                    .local_fragment_factory = entity;
                return Ok(entity);
            }
        }
        let factory = self.program()?.host.options().jsx_fragment_factory.clone();
        if !factory.is_empty() {
            return Ok(self
                .parse_isolated_entity_name(factory.as_bytes())?
                .map(|(node, _)| node));
        }
        Ok(None)
    }

    /// `getFirstIdentifier(entity).Text()` of an entity built by
    /// `parseIsolatedEntityName`.
    fn first_identifier_text(&self, entity: NodeId) -> Result<JsString, Error> {
        let view = self.factory.view();
        let first = tsr_ast::utilities_middle::get_first_identifier(view, entity)?;
        Ok(view.node_text(first)?.into_js_string())
    }

    // port: tsc/internal/checker/jsx.go:Checker.parseIsolatedEntityName
    // port: tsc/internal/checker/jsx.go:markAsSynthetic
    /// The parsed name is rebuilt in the checker's factory, where every node
    /// has the synthetic range; the result carries its first identifier's text.
    fn parse_isolated_entity_name(
        &mut self,
        text: &[u8],
    ) -> Result<Option<(NodeId, JsString)>, Error> {
        let Some(parsed) = tsr_parser::parse_isolated_entity_name(
            tsr_jsstring::SourceText::from_loaded_bytes(text.to_vec()),
        ) else {
            return Ok(None);
        };
        fn collect(
            view: tsr_ast::AstView<'_>,
            node: NodeId,
            parts: &mut Vec<JsString>,
        ) -> Result<(), Error> {
            let read = view.node(node)?;
            match read.kind().known() {
                Some(K::Identifier) => parts.push(view.node_text(node)?.into_js_string()),
                Some(K::QualifiedName) => {
                    let data = read
                        .data_source()
                        .as_qualified_name()
                        .ok_or(Error::MissingLink("isolated qualified name"))?;
                    let left = required(data.left(), "isolated qualified name left")?;
                    let right = required(data.right(), "isolated qualified name right")?;
                    collect(view, left, parts)?;
                    collect(view, right, parts)?;
                }
                _ => return Err(Error::Unsupported("parseIsolatedEntityName: entity syntax")),
            }
            Ok(())
        }
        let mut parts = Vec::new();
        collect(parsed.view(), parsed.root(), &mut parts)?;
        let synthetic = tsr_core::TextRange::new(-1, -1);
        let mut parts = parts.into_iter();
        let first = required(parts.next(), "isolated entity name")?;
        let mut entity = self.factory.new_identifier(first.clone());
        self.factory.set_node_range(entity, synthetic);
        for part in parts {
            let right = self.factory.new_identifier(part);
            self.factory.set_node_range(right, synthetic);
            let name = self.factory.new_qualified_name(Some(entity), Some(right));
            self.factory.set_node_range(name, synthetic);
            self.factory.set_node_parent(entity, Some(name));
            self.factory.set_node_parent(right, Some(name));
            entity = name;
        }
        Ok(Some((entity, first)))
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJsxNamespaceContainerForImplicitImport
    pub(crate) fn jsx_namespace_container_for_implicit_import(
        &mut self,
        location: NodeId,
    ) -> Result<Option<SymbolId>, Error> {
        let file = self.jsx_source_file(location)?;
        let unknown = self.builtins.unknown_symbol;
        if let Some(&container) = self.jsx.implicit_imports.get(&file) {
            return Ok((container != unknown).then_some(container));
        }
        let tag = if let Some(&tag) = self.jsx.first_tags.get(&file) {
            tag
        } else {
            let tag = self.first_jsx_tag_in_file(file)?;
            self.jsx.first_tags.insert(file, tag);
            tag
        };
        let reference = self.jsx_runtime_import_specifier(file)?;
        if reference.is_empty() {
            return Ok(None);
        }
        let module = self.resolve_jsx_runtime_module(file, reference, tag)?;
        let result = match module.filter(|&module| module != unknown) {
            Some(module) => {
                let resolved = required(
                    self.resolve_module_symbol(Some(module), false)?,
                    "JSX runtime module",
                )?;
                Some(self.get_merged_symbol(resolved))
            }
            None => None,
        };
        self.jsx
            .implicit_imports
            .insert(file, result.unwrap_or(unknown));
        Ok(result)
    }

    // port: tsc/internal/checker/jsx.go:Checker.getJSXRuntimeImportSpecifier
    fn jsx_runtime_import_specifier(&self, file: NodeId) -> Result<JsString, Error> {
        let file_name = self
            .source_file_read(file)?
            .parse_options()
            .file_name
            .clone();
        self.program()?
            .host
            .get_jsx_runtime_import_specifier(file_name.as_bytes())
    }

    /// The first JSX element, self-closing element or fragment's opening
    /// fragment in the file, in `ForEachChild` order.
    fn first_jsx_tag_in_file(&self, file: NodeId) -> Result<Option<NodeId>, Error> {
        let mut stack = self.source_children(file)?;
        stack.reverse();
        while let Some(node) = stack.pop() {
            let read = self.node(node)?;
            match read.kind().known() {
                Some(K::JsxElement | K::JsxSelfClosingElement) => return Ok(Some(node)),
                // to match strada, fragments issue errors on the opening fragment instead of the whole tag
                Some(K::JsxFragment) => {
                    return Ok(read
                        .data_source()
                        .as_jsx_fragment()
                        .and_then(|data| data.opening_fragment()))
                }
                _ => {
                    let mut children = self.source_children(node)?;
                    children.reverse();
                    stack.extend(children);
                }
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.markJsxAliasReferenced
    pub(crate) fn mark_jsx_alias_referenced(&mut self, node: NodeId) -> Result<(), Error> {
        if self
            .jsx_namespace_container_for_implicit_import(node)?
            .is_some()
        {
            return Ok(());
        }
        // The reactNamespace/jsxFactory's root symbol should be marked as 'used' so we don't incorrectly elide its import.
        // And if there is no reactNamespace/jsxFactory's symbol in scope when targeting React emit, we should issue an error.
        let options = self.program()?.host.options();
        let jsx = options.jsx;
        let can_collect = !options.verbatim_module_syntax.is_true();
        let message = (jsx == tsr_core::JsxEmit::REACT)
            .then_some(d::This_JSX_tag_requires_0_to_be_in_scope_but_it_could_not_be_found);
        let namespace = self.jsx_namespace(Some(node))?;
        let fragment = self.node(node)?.kind() == K::JsxOpeningFragment;
        let location = if fragment {
            node
        } else {
            self.jsx_tag_name(node)?
        };
        let factory_error =
            jsx != tsr_core::JsxEmit::PRESERVE && jsx != tsr_core::JsxEmit::REACT_NATIVE;
        let mut flags = sf::VALUE;
        if !factory_error {
            flags &= !sf::ENUM;
        }
        // #38720/60122, allow null as jsxFragmentFactory
        let symbol = if fragment && namespace.as_bytes() == b"null" {
            None
        } else {
            self.resolve_name_ex(
                Some(location),
                namespace.as_bytes(),
                flags,
                message,
                true,
                false,
            )?
        };
        if let Some(symbol) = symbol {
            // Mark local symbol as referenced here because it might not have been marked
            // if jsx emit was not jsxFactory as there wont be error being emitted
            *self.query.references.get_or_default(symbol) |= sf::ALL;
            // If react/jsxFactory symbol is alias, mark it as referenced
            if can_collect
                && self.symbol(symbol)?.flags() & sf::ALIAS != 0
                && self.direct_type_only_alias_declaration(symbol)?.is_none()
            {
                self.mark_module_alias_referenced(symbol)?;
            }
        }
        // if JsxFragment, additionally mark jsx pragma as referenced, since `getJsxNamespace` above would have resolved to only the fragment factory if they are distinct
        if fragment {
            let file = self.jsx_source_file(node)?;
            if let Some(entity) = self.jsx_factory_entity(Some(file))? {
                let local = self.first_identifier_text(entity)?;
                self.resolve_name_ex(
                    Some(location),
                    local.as_bytes(),
                    flags,
                    message,
                    true,
                    false,
                )?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarJsxElement
    fn check_grammar_jsx_element(&mut self, node: NodeId) -> Result<bool, Error> {
        let tag = self.jsx_tag_name(node)?;
        self.check_grammar_jsx_name(tag)?;
        self.check_grammar_type_arguments(node)?;
        let attributes = self.jsx_attributes_node(node)?;
        let mut seen = std::collections::HashSet::new();
        for attribute in self.source_list(attributes, self.node(attributes)?.property_list())? {
            let read = self.node(attribute)?;
            if read.kind() == K::JsxSpreadAttribute {
                continue;
            }
            let name = required(read.name(), "JSX attribute name")?;
            let initializer = read.initializer();
            let text = self.jsx_name_text(name)?;
            if !seen.insert(text) {
                return self.grammar_error_node(
                    name,
                    d::JSX_elements_cannot_have_multiple_attributes_with_the_same_name,
                    vec![],
                );
            }
            if let Some(initializer) = initializer {
                let read = self.node(initializer)?;
                if read.kind() == K::JsxExpression && read.expression().is_none() {
                    return self.grammar_error_node(
                        initializer,
                        d::JSX_attributes_must_only_be_assigned_a_non_empty_expression,
                        vec![],
                    );
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarJsxName
    fn check_grammar_jsx_name(&mut self, node: NodeId) -> Result<bool, Error> {
        let read = self.node(node)?;
        if read.kind() == K::PropertyAccessExpression {
            let expression = required(read.expression(), "JSX tag receiver")?;
            if self.node(expression)?.kind() == K::JsxNamespacedName {
                return self.grammar_error_node(
                    expression,
                    d::JSX_property_access_expressions_cannot_include_JSX_namespace_names,
                    vec![],
                );
            }
        }
        if read.kind() == K::JsxNamespacedName
            && self.program()?.host.options().jsx_transform_enabled()
        {
            let namespace = read
                .data_source()
                .as_jsx_namespaced_name()
                .and_then(|data| data.namespace());
            let namespace = required(namespace, "JSX namespace")?;
            if !tsr_scanner::is_intrinsic_jsx_name(self.node_text(namespace)?.as_bytes()) {
                return self.grammar_error_node(
                    node,
                    d::React_components_cannot_include_JSX_namespace_names,
                    vec![],
                );
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarJsxExpression
    fn check_grammar_jsx_expression(&mut self, node: NodeId) -> Result<bool, Error> {
        if let Some(expression) = self.node(node)?.expression() {
            if tsr_ast::utilities::is_comma_sequence(self.ast(expression)?, expression)? {
                return self.grammar_error_node(
                    expression,
                    d::JSX_expressions_may_not_use_the_comma_operator_Did_you_mean_to_write_an_array,
                    vec![],
                );
            }
        }
        Ok(false)
    }
}

#[cfg(feature = "relation-probe")]
impl CheckerState {
    /// The JSX state the emit resolver reads at an opening element or
    /// fragment: the factory and fragment factory entities, the file that
    /// declares the `JSX` namespace, and the file of the implicit runtime
    /// import (C4 contracts 1, 8 and 9).
    pub(crate) fn jsx_link_state(&mut self, node: NodeId) -> Result<serde_json::Value, Error> {
        let factory = self.jsx_factory_entity(Some(node))?;
        let fragment = self.jsx_fragment_factory_entity(Some(node))?;
        let namespace = self.jsx_namespace_at(node)?;
        let container = self.jsx_namespace_container_for_implicit_import(node)?;
        let entity = |state: &Self, entity: Option<NodeId>| -> Result<serde_json::Value, Error> {
            Ok(match entity {
                Some(entity) => serde_json::Value::String(
                    String::from_utf8_lossy(state.jsx_entity_name_text(entity)?.as_bytes())
                        .into_owned(),
                ),
                None => serde_json::Value::Null,
            })
        };
        let declared_in =
            |state: &Self, symbol: Option<SymbolId>| -> Result<serde_json::Value, Error> {
                let Some(symbol) = symbol else {
                    return Ok(serde_json::Value::Null);
                };
                let Some(declaration) = state.symbol_declarations(symbol)?.first().flatten() else {
                    return Ok(serde_json::Value::Null);
                };
                let file = state.jsx_source_file(declaration)?;
                let name = state
                    .source_file_read(file)?
                    .parse_options()
                    .file_name
                    .clone();
                Ok(serde_json::Value::String(
                    String::from_utf8_lossy(name.as_bytes()).into_owned(),
                ))
            };
        Ok(serde_json::json!({
            "factory": entity(self, factory)?,
            "fragment_factory": entity(self, fragment)?,
            "namespace": declared_in(self, namespace)?,
            "implicit_import": declared_in(self, container)?,
        }))
    }

    /// Every syntax kind the checker's own factory has built (C4 contract 9).
    pub(crate) fn synthetic_syntax_kinds(&self) -> Result<Vec<String>, Error> {
        let view = self.factory.view();
        let arena = self.factory.id().arena();
        let count = u32::try_from(self.factory.node_count()).unwrap_or(0);
        let mut kinds = std::collections::BTreeSet::new();
        for slot in 1..=count {
            let id = NodeId::from_parts(arena, slot)?;
            if let Ok(read) = view.node(id) {
                kinds.insert(read.kind_string());
            }
        }
        Ok(kinds.into_iter().collect())
    }
}
