//! Value queries and the supported native return-inference branches.
//! Flow environments contain declaration identities, never identifier spellings.

use super::{
    instantiate, mf, missing, of, tf, Construction, Environment, Error, HashMap, LiteralValue,
    NodeId, Rc, SymbolGroup, TypeCell, TypeLink, K,
};
use tsr_ast::{JsDocProvider, NodeDataRead};

#[derive(Clone, Default)]
struct FlowTypes(HashMap<NodeId, Rc<TypeCell>>);

impl Construction {
    // port: tsc/internal/checker/checker.go:Checker.checkComputedPropertyName
    pub(super) fn member_name(
        self: &Rc<Self>,
        node: NodeId,
        env: &Environment,
    ) -> Result<(Rc<str>, Option<TypeLink>), Error> {
        let read = self.input.node(node)?;
        let ty = if let Some(computed) = read.data_source().as_computed_property_name() {
            self.expression_type(
                computed.expression().ok_or(Error::ResolutionFailed)?,
                env,
                &FlowTypes::default(),
            )?
        } else if read.kind() == K::NumericLiteral {
            self.literal(node)?
        } else {
            let name = self.name(node)?;
            if name.starts_with("\u{ffff}@unique:") {
                return missing(
                    "source property name uses the reserved unique-symbol identity prefix",
                );
            }
            return Ok((name, None));
        };
        let name = match &ty.literal {
            Some(LiteralValue::String(bytes)) => String::from_utf8(bytes.clone())
                .map_err(|_| Error::Unsupported("non-UTF8 computed string property".into()))?,
            Some(LiteralValue::Number(bits)) => {
                tsr_jsnum::Number::new(f64::from_bits(*bits)).to_string()
            }
            _ if ty.flags & tf::UNIQUE_ES_SYMBOL != 0 => format!(
                "\u{ffff}@unique:{}",
                ty.symbol.ok_or(Error::ResolutionFailed)?
            ),
            _ => {
                return missing(
                    "computed property requires a string, number or unique-symbol literal type",
                )
            }
        };
        if ty.flags & tf::UNIQUE_ES_SYMBOL == 0 && name.starts_with("\u{ffff}@unique:") {
            return missing("source property name uses the reserved unique-symbol identity prefix");
        }
        Ok((name.into(), Some(Rc::downgrade(&ty).into())))
    }

    // port: tsc/internal/checker/checker.go:Checker.getESSymbolLikeTypeForNode
    pub(super) fn unique_symbol_type(self: &Rc<Self>, node: NodeId) -> Result<Rc<TypeCell>, Error> {
        let read = self.input.node(node)?;
        let data = read
            .data_source()
            .as_type_operator_node()
            .ok_or(Error::ResolutionFailed)?;
        if self
            .input
            .node(data.r#type().ok_or(Error::ResolutionFailed)?)?
            .kind()
            != K::SymbolKeyword
        {
            return self.initialization.named("errorType");
        }
        let mut declaration = read.parent().ok_or(Error::ResolutionFailed)?;
        while self.input.node(declaration)?.kind() == K::ParenthesizedType {
            declaration = self
                .input
                .node(declaration)?
                .parent()
                .ok_or(Error::ResolutionFailed)?;
        }
        let read = self.input.node(declaration)?;
        let modifiers = read.modifier_flags(self.input.ast(declaration)?)?;
        let valid = match read.kind().known() {
            Some(K::PropertySignature) => modifiers & mf::READONLY != 0,
            Some(K::PropertyDeclaration) => {
                modifiers & (mf::READONLY | mf::STATIC) == mf::READONLY | mf::STATIC
            }
            Some(K::VariableDeclaration) => {
                let parent = self
                    .input
                    .node(read.parent().ok_or(Error::ResolutionFailed)?)?;
                parent.kind() == K::VariableDeclarationList
                    && parent.flags() & tsr_ast::node_flags::CONST != 0
                    && read.name().is_some_and(|name| {
                        self.input
                            .node(name)
                            .is_ok_and(|read| read.kind() == K::Identifier)
                    })
                    && parent.parent().is_some_and(|parent| {
                        self.input
                            .node(parent)
                            .is_ok_and(|read| read.kind() == K::VariableStatement)
                    })
            }
            _ => false,
        };
        if !valid {
            return self.builtin(tf::ES_SYMBOL);
        }
        let Some(symbol) = self
            .input
            .binding(declaration)?
            .and_then(|binding| binding.symbol)
        else {
            return self.builtin(tf::ES_SYMBOL);
        };
        let canonical = self
            .input
            .declarations(symbol)?
            .iter()
            .flatten()
            .next()
            .ok_or(Error::ResolutionFailed)?;
        let key = (canonical, Default::default());
        if let Some(ty) = self.node_types.borrow().get(&key).cloned() {
            return Ok(ty);
        }
        let ty = self.checker.graph.allocate_full(
            tf::UNIQUE_ES_SYMBOL,
            0,
            "unique symbol".into(),
            Some(self.identity(canonical)),
            None,
            None,
            false,
            Vec::new(),
            false,
            None,
        );
        self.node_types.borrow_mut().insert(key, ty.clone());
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromTypeQueryNode
    pub(super) fn type_query(
        self: &Rc<Self>,
        node: NodeId,
        env: &Environment,
    ) -> Result<Rc<TypeCell>, Error> {
        let read = self.input.node(node)?;
        let name = read
            .data_source()
            .as_type_query_node()
            .and_then(|data| data.expr_name())
            .ok_or(Error::ResolutionFailed)?;
        let group = self
            .input
            .resolve_value_name(name)?
            .ok_or_else(|| Error::Unsupported("unresolved type-query value".into()))?;
        self.value_type_of_symbol(&group, env)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeOfSymbol
    pub(super) fn value_type_of_symbol(
        self: &Rc<Self>,
        group: &SymbolGroup,
        env: &Environment,
    ) -> Result<Rc<TypeCell>, Error> {
        let key = (group.symbols.clone(), env.key()?);
        if let Some(ty) = self.value_types.borrow().get(&key).cloned() {
            return Ok(ty);
        }
        let declarations = self.declarations(group)?;
        let declaration = group
            .symbols
            .iter()
            .find_map(|&symbol| self.input.symbol(symbol).ok()?.value_declaration())
            .or_else(|| declarations.first().copied())
            .ok_or(Error::ResolutionFailed)?;
        let read = self.input.node(declaration)?;
        let ty = match read.kind().known() {
            Some(
                K::FunctionDeclaration
                | K::FunctionExpression
                | K::ArrowFunction
                | K::MethodDeclaration
                | K::MethodSignature,
            ) => {
                let name = match read.name() {
                    Some(name) => self.name(name)?,
                    None => Rc::from("__function"),
                };
                let ty = self.object(declarations, env.clone(), name, of::ANONYMOUS)?;
                // The function's anonymous type has a declaration, so it could
                // contain type variables and takes the counted instantiation
                // path (which returns it unchanged without outer parameters).
                self.record_type_source(
                    &ty,
                    instantiate::TypeSource {
                        node: declaration,
                        environment: env.clone(),
                        alias: None,
                    },
                );
                ty
            }
            Some(K::Parameter) => self.parameter_type_of_declaration(declaration, env)?,
            Some(K::VariableDeclaration | K::PropertyDeclaration | K::PropertySignature) => {
                if let Some(annotation) = read.type_node() {
                    self.type_node(annotation, env, None)?
                } else if let Some(initializer) = read.initializer() {
                    let ty = self.expression_type(initializer, env, &FlowTypes::default())?;
                    self.widen_return_literal(&ty)?
                } else {
                    self.builtin(tf::ANY)?
                }
            }
            _ => return missing(&format!("value symbol declaration {}", read.kind_string())),
        };
        self.value_types.borrow_mut().insert(key, ty.clone());
        Ok(ty)
    }

    pub(super) fn parameter_type_of_declaration(
        self: &Rc<Self>,
        parameter: NodeId,
        env: &Environment,
    ) -> Result<Rc<TypeCell>, Error> {
        let read = self.input.node(parameter)?;
        if let Some(annotation) = read.type_node() {
            return self.type_node(annotation, env, None);
        }
        let function = read.parent().ok_or(Error::ResolutionFailed)?;
        let parameter_name = self.text(read.name().ok_or(Error::ResolutionFailed)?)?;
        for tag in self.jsdoc_tags(function)? {
            let tag_node = self.input.node(tag)?;
            if tag_node.kind() != K::JSDocParameterTag {
                continue;
            }
            let data = tag_node.data_source();
            let data = data
                .as_js_doc_parameter_or_property_tag()
                .ok_or(Error::ResolutionFailed)?;
            let Some(name) = data.name() else { continue };
            if self.text(name)? != parameter_name {
                continue;
            }
            if let Some(expression) = data.type_expression() {
                let ty = self.jsdoc_type_expression(expression, env)?;
                return if data.is_bracketed() && self.input.options().strict_null_checks {
                    self.checker
                        .graph
                        .union(&[ty, self.builtin(tf::UNDEFINED)?])
                } else {
                    Ok(ty)
                };
            }
        }
        if let Some(initializer) = read.initializer() {
            let ty = self.expression_type(initializer, env, &FlowTypes::default())?;
            return self.widen_return_literal(&ty);
        }
        self.builtin(tf::ANY)
    }

    // port: tsc/internal/checker/checker.go:Checker.getReturnTypeFromBody
    pub(super) fn return_type_of_declaration(
        self: &Rc<Self>,
        declaration: NodeId,
        env: &Environment,
    ) -> Result<Rc<TypeCell>, Error> {
        let key = (declaration, env.key()?);
        if let Some(ty) = self.return_types.borrow().get(&key).cloned() {
            return Ok(ty);
        }
        if self.resolving_returns.borrow().contains(&key) {
            return missing("recursive function return inference");
        }
        self.resolving_returns.borrow_mut().push(key.clone());
        let result = self.return_type_worker(declaration, env);
        self.resolving_returns.borrow_mut().pop();
        let ty = result?;
        self.return_types.borrow_mut().insert(key, ty.clone());
        Ok(ty)
    }

    fn return_type_worker(
        self: &Rc<Self>,
        declaration: NodeId,
        env: &Environment,
    ) -> Result<Rc<TypeCell>, Error> {
        let read = self.input.node(declaration)?;
        if let Some(annotation) = read.type_node() {
            return self.type_node(annotation, env, None);
        }
        for tag in self.jsdoc_tags(declaration)? {
            if let NodeDataRead::JSDocReturnTag(data) = self.input.node(tag)?.data() {
                if let Some(expression) = data.type_expression() {
                    return self.jsdoc_type_expression(expression, env);
                }
            }
        }
        if read.modifier_flags(self.input.ast(declaration)?)? & mf::ASYNC != 0 {
            return missing("async return inference");
        }
        let data = read.data_source();
        let asterisk = data
            .as_function_declaration()
            .and_then(|data| data.asterisk_token())
            .or_else(|| {
                data.as_function_expression()
                    .and_then(|data| data.asterisk_token())
            })
            .or_else(|| {
                data.as_method_declaration()
                    .and_then(|data| data.asterisk_token())
            });
        if asterisk.is_some() {
            return missing("generator return inference");
        }
        let Some(body) = read.body() else {
            return self.builtin(tf::ANY);
        };
        if self.input.node(body)?.kind() != K::Block {
            let ty = self.expression_type(body, env, &FlowTypes::default())?;
            return self.widen_return_literal(&ty);
        }
        let mut returns = Vec::new();
        let mut bare_return = false;
        let (reaches_end, _) = self.return_statements(
            body,
            env,
            FlowTypes::default(),
            &mut returns,
            &mut bare_return,
        )?;
        if returns.is_empty() {
            return self.builtin(
                if !reaches_end
                    && !bare_return
                    && matches!(
                        read.kind().known(),
                        Some(K::FunctionExpression | K::ArrowFunction)
                    )
                {
                    tf::NEVER
                } else {
                    tf::VOID
                },
            );
        }
        if self.input.options().strict_null_checks && (reaches_end || bare_return) {
            returns.push(self.builtin(tf::UNDEFINED)?);
        }
        let result = self.checker.graph.union(&returns)?;
        self.widen_return_literal(&result)
    }

    fn jsdoc_tags(&self, declaration: NodeId) -> Result<Vec<NodeId>, Error> {
        let source = self.input.source(declaration)?;
        let roots = tsr_parser::ParserJsDocProvider::default().jsdoc(
            self.input.ast(declaration)?,
            source,
            declaration,
        )?;
        let mut tags = Vec::new();
        for &root in roots.iter() {
            let read = self.input.node(root)?;
            let list = read.data_source().as_js_doc().and_then(|data| data.tags());
            tags.extend(self.list(root, list)?);
        }
        Ok(tags)
    }

    fn jsdoc_type_expression(
        self: &Rc<Self>,
        expression: NodeId,
        env: &Environment,
    ) -> Result<Rc<TypeCell>, Error> {
        let node = self.input.node(expression)?;
        let annotation = node
            .data_source()
            .as_js_doc_type_expression()
            .and_then(|data| data.r#type())
            .ok_or(Error::ResolutionFailed)?;
        self.type_node(annotation, env, None)
    }

    fn return_statements(
        self: &Rc<Self>,
        statement: NodeId,
        env: &Environment,
        flow: FlowTypes,
        returns: &mut Vec<Rc<TypeCell>>,
        bare: &mut bool,
    ) -> Result<(bool, FlowTypes), Error> {
        let node = self.input.node(statement)?;
        match node.data() {
            NodeDataRead::Block(data) => {
                let mut current = flow;
                for statement in self.list(statement, data.statements())? {
                    let (reachable, next) =
                        self.return_statements(statement, env, current, returns, bare)?;
                    current = next;
                    if !reachable {
                        return Ok((false, current));
                    }
                }
                Ok((true, current))
            }
            NodeDataRead::ReturnStatement(data) => {
                if let Some(expression) = data.expression() {
                    let ty = self.expression_type(expression, env, &flow)?;
                    if !returns.iter().any(|prior| Rc::ptr_eq(prior, &ty)) {
                        returns.push(ty);
                    }
                } else {
                    *bare = true;
                }
                Ok((false, flow))
            }
            NodeDataRead::IfStatement(data) => {
                let condition = data.expression().ok_or(Error::ResolutionFailed)?;
                let (when_true, when_false) = self.narrow_condition(condition, env, &flow)?;
                let (true_reachable, true_flow) = self.return_statements(
                    data.then_statement().ok_or(Error::ResolutionFailed)?,
                    env,
                    when_true,
                    returns,
                    bare,
                )?;
                let (false_reachable, false_flow) = if let Some(otherwise) = data.else_statement() {
                    self.return_statements(otherwise, env, when_false, returns, bare)?
                } else {
                    (true, when_false)
                };
                match (true_reachable, false_reachable) {
                    (false, false) => Ok((false, flow)),
                    (true, false) => Ok((true, true_flow)),
                    (false, true) => Ok((true, false_flow)),
                    (true, true) => {
                        let mut joined = flow;
                        for (declaration, true_type) in true_flow.0 {
                            if let Some(false_type) = false_flow.0.get(&declaration) {
                                joined.0.insert(
                                    declaration,
                                    self.checker.graph.union(&[true_type, false_type.clone()])?,
                                );
                            }
                        }
                        Ok((true, joined))
                    }
                }
            }
            NodeDataRead::ThrowStatement(_) => Ok((false, flow)),
            _ if node.kind() == K::EmptyStatement => Ok((true, flow)),
            _ => missing(&format!(
                "return inference statement {}",
                node.kind_string()
            )),
        }
    }

    fn narrow_condition(
        self: &Rc<Self>,
        condition: NodeId,
        env: &Environment,
        flow: &FlowTypes,
    ) -> Result<(FlowTypes, FlowTypes), Error> {
        let node = self.input.node(condition)?;
        if let NodeDataRead::ParenthesizedExpression(data) = node.data() {
            return self.narrow_condition(
                data.expression().ok_or(Error::ResolutionFailed)?,
                env,
                flow,
            );
        }
        let NodeDataRead::BinaryExpression(data) = node.data() else {
            return missing("return flow condition");
        };
        let operator = self
            .input
            .node(data.operator_token().ok_or(Error::ResolutionFailed)?)?
            .kind();
        let equal = matches!(
            operator.known(),
            Some(K::EqualsEqualsEqualsToken | K::EqualsEqualsToken)
        );
        if !equal
            && !matches!(
                operator.known(),
                Some(K::ExclamationEqualsEqualsToken | K::ExclamationEqualsToken)
            )
        {
            return missing("return flow comparison operator");
        }
        let mut left = data.left().ok_or(Error::ResolutionFailed)?;
        let mut right = data.right().ok_or(Error::ResolutionFailed)?;
        if self.nullish_constant(right)?.is_none() {
            std::mem::swap(&mut left, &mut right);
        }
        let mut nullable = self.nullish_constant(right)?.ok_or_else(|| {
            Error::Unsupported("flow comparison is not a nullish equality".into())
        })?;
        if matches!(
            operator.known(),
            Some(K::EqualsEqualsToken | K::ExclamationEqualsToken)
        ) {
            nullable = tf::NULLABLE;
        }
        let group = self
            .input
            .resolve_value_name(left)?
            .ok_or(Error::ResolutionFailed)?;
        let declarations = self.declarations(&group)?;
        let declaration = *declarations.first().ok_or(Error::ResolutionFailed)?;
        let original = if let Some(ty) = flow.0.get(&declaration) {
            ty.clone()
        } else {
            self.value_type_of_symbol(&group, env)?
        };
        if original.flags & (tf::ANY | tf::UNKNOWN) != 0 {
            return missing("nullish flow narrowing of any or unknown");
        }
        let parts = if original.flags & tf::UNION != 0 {
            original.types()?
        } else {
            vec![original]
        };
        let mut matching = Vec::new();
        let mut other = Vec::new();
        for part in parts {
            if part.flags & nullable != 0 {
                matching.push(part);
            } else {
                other.push(part);
            }
        }
        let matching = self.checker.graph.union(&matching)?;
        let other = self.checker.graph.union(&other)?;
        let mut true_flow = flow.clone();
        let mut false_flow = flow.clone();
        true_flow.0.insert(
            declaration,
            if equal {
                matching.clone()
            } else {
                other.clone()
            },
        );
        false_flow
            .0
            .insert(declaration, if equal { other } else { matching });
        Ok((true_flow, false_flow))
    }

    fn nullish_constant(&self, node: NodeId) -> Result<Option<u32>, Error> {
        let read = self.input.node(node)?;
        if read.kind() == K::NullKeyword {
            return Ok(Some(tf::NULL));
        }
        if read.kind() == K::Identifier
            && self.text(node)? == b"undefined"
            && self.input.resolve_value_name(node)?.is_none()
        {
            return Ok(Some(tf::UNDEFINED));
        }
        Ok(None)
    }

    fn expression_type(
        self: &Rc<Self>,
        node: NodeId,
        env: &Environment,
        flow: &FlowTypes,
    ) -> Result<Rc<TypeCell>, Error> {
        let read = self.input.node(node)?;
        match read.kind().known() {
            Some(
                K::NumericLiteral
                | K::StringLiteral
                | K::NoSubstitutionTemplateLiteral
                | K::BigIntLiteral
                | K::TrueKeyword
                | K::FalseKeyword
                | K::PrefixUnaryExpression,
            ) => self.checker.graph.fresh_literal(&self.literal(node)?),
            Some(K::NullKeyword) => self.builtin(tf::NULL),
            Some(K::Identifier) => {
                let Some(group) = self.input.resolve_value_name(node)? else {
                    return if self.text(node)? == b"undefined" {
                        self.builtin(tf::UNDEFINED)
                    } else {
                        missing("unresolved value identifier")
                    };
                };
                for declaration in self.declarations(&group)? {
                    if let Some(ty) = flow.0.get(&declaration) {
                        return Ok(ty.clone());
                    }
                }
                self.value_type_of_symbol(&group, env)
            }
            Some(K::ParenthesizedExpression) => self.expression_type(
                read.data_source()
                    .as_parenthesized_expression()
                    .and_then(|data| data.expression())
                    .ok_or(Error::ResolutionFailed)?,
                env,
                flow,
            ),
            Some(K::PropertyAccessExpression) => {
                let data = read.data_source();
                let data = data
                    .as_property_access_expression()
                    .ok_or(Error::ResolutionFailed)?;
                let receiver = self.expression_type(
                    data.expression().ok_or(Error::ResolutionFailed)?,
                    env,
                    flow,
                )?;
                let name = self.name(data.name().ok_or(Error::ResolutionFailed)?)?;
                self.property_value_type(&receiver, &name)
            }
            Some(K::AsExpression | K::TypeAssertionExpression) => {
                self.type_node(read.type_node().ok_or(Error::ResolutionFailed)?, env, None)
            }
            _ => missing(&format!("return expression {}", read.kind_string())),
        }
    }

    fn property_value_type(
        &self,
        receiver: &Rc<TypeCell>,
        name: &str,
    ) -> Result<Rc<TypeCell>, Error> {
        if receiver.flags & tf::ANY != 0 {
            return self.builtin(tf::ANY);
        }
        if receiver.flags & tf::UNION != 0 {
            let values = receiver
                .types()?
                .iter()
                .map(|ty| self.property_value_type(ty, name))
                .collect::<Result<Vec<_>, _>>()?;
            return self.checker.graph.union(&values);
        }
        let wrapper = if receiver.flags & tf::STRING_LIKE != 0 {
            Some("String")
        } else if receiver.flags & tf::NUMBER_LIKE != 0 {
            Some("Number")
        } else if receiver.flags & tf::BOOLEAN_LIKE != 0 {
            Some("Boolean")
        } else {
            None
        };
        let object = if let Some(wrapper) = wrapper {
            self.initialization
                .named
                .borrow()
                .get(wrapper)
                .cloned()
                .ok_or(Error::ResolutionFailed)?
        } else {
            receiver.clone()
        };
        object
            .member(&self.checker.graph, name)?
            .ok_or_else(|| Error::Unsupported(format!("missing value property {name}").into()))?
            .r#type()
    }

    fn widen_return_literal(&self, ty: &Rc<TypeCell>) -> Result<Rc<TypeCell>, Error> {
        if ty.flags & tf::UNION != 0 {
            let widened = ty
                .types()?
                .iter()
                .map(|part| self.widen_return_literal(part))
                .collect::<Result<Vec<_>, _>>()?;
            return self.checker.graph.union(&widened);
        }
        if !ty.fresh {
            return Ok(ty.clone());
        }
        for (literal, primitive) in [
            (tf::STRING_LITERAL, tf::STRING),
            (tf::NUMBER_LITERAL, tf::NUMBER),
            (tf::BIG_INT_LITERAL, tf::BIG_INT),
            (tf::BOOLEAN_LITERAL, tf::BOOLEAN),
        ] {
            if ty.flags & literal != 0 {
                return self.builtin(primitive);
            }
        }
        Ok(ty.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bound::BoundChecker;
    use crate::bound_input::BoundInput;
    use crate::bound_input::BoundInputOptions;
    use tsr_ast::SourceFileParseOptions;
    use tsr_core::ScriptKind;
    use tsr_jsstring::{JsString, SourceText};

    fn fixture(text: &[u8], kind: ScriptKind) -> (BoundChecker, NodeId) {
        let parsed = tsr_parser::parse_source_file(
            SourceText::from_loaded_bytes(text),
            kind,
            SourceFileParseOptions {
                file_name: JsString::from_bytes(if kind == ScriptKind::JS {
                    b"/value.js".as_slice()
                } else {
                    b"/value.ts".as_slice()
                }),
                ..Default::default()
            },
        );
        let file = tsr_binder::bind_parsed_file(parsed).unwrap();
        let source = file.source();
        let input = BoundInput::new(
            vec![file],
            BoundInputOptions {
                strict_null_checks: true,
                ..Default::default()
            },
            Vec::new(),
        )
        .unwrap();
        (BoundChecker::new(input).unwrap(), source)
    }

    #[test]
    fn computed_properties_follow_bound_unique_symbol_identity() {
        let (checker, source) = fixture(b"interface SymbolConstructor { readonly iterator: unique symbol } declare const Symbol: SymbolConstructor; interface OtherConstructor { readonly iterator: unique symbol } declare const Other: OtherConstructor; type A = { [Symbol.iterator]: number }; type B = { [Symbol.iterator]: string }; type C = { [Other.iterator]: number };", ScriptKind::TS);
        let properties = [b"A".as_slice(), b"B", b"C"].map(|name| {
            let declaration = checker
                .input()
                .declaration_by_name(source, name)
                .unwrap()
                .unwrap();
            let ty = checker.declared_type(declaration).unwrap();
            ty.members(&checker.checker().graph).unwrap()[0].clone()
        });
        assert_eq!(properties[0].name, properties[1].name);
        assert_ne!(properties[0].name, properties[2].name);
        let first = properties[0].name_type.as_ref().unwrap().resolve().unwrap();
        let second = properties[1].name_type.as_ref().unwrap().resolve().unwrap();
        assert!(Rc::ptr_eq(&first, &second));
        assert_eq!(first.flags, tf::UNIQUE_ES_SYMBOL);
    }

    #[test]
    fn infers_return_after_nullish_early_return_from_real_string_declaration() {
        let (checker, source) = fixture(b"interface String { readonly length: number } function f(x: string | undefined) { if (x === undefined) return 0; return x.length; }", ScriptKind::TS);
        let function = checker
            .input()
            .declaration_by_name(source, b"f")
            .unwrap()
            .unwrap();
        let result = checker
            .state
            .return_type_of_declaration(function, &Environment::default())
            .unwrap();
        assert_eq!(result.flags(), tf::NUMBER);
        let again = checker
            .state
            .return_type_of_declaration(function, &Environment::default())
            .unwrap();
        assert!(Rc::ptr_eq(&result, &again));
    }

    #[test]
    fn reads_lazy_jsdoc_parameter_and_return_nodes() {
        let (checker, source) = fixture(b"/** @param {string} value @returns {number} */ export function length(value) { return value.length; }", ScriptKind::JS);
        let function = checker
            .input()
            .declaration_by_name(source, b"length")
            .unwrap()
            .unwrap();
        let result = checker
            .state
            .return_type_of_declaration(function, &Environment::default())
            .unwrap();
        assert_eq!(result.flags(), tf::NUMBER);
        let parameters = checker
            .state
            .list(
                function,
                checker.input().node(function).unwrap().parameter_list(),
            )
            .unwrap();
        let parameter = checker
            .state
            .parameter_type_of_declaration(parameters[0], &Environment::default())
            .unwrap();
        assert_eq!(parameter.flags(), tf::STRING);
    }

    #[test]
    fn unsupported_flow_remains_an_error_and_retry_is_stable() {
        let (checker, source) = fixture(
            b"function f(x: number) { while (x) { return x; } }",
            ScriptKind::TS,
        );
        let function = checker
            .input()
            .declaration_by_name(source, b"f")
            .unwrap()
            .unwrap();
        let first = checker
            .state
            .return_type_of_declaration(function, &Environment::default())
            .unwrap_err();
        let second = checker
            .state
            .return_type_of_declaration(function, &Environment::default())
            .unwrap_err();
        assert_eq!(first, second);
        assert!(
            matches!(first, Error::Unsupported(message) if message.contains("return inference statement"))
        );
    }
}
