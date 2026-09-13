//! Type and symbol display through synthetic AST nodes and `ts_printer`
//! (`tsc/internal/checker/printer.go`). The builder cache and emit context are
//! reused across diagnostic calls. Completed cache entries retain their AST
//! frames; uncached request output is released. Returned bytes own no AST.

use crate::{type_format_flags, CheckerState, Error, TypeAlias, TypeFormatFlags, TypeId};
use ts_arena::SymbolId;
use ts_ast::JsString;
use ts_printer::{EmitTextWriter, Printer, PrinterOptions, SingleLineStringWriter, TextWriter};

// Defaults used by the pinned Checker.TypeToString entry point.
pub(crate) const DEFAULT_FLAGS: TypeFormatFlags = type_format_flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
    | type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE;

impl CheckerState {
    // port: tsc/internal/checker/printer.go:Checker.symbolToStringEx
    pub(crate) fn symbol_to_string_at(
        &mut self,
        symbol: SymbolId,
        enclosing: Option<ts_arena::NodeId>,
        meaning: ts_ast::SymbolFlags,
        flags: crate::SymbolFormatFlags,
    ) -> Result<JsString, Error> {
        use crate::symbol_format_flags as sf;
        use ts_nodebuilder::{flags as nf, internal_flags as inf};
        let mut node_flags = nf::IGNORE_ERRORS;
        for (source, target) in [
            (
                sf::USE_ONLY_EXTERNAL_ALIASING,
                nf::USE_ONLY_EXTERNAL_ALIASING,
            ),
            (
                sf::WRITE_TYPE_PARAMETERS_OR_ARGUMENTS,
                nf::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME,
            ),
            (
                sf::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
                nf::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
            ),
        ] {
            if flags & source != 0 {
                node_flags |= target;
            }
        }
        let mut internal = inf::NONE;
        if flags & sf::DO_NOT_INCLUDE_SYMBOL_CHAIN != 0 {
            internal |= inf::DO_NOT_INCLUDE_SYMBOL_CHAIN;
        }
        if flags & sf::WRITE_COMPUTED_PROPS != 0 {
            internal |= inf::WRITE_COMPUTED_PROPS;
        }
        let source = enclosing
            .map(|node| {
                ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))
                    .map_err(Error::from)
            })
            .transpose()?
            .flatten();
        let never_ascii_escape = enclosing
            .map(|node| {
                self.ast(node)?
                    .node(node)
                    .map(|n| n.kind() == ts_ast::SyntaxKind::SourceFile)
                    .map_err(Error::from)
            })
            .transpose()?
            .unwrap_or(false);
        crate::node_builder::NodeBuilder::with_cached(self, node_flags, |builder| {
            builder.prepare_context(enclosing, node_flags, internal)?;
            let node = builder.symbol_display_node(
                symbol,
                meaning,
                flags & sf::ALLOW_ANY_NODE_KIND != 0,
            )?;
            if builder.encountered_error {
                return Ok(JsString::from_bytes(b"".as_slice()));
            }
            let printer = Printer::new(
                PrinterOptions {
                    remove_comments: true,
                    omit_trailing_semicolon: true,
                    never_ascii_escape,
                    ..Default::default()
                },
                &builder.emit,
            );
            let mut writer = SingleLineStringWriter::new();
            printer.write(builder.ast.view(), node, source, &mut writer)?;
            Ok(JsString::from_bytes(writer.text().to_vec()))
        })
    }

    pub(crate) fn type_to_string(
        &mut self,
        ty: TypeId,
        flags: TypeFormatFlags,
    ) -> Result<JsString, Error> {
        self.type_to_string_at(ty, None, flags)
    }

    // port: tsc/internal/checker/printer.go:Checker.typeToStringEx
    pub(crate) fn type_to_string_at(
        &mut self,
        ty: TypeId,
        enclosing: Option<ts_arena::NodeId>,
        flags: TypeFormatFlags,
    ) -> Result<JsString, Error> {
        if self.serialization_level >= MAX_SERIALIZATION_LEVEL as u32 {
            return Ok(JsString::from_bytes(b"?".as_slice()));
        }
        let no_truncation = flags & type_format_flags::NO_TRUNCATION != 0
            || self
                .program
                .as_ref()
                .is_some_and(|program| program.host.options().no_error_truncation.is_true());
        let mut combined = to_node_builder_flags(flags) | ts_nodebuilder::flags::IGNORE_ERRORS;
        if no_truncation {
            combined |= ts_nodebuilder::flags::NO_TRUNCATION;
        }
        let source = enclosing
            .map(|node| {
                ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))
                    .map_err(Error::from)
            })
            .transpose()?
            .flatten();
        crate::node_builder::NodeBuilder::with_cached(self, combined, |builder| {
            builder.prepare_context(enclosing, combined, ts_nodebuilder::internal_flags::NONE)?;
            builder.checker.serialization_level += 1;
            let node = builder.type_node(ty);
            builder.checker.serialization_level -= 1;
            let node = node?;
            let printer = Printer::new(
                PrinterOptions {
                    remove_comments: true,
                    ..Default::default()
                },
                &builder.emit,
            );
            let newline = if flags & type_format_flags::MULTILINE_OBJECT_LITERALS != 0 {
                b"\n".as_slice()
            } else {
                b"".as_slice()
            };
            let mut writer = TextWriter::new(newline, 0);
            printer.write(builder.ast.view(), node, source, &mut writer)?;
            let maximum = if no_truncation {
                NO_TRUNCATION_MAXIMUM_TRUNCATION_LENGTH
            } else {
                DEFAULT_MAXIMUM_TRUNCATION_LENGTH
            } * 2;
            let mut text = writer.text().to_vec();
            if !text.is_empty() && text.len() >= maximum {
                text.truncate(maximum - 3);
                text.extend_from_slice(b"...");
            }
            Ok(JsString::from_bytes(text))
        })
    }

    // port: tsc/internal/checker/printer.go:Checker.signatureToStringEx
    pub(crate) fn signature_to_string(
        &mut self,
        signature: crate::SignatureId,
    ) -> Result<JsString, Error> {
        let construct =
            self.signatures.get(signature)?.flags & crate::signature_flags::CONSTRUCT != 0;
        crate::node_builder::NodeBuilder::with_cached(
            self,
            ts_nodebuilder::flags::IGNORE_ERRORS
                | ts_nodebuilder::flags::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME,
            |builder| {
                let node = builder.signature_node(
                    signature,
                    if construct {
                        ts_ast::SyntaxKind::ConstructSignature
                    } else {
                        ts_ast::SyntaxKind::CallSignature
                    },
                    None,
                    None,
                )?;
                let printer = Printer::new(
                    PrinterOptions {
                        remove_comments: true,
                        omit_trailing_semicolon: true,
                        never_ascii_escape: true,
                        ..Default::default()
                    },
                    &builder.emit,
                );
                let mut writer = SingleLineStringWriter::new();
                printer.write(builder.ast.view(), node, None, &mut writer)?;
                Ok(JsString::from_bytes(writer.text().to_vec()))
            },
        )
    }

    // port: tsc/internal/checker/printer.go:Checker.typePredicateToString
    pub(crate) fn type_predicate_to_string(
        &mut self,
        predicate: crate::TypePredicateId,
    ) -> Result<JsString, Error> {
        crate::node_builder::NodeBuilder::with_cached(
            self,
            ts_nodebuilder::flags::IGNORE_ERRORS
                | ts_nodebuilder::flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
            |builder| {
                let node = builder.predicate_node(predicate)?;
                let printer = Printer::new(
                    PrinterOptions {
                        remove_comments: true,
                        omit_trailing_semicolon: true,
                        never_ascii_escape: true,
                        ..Default::default()
                    },
                    &builder.emit,
                );
                let mut writer = SingleLineStringWriter::new();
                printer.write(builder.ast.view(), node, None, &mut writer)?;
                Ok(JsString::from_bytes(writer.text().to_vec()))
            },
        )
    }

    // port: tsc/internal/checker/printer.go:Checker.symbolToString
    pub(crate) fn symbol_to_string(&mut self, symbol: SymbolId) -> Result<JsString, Error> {
        self.symbol_to_string_without_chain(symbol, None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getFullyQualifiedName
    pub(crate) fn fully_qualified_name(
        &mut self,
        symbol: SymbolId,
        location: Option<ts_arena::NodeId>,
    ) -> Result<JsString, Error> {
        if let Some(parent) = self.symbol(symbol)?.parent() {
            let parent = self.fully_qualified_name(parent, location)?;
            let name = self.symbol_to_string(symbol)?;
            let mut text = parent.as_bytes().to_vec();
            text.push(b'.');
            text.extend_from_slice(name.as_bytes());
            return Ok(JsString::from_bytes(text));
        }
        self.symbol_to_string_without_chain(symbol, location)
    }

    fn symbol_to_string_without_chain(
        &mut self,
        symbol: SymbolId,
        location: Option<ts_arena::NodeId>,
    ) -> Result<JsString, Error> {
        crate::node_builder::NodeBuilder::with_cached(
            self,
            ts_nodebuilder::flags::IGNORE_ERRORS,
            |builder| {
                let node = builder.symbol_expression_without_chain(symbol, location)?;
                let printer = Printer::new(
                    PrinterOptions {
                        remove_comments: true,
                        omit_trailing_semicolon: true,
                        ..Default::default()
                    },
                    &builder.emit,
                );
                let mut writer = SingleLineStringWriter::new();
                printer.write(builder.ast.view(), node, None, &mut writer)?;
                Ok(JsString::from_bytes(writer.text().to_vec()))
            },
        )
    }
}

/// Nested type serialization (diagnostics requesting types requesting members)
/// returns `"?"` beyond this depth (`checker.go`, `maxSerializationLevel`).
pub const MAX_SERIALIZATION_LEVEL: i32 = 2;
/// `nodebuilderimpl.go`, `defaultMaximumTruncationLength`.
pub const DEFAULT_MAXIMUM_TRUNCATION_LENGTH: usize = 160;
/// `nodebuilderimpl.go`, `noTruncationMaximumTruncationLength`.
pub const NO_TRUNCATION_MAXIMUM_TRUNCATION_LENGTH: usize = 1_000_000;

/// The bits of `TypeFormatFlags` that are node-builder flags at the same positions.
// port: tsc/internal/checker/printer.go:toNodeBuilderFlags
pub fn to_node_builder_flags(flags: TypeFormatFlags) -> ts_nodebuilder::Flags {
    flags & type_format_flags::NODE_BUILDER_FLAGS_MASK
}

/// Upstream's nil-tolerant accessor: an absent alias has no symbol.
// port: tsc/internal/checker/types.go:TypeAlias.Symbol
pub(crate) fn alias_symbol(alias: Option<&TypeAlias>) -> Option<SymbolId> {
    alias.map(|alias| alias.symbol)
}

/// Upstream's nil-tolerant accessor: an absent alias has no type arguments.
// port: tsc/internal/checker/types.go:TypeAlias.TypeArguments
pub(crate) fn alias_type_arguments(alias: Option<&TypeAlias>) -> &[TypeId] {
    alias.map_or(&[], |alias| &alias.type_arguments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{object_flags, CheckerOptions};
    use std::sync::Arc;
    use ts_arena::{CheckerIdentity, Counters, Generation};
    use ts_ast::{check_flags, symbol_flags, SymbolTable};

    fn checker() -> (CheckerState, Counters) {
        let counters = Counters::new();
        let identity = CheckerIdentity::new(Generation::new(&counters), &counters);
        let state = CheckerState::new(
            &identity,
            &counters,
            CheckerOptions {
                strict_null_checks: true,
                exact_optional_property_types: true,
            },
        )
        .unwrap();
        (state, counters)
    }

    #[test]
    fn literal_display_uses_the_printer_for_escaping_and_preserves_freshness() {
        let (mut state, counters) = checker();
        let literal = state
            .get_string_literal_type(JsString::from_bytes("hé\n\"".as_bytes()))
            .unwrap();
        let fresh = state.get_fresh_type_of_literal_type(literal).unwrap();
        let number = state
            .get_number_literal_type(ts_jsnum::Number::new(-42.0))
            .unwrap();
        let bigint = state
            .get_big_int_literal_type(ts_jsnum::PseudoBigInt::new(b"42", true))
            .unwrap();
        let before = counters.snapshot();
        for ty in [literal, fresh] {
            assert_eq!(
                state.type_to_string(ty, 0).unwrap().as_bytes(),
                "\"hé\\n\\\"\"".as_bytes()
            );
            assert_eq!(
                state
                    .type_to_string(
                        ty,
                        type_format_flags::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE
                    )
                    .unwrap()
                    .as_bytes(),
                "'hé\\n\"'".as_bytes()
            );
        }
        assert_eq!(state.type_to_string(number, 0).unwrap().as_bytes(), b"-42");
        assert_eq!(state.type_to_string(bigint, 0).unwrap().as_bytes(), b"-42n");
        assert_eq!(
            state
                .type_to_string(state.builtins.boolean_type, 0)
                .unwrap()
                .as_bytes(),
            b"boolean"
        );
        assert_eq!(
            counters.snapshot(),
            before,
            "display releases its synthetic AST"
        );
    }

    #[test]
    fn alias_expansion_preserves_optional_readonly_and_quoted_property_names() {
        let (mut state, _) = checker();
        let property = state
            .new_symbol(
                symbol_flags::PROPERTY | symbol_flags::OPTIONAL,
                JsString::from_bytes(b"field-name".as_slice()),
            )
            .unwrap();
        state.symbol_mut(property).unwrap().check_flags |= check_flags::READONLY;
        let ty = state
            .get_union_type(&[state.builtins.number_type, state.builtins.missing_type])
            .unwrap();
        state
            .value_symbol_links
            .get_or_default(property)
            .resolved_type = Some(ty);
        let members = state.alloc_symbol_table(SymbolTable::from([(
            JsString::from_bytes(b"field-name".as_slice()),
            Some(property),
        )]));
        let object = state
            .new_anonymous_type(None, Some(members), &[], &[], &[])
            .unwrap();
        let alias = state
            .new_symbol(
                symbol_flags::TYPE_ALIAS,
                JsString::from_bytes(b"Shape".as_slice()),
            )
            .unwrap();
        let alias = state
            .types
            .push_alias(TypeAlias {
                symbol: alias,
                type_arguments: Arc::from([]),
            })
            .unwrap();
        state.types.get_mut(object).unwrap().alias = Some(alias);
        assert_eq!(
            state.type_to_string(object, 0).unwrap().as_bytes(),
            b"Shape"
        );
        assert_eq!(
            state
                .type_to_string(object, type_format_flags::IN_TYPE_ALIAS)
                .unwrap()
                .as_bytes(),
            b"{ readonly \"field-name\"?: number; }"
        );

        let other = state
            .new_anonymous_type(None, Some(members), &[], &[], &[])
            .unwrap();
        let other_symbol = state
            .new_symbol(
                symbol_flags::TYPE_ALIAS,
                JsString::from_bytes(b"Shape".as_slice()),
            )
            .unwrap();
        let other_alias = state
            .types
            .push_alias(TypeAlias {
                symbol: other_symbol,
                type_arguments: Arc::from([]),
            })
            .unwrap();
        state.types.get_mut(other).unwrap().alias = Some(other_alias);
        let union = state.get_union_type(&[object, other]).unwrap();
        // Distinct aliases spelled the same are regenerated fully qualified;
        // these synthetic aliases have no containers, so the spelling repeats.
        assert_eq!(
            state.type_to_string(union, 0).unwrap().as_bytes(),
            b"Shape | Shape"
        );
        // The source permits duplicate spellings when both types share the
        // same alias record; only distinct references need qualification.
        state.types.get_mut(other).unwrap().alias = Some(alias);
        assert_eq!(
            state.type_to_string(union, 0).unwrap().as_bytes(),
            b"Shape | Shape"
        );
        state.types.get_mut(object).unwrap().object_flags |=
            object_flags::FRESH_LITERAL | object_flags::OBJECT_LITERAL;
        assert_eq!(
            state
                .type_to_string(object, type_format_flags::IN_TYPE_ALIAS)
                .unwrap()
                .as_bytes(),
            b"{ readonly \"field-name\"?: number; }"
        );
    }

    #[test]
    fn display_limits_match_source_byte_boundaries_and_errors_restore_serialization_level() {
        let (mut state, _) = checker();
        for (length, expected_length) in [(317, 319), (318, 320)] {
            let literal = state
                .get_string_literal_type(JsString::from_bytes(vec![b'x'; length]))
                .unwrap();
            let display = state.type_to_string(literal, 0).unwrap();
            assert_eq!(display.len(), expected_length);
            assert_eq!(display.as_bytes().ends_with(b"..."), length == 318);
            let untruncated = state
                .type_to_string(literal, type_format_flags::NO_TRUNCATION)
                .unwrap();
            assert_eq!(untruncated.len(), length + 2);
            assert!(untruncated.as_bytes().ends_with(b"\""));
        }
        state.serialization_level = MAX_SERIALIZATION_LEVEL as u32;
        assert_eq!(
            state
                .type_to_string(state.builtins.number_type, 0)
                .unwrap()
                .as_bytes(),
            b"?"
        );
        assert_eq!(state.serialization_level, MAX_SERIALIZATION_LEVEL as u32);
        state.serialization_level = 0;
        // The `/*unresolved*/` synthetic comment is dropped by the comment-free
        // display printer, leaving the `any` keyword.
        assert_eq!(
            state
                .type_to_string(state.builtins.unresolved_type, 0)
                .unwrap()
                .as_bytes(),
            b"any"
        );
        assert_eq!(state.serialization_level, 0);
        assert_eq!(
            state
                .type_to_string(state.builtins.number_type, 0)
                .unwrap()
                .as_bytes(),
            b"number"
        );
    }

    #[test]
    fn object_elision_starts_above_160_and_preserves_the_last_property() {
        for length in [153, 154] {
            let (mut state, _) = checker();
            let first_name = "a".repeat(length);
            let mut properties = Vec::new();
            for name in [first_name.as_str(), "b", "c", "d", "e", "z"] {
                let symbol = state
                    .new_symbol(
                        symbol_flags::PROPERTY,
                        JsString::from_bytes(name.as_bytes()),
                    )
                    .unwrap();
                state
                    .value_symbol_links
                    .get_or_default(symbol)
                    .resolved_type = Some(state.builtins.number_type);
                properties.push(symbol);
            }
            let object = state.new_anonymous_type(None, None, &[], &[], &[]).unwrap();
            state.types.structured_mut(object).unwrap().properties = Some(properties.into());
            let expected = if length == 154 {
                format!("{{ {first_name}: number; ... 4 more ...; z: number; }}")
            } else {
                format!("{{ {first_name}: number; b: number; c: number; d: number; e: number; z: number; }}")
            };
            assert_eq!(
                state.type_to_string(object, 0).unwrap().as_bytes(),
                expected.as_bytes()
            );
        }
    }
}
