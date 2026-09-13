//! Cache roots, request-frame rotation and metadata retention across display calls.
use super::{NodeBuilder, SerializedKey};
use crate::{CheckerOptions, CheckerState, Error};
use ts_arena::{CheckerIdentity, Counters, Generation};
use ts_ast::{AstBuilder, AstFile, FactoryMethods, JsString, NodeId, SyntaxKind as K};
use ts_jsstring::SourceText;
use ts_printer::{emit_flags, EmitTextWriter, Printer, PrinterOptions, TextWriter};

fn state(counters: &Counters) -> CheckerState {
    let identity = CheckerIdentity::new(Generation::new(counters), counters);
    CheckerState::new(&identity, counters, CheckerOptions::default()).unwrap()
}
fn scope(counters: &Counters) -> AstFile {
    let mut ast = AstBuilder::new(SourceText::from_bytes(b"".as_slice()), counters);
    let root = ast.new_identifier(JsString::from_bytes(b"scope".as_slice()));
    ast.complete(root).unwrap().publish_unbound()
}
fn enter(builder: &mut NodeBuilder<'_>, scope: &AstFile) {
    builder.ast.retain_file(scope.clone());
    builder.enclosing = scope.root();
}
fn print(builder: &NodeBuilder<'_>, node: NodeId) -> Result<JsString, Error> {
    let mut writer = TextWriter::new(b"", 0);
    Printer::new(PrinterOptions::default(), &builder.emit).write(
        builder.ast.view(),
        node,
        None,
        &mut writer,
    )?;
    Ok(JsString::from_bytes(writer.text()))
}

#[test]
fn cached_object_survives_rotation_without_retaining_repeat_output() {
    let counters = Counters::new();
    let initial = counters.snapshot();
    {
        let mut checker = state(&counters);
        let scope = scope(&counters);
        let property = checker
            .new_symbol(
                ts_ast::symbol_flags::PROPERTY,
                JsString::from_bytes(b"a".as_slice()),
            )
            .unwrap();
        checker
            .value_symbol_links
            .get_or_default(property)
            .resolved_type = Some(checker.builtins.number_type);
        let object_symbol = checker
            .new_symbol(
                ts_ast::symbol_flags::TYPE_LITERAL,
                JsString::from_bytes(b"__type".as_slice()),
            )
            .unwrap();
        let object = checker
            .new_anonymous_type(Some(object_symbol), None, &[], &[], &[])
            .unwrap();
        checker.types.structured_mut(object).unwrap().properties = Some(vec![property].into());
        let mut retained = None;
        let mut original = None;
        for _ in 0..8 {
            let text = NodeBuilder::with_cached(&mut checker, 0, |builder| {
                enter(builder, &scope);
                let node = builder.type_node(object)?;
                if let Some(original) = original {
                    assert_ne!(node, original);
                    assert_eq!(builder.emit.most_original(node), original);
                    assert!(builder.ast.view().node(original).is_ok());
                } else {
                    original = Some(node);
                }
                print(builder, node)
            })
            .unwrap();
            assert_eq!(text.as_bytes(), b"{ a: number; }");
            assert_eq!(checker.display_builder.entries.len(), 1);
            assert_eq!(
                checker
                    .display_builder
                    .entries
                    .values()
                    .next()
                    .unwrap()
                    .value
                    .node,
                original.unwrap()
            );
            if let Some(retained) = retained {
                assert_eq!(counters.snapshot(), retained);
            }
            retained = Some(counters.snapshot());
        }
        assert!(checker
            .display_builder
            .type_roots()
            .any(|root| root == object));
        let measured = checker.census(&[]).unwrap();
        assert!(
            measured["families"]["display_cache"]["bytes"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert!(
            measured["families"]["display_ast"]["count"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert!(measured["unavailable"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("display_ast")));
        let cached = checker
            .display_builder
            .entries
            .values()
            .next()
            .unwrap()
            .owner
            .clone();
        drop(scope);
        assert!(cached.view().node(original.unwrap()).is_ok());
        assert!(cached
            .view()
            .node(
                checker
                    .display_builder
                    .entries
                    .keys()
                    .next()
                    .unwrap()
                    .enclosing
                    .unwrap()
            )
            .is_ok());
        // Context-free calls have no native cache lookup; their AST frames must be released.
        for _ in 0..8 {
            assert_eq!(
                checker.type_to_string(object, 0).unwrap().as_bytes(),
                b"{ a: number; }"
            );
            assert_eq!(counters.snapshot(), retained.unwrap());
        }
    }
    assert_eq!(counters.snapshot(), initial);
}

#[test]
fn cache_hits_restore_length_symbols_and_generated_identifier_metadata() {
    let counters = Counters::new();
    let mut checker = state(&counters);
    let scope = scope(&counters);
    let ty = checker.builtins.string_type;
    let symbol = checker
        .new_symbol(
            ts_ast::symbol_flags::TYPE_ALIAS,
            JsString::from_bytes(b"X".as_slice()),
        )
        .unwrap();
    let mut first = None;
    for _ in 0..3 {
        NodeBuilder::with_cached(&mut checker, 0, |builder| {
            enter(builder, &scope);
            let key = SerializedKey {
                enclosing: builder.enclosing,
                ty,
                flags: 0,
                internal_flags: 0,
            };
            let node = builder.visit_transform_type(ty, |builder, _| {
                assert!(
                    first.is_none(),
                    "a completed cache entry must bypass transformation"
                );
                builder.track_symbol(symbol, ts_ast::symbol_flags::TYPE)?;
                builder.approximate_length += 17;
                builder.truncating = true;
                let name = builder.emit.new_generated_name_for_node_ex(
                    &mut builder.ast,
                    scope.root().unwrap(),
                    ts_printer::AutoGenerateOptions::default(),
                );
                builder.id_to_symbol.insert(name, Some(symbol));
                builder
                    .emit
                    .set_emit_flags(name, emit_flags::NO_ASCII_ESCAPING);
                let node = builder.ast.new_type_reference_node(Some(name), None);
                first = Some((node, name));
                Ok(node)
            })?;
            assert_eq!(builder.approximate_length, 17);
            assert!(builder.truncating);
            let (_, original_name) = first.unwrap();
            let name = builder
                .ast
                .view()
                .node(node)?
                .as_type_reference_node()
                .unwrap()
                .type_name()
                .unwrap();
            assert_eq!(
                builder.emit.auto_generate_info(name),
                builder.emit.auto_generate_info(original_name)
            );
            assert_eq!(builder.emit.most_original(name), original_name);
            assert_eq!(
                builder.identifier_symbol(original_name),
                Some(&Some(symbol))
            );
            assert_eq!(builder.emit.emit_flags(name), emit_flags::NO_ASCII_ESCAPING);
            if !builder.serialized.contains_key(&key) {
                assert_eq!(builder.tracked_symbols.len(), 1);
                assert_eq!(builder.tracked_symbols[0].symbol, symbol);
                assert_eq!(builder.tracked_symbols[0].enclosing, scope.root());
            }
            Ok(JsString::default())
        })
        .unwrap();
    }
}

#[test]
fn nested_error_and_diagnostic_frames_do_not_publish_or_sweep_outer_metadata() {
    let counters = Counters::new();
    let mut checker = state(&counters);
    let scope = scope(&counters);
    let before = counters.snapshot();
    let ty = checker.builtins.string_type;
    NodeBuilder::with_cached(&mut checker, 0, |outer| {
        enter(outer, &scope);
        let root = outer.ast.new_keyword_type_node(K::StringKeyword.into());
        outer
            .emit
            .set_emit_flags(root, emit_flags::NO_ASCII_ESCAPING);
        let result = NodeBuilder::with_cached(outer.checker, 0, |inner| {
            enter(inner, &scope);
            inner.visit_transform_type(ty, |inner, _| {
                Ok(inner.ast.new_keyword_type_node(K::NumberKeyword.into()))
            })?;
            Err(Error::Unsupported("request failed after transformation"))
        });
        assert!(matches!(
            result,
            Err(Error::Unsupported("request failed after transformation"))
        ));
        assert!(outer.checker.display_builder.entries.is_empty());
        assert_eq!(outer.emit.emit_flags(root), emit_flags::NO_ASCII_ESCAPING);
        assert_eq!(outer.checker.display_builder.active, 1);
        outer.visit_transform_type(ty, |outer, _| {
            outer.report(ts_printer::emit_resolver::DeclarationTrackerEvent::CyclicStructure);
            Ok(root)
        })?;
        assert!(outer.serialized.is_empty());
        print(outer, root)
    })
    .unwrap();
    assert_eq!(checker.display_builder.active, 0);
    assert!(checker.display_builder.entries.is_empty());
    assert_eq!(counters.snapshot(), before);
}

#[test]
fn explicit_builder_keeps_old_results_and_context_flags_separate() {
    let counters = Counters::new();
    let mut checker = state(&counters);
    let scope = scope(&counters);
    let ty = checker.builtins.string_type;
    let mut builder = NodeBuilder::new(&mut checker, 0);
    enter(&mut builder, &scope);
    let first = builder
        .visit_transform_type(ty, |b, _| {
            Ok(b.ast.new_keyword_type_node(K::StringKeyword.into()))
        })
        .unwrap();
    for (flags, internal) in [(1, 0), (0, 1)] {
        builder.flags = flags;
        builder.internal_flags = internal;
        let node = builder
            .visit_transform_type(ty, |b, _| {
                Ok(b.ast.new_keyword_type_node(K::NumberKeyword.into()))
            })
            .unwrap();
        assert_ne!(node, first);
        assert_eq!(print(&builder, node).unwrap().as_bytes(), b"number");
    }
    builder.flags = 0;
    builder.internal_flags = 0;
    let hit = builder
        .visit_transform_type(ty, |_, _| panic!("hit expected"))
        .unwrap();
    assert_ne!(first, hit);
    assert_eq!(print(&builder, first).unwrap().as_bytes(), b"string");
    assert_eq!(print(&builder, hit).unwrap().as_bytes(), b"string");
    assert!(builder.checker.display_builder.entries.is_empty());
}

#[test]
fn nested_success_keeps_both_cache_roots_and_enclosing_keys_distinct() {
    let counters = Counters::new();
    let initial = counters.snapshot();
    {
        let mut checker = state(&counters);
        let outer_scope = scope(&counters);
        let inner_scope = scope(&counters);
        let ty = checker.builtins.string_type;
        NodeBuilder::with_cached(&mut checker, 0, |outer| {
            enter(outer, &outer_scope);
            let root = outer.visit_transform_type(ty, |builder, _| {
                Ok(builder.ast.new_keyword_type_node(K::StringKeyword.into()))
            })?;
            outer
                .emit
                .set_emit_flags(root, emit_flags::NO_ASCII_ESCAPING);
            assert_eq!(
                NodeBuilder::with_cached(outer.checker, 0, |inner| {
                    enter(inner, &inner_scope);
                    let node = inner.visit_transform_type(ty, |builder, _| {
                        Ok(builder.ast.new_keyword_type_node(K::NumberKeyword.into()))
                    })?;
                    print(inner, node)
                })?
                .as_bytes(),
                b"number"
            );
            assert_eq!(outer.emit.emit_flags(root), emit_flags::NO_ASCII_ESCAPING);
            print(outer, root)
        })
        .unwrap();
        assert_eq!(checker.display_builder.entries.len(), 2);
        let retained = counters.snapshot();
        for (scope, expected) in [
            (&outer_scope, b"string".as_slice()),
            (&inner_scope, b"number".as_slice()),
        ] {
            let result = NodeBuilder::with_cached(&mut checker, 0, |builder| {
                enter(builder, scope);
                let node =
                    builder.visit_transform_type(ty, |_, _| panic!("completed cache hit"))?;
                print(builder, node)
            })
            .unwrap();
            assert_eq!(result.as_bytes(), expected);
            assert_eq!(counters.snapshot(), retained);
        }
    }
    assert_eq!(counters.snapshot(), initial);
}
