//! Diagnostic rules shared by expression operators and calls.
use crate::{type_facts as f, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::{JsString, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    // port: tsc/internal/checker/utilities.go:entityNameToString
    // port: tsc/internal/ast/utilities.go:EntityNameToString
    pub(crate) fn entity_name_text(&self, node: NodeId) -> Result<JsString, Error> {
        // The explicit stack also covers long property chains on a small stack.
        enum Part {
            Node(NodeId),
            Dot,
        }
        let mut pending = vec![Part::Node(node)];
        let mut bytes = Vec::new();
        while let Some(part) = pending.pop() {
            let Part::Node(node) = part else {
                bytes.push(b'.');
                continue;
            };
            let read = self.ast(node)?.node(node)?;
            match read.kind().known() {
                Some(K::ThisKeyword) => bytes.extend_from_slice(b"this"),
                Some(K::Identifier | K::PrivateIdentifier) => {
                    let text = if read.pos() < 0 {
                        self.ast(node)?.node_text(node)?.into_js_string()
                    } else {
                        ts_scanner::get_text_of_node(self.ast(node)?, node)?
                    };
                    bytes.extend_from_slice(text.as_bytes());
                }
                Some(K::QualifiedName | K::PropertyAccessExpression) => {
                    let (left, right) = if let Some(data) = read.data_source().as_qualified_name() {
                        (data.left(), data.right())
                    } else {
                        (read.expression(), read.name())
                    };
                    pending.push(Part::Node(right.ok_or(Error::MissingLink("entity right"))?));
                    pending.push(Part::Dot);
                    pending.push(Part::Node(left.ok_or(Error::MissingLink("entity left"))?));
                }
                _ => return Err(Error::Unsupported("entityNameToString: node kind")),
            }
        }
        Ok(JsString::from_bytes(bytes))
    }

    // port: tsc/internal/checker/checker.go:Checker.checkNonNullType
    pub(crate) fn check_non_null_type(
        &mut self,
        ty: TypeId,
        node: NodeId,
    ) -> Result<TypeId, Error> {
        self.check_non_null_type_with_reporter(ty, node, false)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkNonNullTypeWithReporter
    pub(crate) fn check_non_null_type_with_reporter(
        &mut self,
        ty: TypeId,
        node: NodeId,
        invocation: bool,
    ) -> Result<TypeId, Error> {
        if self.options.strict_null_checks && self.types.flags(ty)? & tf::UNKNOWN != 0 {
            if ts_ast::is_entity_name_expression(self.ast(node)?, node)? {
                let text = self.entity_name_text(node)?;
                if text.len() < 100 {
                    self.error_at(Some(node), d::X_0_is_of_type_unknown, vec![text])?;
                    return Ok(self.builtins.error_type);
                }
            }
            self.error_at(Some(node), d::Object_is_of_type_unknown, vec![])?;
            return Ok(self.builtins.error_type);
        }
        let facts = self.type_facts(ty, f::IS_UNDEFINED_OR_NULL)?;
        if facts == 0 {
            return Ok(ty);
        }
        if invocation {
            let message = if facts & f::IS_UNDEFINED != 0 {
                if facts & f::IS_NULL != 0 {
                    d::Cannot_invoke_an_object_which_is_possibly_null_or_undefined
                } else {
                    d::Cannot_invoke_an_object_which_is_possibly_undefined
                }
            } else {
                d::Cannot_invoke_an_object_which_is_possibly_null
            };
            self.error_at(Some(node), message, vec![])?;
        } else {
            self.report_possibly_null_or_undefined(node, facts)?;
        }
        let result = self.non_nullable_type(ty)?;
        Ok(
            if self.types.flags(result)? & (tf::NULLABLE | tf::NEVER) != 0 {
                self.builtins.error_type
            } else {
                result
            },
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.reportObjectPossiblyNullOrUndefinedError
    fn report_possibly_null_or_undefined(&mut self, node: NodeId, facts: u32) -> Result<(), Error> {
        let text = if ts_ast::is_entity_name_expression(self.ast(node)?, node)? {
            self.entity_name_text(node)?
        } else {
            JsString::default()
        };
        let kind = self.ast(node)?.node(node)?.kind();
        let named = !text.is_empty() && text.len() < 100;
        let (message, args) = if kind == K::NullKeyword
            || named && kind == K::Identifier && text.as_bytes() == b"undefined"
        {
            (
                d::The_value_0_cannot_be_used_here,
                vec![if kind == K::NullKeyword {
                    JsString::from_bytes(b"null".as_slice())
                } else {
                    text
                }],
            )
        } else {
            let message = if facts & f::IS_UNDEFINED != 0 {
                if facts & f::IS_NULL != 0 {
                    if named {
                        d::X_0_is_possibly_null_or_undefined
                    } else {
                        d::Object_is_possibly_null_or_undefined
                    }
                } else if named {
                    d::X_0_is_possibly_undefined
                } else {
                    d::Object_is_possibly_undefined
                }
            } else if named {
                d::X_0_is_possibly_null
            } else {
                d::Object_is_possibly_null
            };
            (message, if named { vec![text] } else { vec![] })
        };
        self.error_at(Some(node), message, args)?;
        Ok(())
    }
}
