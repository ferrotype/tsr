//! Known-truthy warnings inspect uses in the condition and its body before reporting.
use crate::{type_facts as f, type_flags as tf, CheckerState, Error, LiteralValue, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{symbol_flags as sf, JsString, SyntaxKind as K};
use ts_diagnostics as d;
fn required<T>(value: Option<T>, name: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(name))
}
impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkTestingKnownTruthyCallableOrAwaitableOrEnumMemberType
    pub(crate) fn check_known_truthy_guard(
        &mut self,
        node: NodeId,
        ty: TypeId,
        body: Option<NodeId>,
    ) -> Result<(), Error> {
        if self.options.strict_null_checks {
            self.known_truthy_types(node, ty, body)?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkTestingKnownTruthyTypes
    fn known_truthy_types(
        &mut self,
        mut node: NodeId,
        ty: TypeId,
        body: Option<NodeId>,
    ) -> Result<(), Error> {
        node = self.truthy_skip_parentheses(node)?;
        loop {
            self.known_truthy_type(node, ty, body)?;
            let read = self.ast(node)?.node(node)?;
            let Some(data) = read.data_source().as_binary_expression() else {
                break;
            };
            if !matches!(
                self.ast(node)?
                    .node(required(data.operator_token(), "truthy operator")?)?
                    .kind()
                    .known(),
                Some(K::BarBarToken | K::QuestionQuestionToken)
            ) {
                break;
            }
            node = self.truthy_skip_parentheses(required(data.left(), "truthy left")?)?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.checkTestingKnownTruthyType
    fn known_truthy_type(
        &mut self,
        condition: NodeId,
        ty: TypeId,
        body: Option<NodeId>,
    ) -> Result<(), Error> {
        let mut location = condition;
        if ts_ast::utilities::is_logical_or_coalescing_binary_expression(
            self.ast(location)?,
            location,
        )? {
            let right = required(
                self.ast(location)?
                    .node(location)?
                    .data_source()
                    .as_binary_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .right(),
                "truthy right",
            )?;
            location = self.truthy_skip_parentheses(right)?;
        }
        if ts_ast::is_module_exports_access_expression(self.ast(location)?, location)? {
            return Ok(());
        }
        if ts_ast::utilities::is_logical_or_coalescing_binary_expression(
            self.ast(location)?,
            location,
        )? {
            return self.known_truthy_types(location, ty, body);
        }
        let ty = if location == condition {
            ty
        } else {
            self.check_expression(location)?
        };
        let read = self.ast(location)?.node(location)?;
        let property = read.kind() == K::PropertyAccessExpression;
        let receiver = if property { read.expression() } else { None };
        if self.types.flags(ty)? & tf::ENUM_LITERAL != 0 && property {
            let receiver = required(receiver, "truthy enum receiver")?;
            let symbol = self
                .query
                .resolved_symbols
                .try_get(receiver)
                .copied()
                .flatten()
                .unwrap_or(self.builtins.unknown_symbol);
            if self.symbol(symbol)?.flags() & sf::ENUM != 0 {
                let truthy = match &self.types.literal(ty)?.value {
                    LiteralValue::String(value) => !value.is_empty(),
                    LiteralValue::Number(value) => value.value() != 0.0 && !value.value().is_nan(),
                    LiteralValue::Boolean(value) => *value,
                    LiteralValue::BigInt(value) => *value != ts_jsnum::PseudoBigInt::default(),
                    LiteralValue::ComputedEnum => return Err(ts_arena::Error::InvalidGraph.into()),
                };
                self.error_at(
                    Some(location),
                    d::This_condition_will_always_return_0,
                    vec![JsString::from_bytes(if truthy {
                        b"true".as_slice()
                    } else {
                        b"false".as_slice()
                    })],
                )?;
                return Ok(());
            }
        }
        let cast = if let Some(receiver) = receiver {
            let receiver = self.truthy_skip_parentheses(receiver)?;
            matches!(
                self.ast(receiver)?.node(receiver)?.kind().known(),
                Some(K::AsExpression | K::TypeAssertionExpression)
            )
        } else {
            false
        };
        if self.type_facts(ty, f::TRUTHY)? == 0 || cast {
            return Ok(());
        }
        let callable = !self.signatures_of_type(ty, false)?.is_empty();
        let promise = self.awaited_type_of_promise(ty)?.is_some();
        if !callable && !promise {
            return Ok(());
        }
        let read = self.ast(location)?.node(location)?;
        let tested_node = if read.kind() == K::Identifier {
            Some(location)
        } else if property {
            read.name()
        } else {
            None
        };
        let symbol = match tested_node {
            Some(node) => self.get_symbol_at_location(node)?,
            None => None,
        };
        if symbol.is_none() && !promise {
            return Ok(());
        }
        let mut used = false;
        if let Some(symbol) = symbol {
            if let Some(parent) = self.ast(condition)?.node(condition)?.parent() {
                if self.ast(parent)?.node(parent)?.kind() == K::BinaryExpression {
                    used = self.symbol_used_binary_chain(parent, symbol)?;
                }
            }
            if !used {
                if let Some(body) = body {
                    used = self.symbol_used_condition_body(
                        condition,
                        body,
                        required(tested_node, "truthy tested name")?,
                        symbol,
                    )?;
                }
            }
        }
        if !used {
            if promise {
                let name =
                    self.type_to_string(ty, crate::type_format_flags::USE_FULLY_QUALIFIED_TYPE)?;
                let index = self.error_at(
                    Some(location),
                    d::This_condition_will_always_return_true_since_this_0_is_always_defined,
                    vec![name],
                )?;
                if let Some(index) = index {
                    let related = self.diagnostic_for_node(
                        Some(location),
                        d::Did_you_forget_to_use_await,
                        vec![],
                    )?;
                    self.add_related_diagnostic(index, related)?;
                }
            } else {
                self.error_at(Some(location),d::This_condition_will_always_return_true_since_this_function_is_always_defined_Did_you_mean_to_call_it_instead,vec![])?;
            }
        }
        Ok(())
    }
    fn truthy_skip_parentheses(&self, mut node: NodeId) -> Result<NodeId, Error> {
        while self.ast(node)?.node(node)?.kind() == K::ParenthesizedExpression {
            node = required(
                self.ast(node)?.node(node)?.expression(),
                "truthy parentheses",
            )?;
        }
        Ok(node)
    }
    // port: tsc/internal/checker/checker.go:Checker.isSymbolUsedInBinaryExpressionChain
    fn symbol_used_binary_chain(
        &mut self,
        mut node: NodeId,
        symbol: SymbolId,
    ) -> Result<bool, Error> {
        loop {
            let read = self.ast(node)?.node(node)?;
            let Some(data) = read.data_source().as_binary_expression() else {
                return Ok(false);
            };
            if self
                .ast(node)?
                .node(required(data.operator_token(), "truthy chain operator")?)?
                .kind()
                != K::AmpersandAmpersandToken
            {
                return Ok(false);
            }
            let right = required(data.right(), "truthy chain right")?;
            let parent = read.parent();
            let mut stack: Vec<_> = self.source_children(right)?.into_iter().rev().collect();
            while let Some(child) = stack.pop() {
                if self.ast(child)?.node(child)?.kind() == K::Identifier
                    && self.get_symbol_at_location(child)? == Some(symbol)
                {
                    return Ok(true);
                }
                let children = self.source_children(child)?;
                stack.extend(children.into_iter().rev());
            }
            let Some(parent) = parent else {
                return Ok(false);
            };
            node = parent;
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.isSymbolUsedInConditionBody
    fn symbol_used_condition_body(
        &mut self,
        condition: NodeId,
        body: NodeId,
        tested: NodeId,
        symbol: SymbolId,
    ) -> Result<bool, Error> {
        let read = self.ast(tested)?.node(tested)?;
        let tested_parent = read.parent();
        let simple = self.ast(condition)?.node(condition)?.kind() == K::Identifier
            || read.kind() == K::Identifier
                && tested_parent
                    .map(|p| {
                        self.ast(p)?
                            .node(p)
                            .map(|n| n.kind() == K::BinaryExpression)
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .unwrap_or(false);
        let mut stack: Vec<_> = self.source_children(body)?.into_iter().rev().collect();
        while let Some(child) = stack.pop() {
            if self.ast(child)?.node(child)?.kind() == K::Identifier
                && self.get_symbol_at_location(child)? == Some(symbol)
            {
                if simple {
                    return Ok(true);
                }
                if self.same_truthy_target(tested_parent, self.ast(child)?.node(child)?.parent())? {
                    return Ok(true);
                }
            }
            let children = self.source_children(child)?;
            stack.extend(children.into_iter().rev());
        }
        Ok(false)
    }
    fn same_truthy_target(
        &mut self,
        mut tested: Option<NodeId>,
        mut child: Option<NodeId>,
    ) -> Result<bool, Error> {
        while let (Some(a), Some(b)) = (tested, child) {
            let ar = self.ast(a)?.node(a)?;
            let br = self.ast(b)?.node(b)?;
            let ak = ar.kind();
            let bk = br.kind();
            if ak == K::Identifier && bk == K::Identifier
                || ak == K::ThisKeyword && bk == K::ThisKeyword
            {
                return Ok(self.get_symbol_at_location(a)? == self.get_symbol_at_location(b)?);
            }
            if ak == K::PropertyAccessExpression && bk == K::PropertyAccessExpression {
                let an = required(ar.name(), "truthy tested property")?;
                let bn = required(br.name(), "truthy used property")?;
                if self.get_symbol_at_location(an)? != self.get_symbol_at_location(bn)? {
                    return Ok(false);
                }
            } else if !(ak == K::CallExpression && bk == K::CallExpression) {
                return Ok(false);
            }
            tested = self.ast(a)?.node(a)?.expression();
            child = self.ast(b)?.node(b)?.expression();
        }
        Ok(false)
    }
}
