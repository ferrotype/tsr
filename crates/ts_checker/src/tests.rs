use crate::{
    alias_symbol, alias_type_arguments, object_flags, to_node_builder_flags, type_flags,
    type_format_flags, CheckerOptions, CheckerOwner, Error, IntrinsicData, LinkStore, Payload,
    ResolutionStack, TypeAlias, TypeId, TypeResolution, TypeStore, TypeSystemEntity,
    TypeSystemPropertyName,
};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use ts_arena::{CheckerIdentity, Counters, Generation, NodeId, SymbolArena, SymbolId};
use ts_ast::{symbol_flags, JsString};

fn node(arena: &SymbolArena<u32>, slot: u32) -> NodeId {
    NodeId::from_parts(arena.id(), slot).unwrap()
}

fn symbol(arena: &SymbolArena<u32>, slot: u32) -> SymbolId {
    SymbolId::from_parts(arena.id(), slot).unwrap()
}

fn owner() -> (
    Counters,
    Generation,
    Arc<CheckerIdentity>,
    Arc<CheckerOwner>,
) {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let identity = CheckerIdentity::new(generation.clone(), &counters);
    let owner = CheckerOwner::new(identity.clone(), &counters, CheckerOptions::default()).unwrap();
    (counters, generation, identity, Arc::new(owner))
}

#[test]
fn missing_indexed_property_without_a_program_remains_unresolved() {
    let (_counters, _generation, _identity, owner) = owner();
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    let object = state.new_anonymous_type(None, None, &[], &[], &[]).unwrap();
    let name = state
        .get_string_literal_type(JsString::from_bytes(b"missing".as_slice()))
        .unwrap();
    assert_eq!(
        state.indexed_access_or_undefined(object, name, 0, None, None),
        Ok(None)
    );
}

#[test]
fn unused_pass_failure_is_sticky_without_poisoning_completed_type_checks() {
    use ts_ast::FactoryMethods;
    let (_counters, _generation, _identity, owner) = owner();
    let (source, other) = {
        let mut operation = owner.operation().unwrap();
        let state = operation.state_mut();
        let mut sources = Vec::new();
        for file in [b"/failed.ts".as_slice(), b"/other.ts"] {
            let source = state.factory.new_source_file(
                ts_ast::SourceFileParseOptions {
                    file_name: JsString::from_bytes(file),
                    path: JsString::from_bytes(file),
                    ..Default::default()
                },
                ts_jsstring::SourceText::from_loaded_bytes(b"".as_slice()),
                None,
                None,
            );
            // Model the already-completed prerequisite. Inject an unsupported
            // node into the late pass so the production failure path executes.
            state
                .source_checks
                .insert(source, crate::check::SourceCheckStatus::Complete);
            sources.push(source);
        }
        let invalid_unused_node = state
            .factory
            .new_identifier(JsString::from_bytes(b"x".as_slice()));
        state
            .query
            .identifier_check_nodes
            .insert(sources[0], vec![invalid_unused_node]);
        (sources[0], sources[1])
    };
    let expected = Err(Error::Unsupported(
        "checkUnusedIdentifiers: unhandled registered node kind",
    ));
    for _ in 0..3 {
        let mut operation = owner.operation().unwrap();
        let state = operation.state_mut();
        assert_eq!(state.check_source_file_ex(source, true), expected);
        assert!(!state.query.identifier_check_nodes.contains_key(&source));
        assert_eq!(state.check_source_file_ex(source, false), Ok(()));
        assert_eq!(state.check_source_file_ex(other, true), Ok(()));
        assert!(matches!(
            state.source_checks.get(&source),
            Some(crate::check::SourceCheckStatus::Complete)
        ));
    }
}

#[test]
fn alias_circularity_without_a_declaration_returns_any() {
    let (_counters, _generation, _identity, owner) = owner();
    let symbol = owner
        .operation()
        .unwrap()
        .state_mut()
        .new_symbol(
            symbol_flags::ALIAS,
            JsString::from_bytes(b"alias".as_slice()),
        )
        .unwrap();
    for _ in 0..2 {
        let mut operation = owner.operation().unwrap();
        let state = operation.state_mut();
        assert_eq!(
            state.report_symbol_circularity(symbol),
            Ok(state.builtins.any_type)
        );
        // ResolveAlias/markAliasSymbolAsReferenced still require a declaration.
        assert_eq!(
            state.alias_declaration(symbol),
            Err(Error::MissingLink("alias declaration"))
        );
        assert!(state.diagnostics_for_file(None).unwrap().is_empty());
    }
}

#[test]
fn speculative_alias_resolution_preserves_the_active_chain() {
    let (_counters, _generation, _identity, owner) = owner();
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    let alias = state
        .new_symbol(
            symbol_flags::ALIAS,
            JsString::from_bytes(b"alias".as_slice()),
        )
        .unwrap();
    let intermediate = state
        .new_symbol(
            symbol_flags::PROPERTY,
            JsString::from_bytes(b"value".as_slice()),
        )
        .unwrap();
    assert!(state.push_source_resolution(alias, TypeSystemPropertyName::AliasTarget));
    assert!(state.push_source_resolution(intermediate, TypeSystemPropertyName::Type));
    assert_eq!(state.try_resolve_alias(alias), Ok(None));
    assert_eq!(state.resolution.depth(), 2);
    assert!(!state.module_aliases.targets.contains_key(&alias));
    assert!(
        state.resolution.pop(),
        "speculation must not fail intermediate work"
    );
    assert!(
        state.resolution.pop(),
        "speculation must not manufacture a cycle"
    );
}

#[test]
fn module_resolution_follows_assignment_backed_aliases_but_keeps_local_merges() {
    let (_counters, _generation, _identity, owner) = owner();
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    let target = state
        .new_symbol(
            symbol_flags::FUNCTION,
            JsString::from_bytes(b"target".as_slice()),
        )
        .unwrap();
    for (flags, resolves) in [
        (symbol_flags::ALIAS, true),
        (symbol_flags::ALIAS | symbol_flags::PROPERTY, false),
        (
            symbol_flags::ALIAS | symbol_flags::PROPERTY | symbol_flags::ASSIGNMENT,
            true,
        ),
    ] {
        let alias = state
            .new_symbol(flags, JsString::from_bytes(b"alias".as_slice()))
            .unwrap();
        // A cached target isolates the caller's decision from alias traversal.
        state.module_aliases.targets.insert(alias, Ok(target));
        assert_eq!(
            state.resolve_module_symbol(Some(alias), false),
            Ok(Some(if resolves { target } else { alias }))
        );
        assert_eq!(
            state.resolve_module_symbol(Some(alias), true),
            Ok(Some(alias))
        );
    }
}

#[test]
fn census_follows_mapper_links_without_resolving_or_rooting_the_arena() {
    let (_counters, _generation, _identity, owner) = owner();
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    let before = state.reachable_types(&[]).unwrap();
    let source = state.new_type_parameter(None).unwrap();
    let target = state.new_type_parameter(None).unwrap();
    let mapper = state
        .alloc_mapper(crate::mapper::Mapper::Simple { source, target })
        .unwrap();
    let root = state.new_anonymous_type(None, None, &[], &[], &[]).unwrap();
    state.types.object_mut(root).unwrap().mapper = Some(mapper);
    // Occupied records alone are not roots. A retained result keeps its mapper
    // and both mapper endpoints live, even without a query-cache entry.
    assert_eq!(state.reachable_types(&[]).unwrap(), before);
    let created = state.types.len();
    let report = state.census(&[root]).unwrap();
    assert_eq!(
        report["types"]["reachable"].as_u64(),
        Some((before + 3) as u64)
    );
    assert!(report["families"]["mappers"]["bytes"].as_u64().unwrap() > 0);
    assert_eq!(state.types.len(), created);
    assert!(state
        .types
        .type_parameter(source)
        .unwrap()
        .constraint
        .is_none());
}

#[test]
fn pattern_literal_property_conflicts_reduce_the_intersection() {
    let (_counters, _generation, _identity, owner) = owner();
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    let string = state.builtins.string_type;
    let number = state.builtins.number_type;
    let pattern = state
        .get_template_literal_type(
            &[JsString::from_bytes(b"x".as_slice()), JsString::default()],
            &[string],
        )
        .unwrap();
    // Source template type nodes are still unsupported. Construct the property
    // types through the existing checker API to exercise synthesis and reduction,
    // including the non-literal string/number control from pinned Go's predicate.
    for (left, is_discriminant) in [(pattern, true), (string, false)] {
        let mut objects = Vec::new();
        for ty in [left, number] {
            let name = JsString::from_bytes(b"a".as_slice());
            let property = state
                .new_symbol(symbol_flags::PROPERTY, name.clone())
                .unwrap();
            state
                .value_symbol_links
                .get_or_default(property)
                .resolved_type = Some(ty);
            let mut table = ts_ast::SymbolTable::new();
            table.insert(name, Some(property));
            let members = state.alloc_symbol_table(table);
            objects.push(
                state
                    .new_anonymous_type(None, Some(members), &[], &[], &[])
                    .unwrap(),
            );
        }
        let intersection = state.get_intersection_type(&objects).unwrap();
        let properties = state
            .get_properties_of_union_or_intersection_type(intersection)
            .unwrap();
        assert_eq!(properties.len(), 1);
        assert_eq!(
            state.symbol(properties[0]).unwrap().check_flags()
                & ts_ast::check_flags::HAS_LITERAL_TYPE
                != 0,
            is_discriminant,
        );
        assert_eq!(
            state.get_type_of_symbol(properties[0]).unwrap(),
            state.builtins.never_type
        );
        let expected = if is_discriminant {
            state.builtins.never_type
        } else {
            intersection
        };
        assert_eq!(state.get_reduced_type(intersection).unwrap(), expected);
        assert_eq!(state.get_reduced_type(intersection).unwrap(), expected);
    }
}

#[test]
fn link_store_pages_on_first_use_per_arena_and_distinguishes_absent_entries() {
    let counters = Counters::new();
    let first = SymbolArena::<u32>::new(&counters);
    let second = SymbolArena::<u32>::new(&counters);
    let mut links = LinkStore::<NodeId, u32>::new();
    assert!(links.is_empty());
    assert_eq!(links.pages(), 0);
    assert!(!links.has(node(&first, 1)));
    assert_eq!(links.try_get(node(&first, 1)), None);
    *links.get_or_default(node(&first, 1)) = 5;
    *links.get_or_default(node(&first, 300)) = 6;
    *links.get_or_default(node(&second, 1)) = 7;
    assert_eq!(links.len(), 3);
    assert_eq!(
        links.pages(),
        3,
        "slot 300 opens a second page; the second arena its own"
    );
    assert_eq!(links.try_get(node(&first, 1)), Some(&5));
    assert_eq!(links.try_get(node(&first, 300)), Some(&6));
    assert_eq!(links.try_get(node(&second, 1)), Some(&7));
    assert!(
        !links.has(node(&first, 2)),
        "a page does not make its other slots present"
    );
    assert!(!links.has(node(&first, 299)));
    assert_eq!(
        *links.get_or_default(node(&first, 1)),
        5,
        "Get returns the existing entry"
    );
    assert_eq!(links.len(), 3);
    *links.try_get_mut(node(&second, 1)).unwrap() = 8;
    assert_eq!(links.try_get(node(&second, 1)), Some(&8));
    let mut by_symbol = LinkStore::<SymbolId, Vec<u8>>::new();
    by_symbol.get_or_default(symbol(&first, 1)).push(1);
    assert_eq!(by_symbol.try_get(symbol(&first, 1)), Some(&vec![1]));
}

#[test]
fn resolution_cycle_invalidates_the_suffix_and_pushes_nothing() {
    let counters = Counters::new();
    let arena = SymbolArena::<u32>::new(&counters);
    let a = TypeSystemEntity::Symbol(symbol(&arena, 1));
    let b = TypeSystemEntity::Symbol(symbol(&arena, 2));
    let mut stack = ResolutionStack::new();
    let never = |_: &TypeResolution| false;
    assert!(stack.push(a, TypeSystemPropertyName::Type, never));
    assert!(stack.push(b, TypeSystemPropertyName::DeclaredType, never));
    assert!(
        stack.push(a, TypeSystemPropertyName::DeclaredType, never),
        "same entity, other property"
    );
    assert!(!stack.push(a, TypeSystemPropertyName::Type, never));
    assert_eq!(stack.depth(), 3, "a failed push adds nothing");
    assert!(
        !stack.pop(),
        "everything from the cycle start is marked failed"
    );
    assert!(!stack.pop());
    assert!(!stack.pop());
    assert_eq!(stack.depth(), 0);
}

#[test]
fn resolution_search_stops_at_a_produced_property_and_at_the_floor() {
    let counters = Counters::new();
    let arena = SymbolArena::<u32>::new(&counters);
    let a = TypeSystemEntity::Symbol(symbol(&arena, 1));
    let b = TypeSystemEntity::Type(TypeId::new(1).unwrap());
    let mut stack = ResolutionStack::new();
    assert!(stack.push(a, TypeSystemPropertyName::Type, |_| false));
    assert!(stack.push(b, TypeSystemPropertyName::ResolvedBaseTypes, |_| false));
    // b has produced its property meanwhile: the search stops there and a is not a cycle.
    let produced = |r: &TypeResolution| r.target == b;
    assert_eq!(
        stack.find_resolution_cycle_start_index(a, TypeSystemPropertyName::Type, produced),
        None
    );
    assert!(stack.push(a, TypeSystemPropertyName::Type, produced));
    assert!(stack.pop());
    assert_eq!(
        stack.find_resolution_cycle_start_index(a, TypeSystemPropertyName::Type, |_| false),
        Some(0)
    );
    assert_eq!(stack.set_resolution_start(2), 0);
    assert!(
        stack.push(a, TypeSystemPropertyName::Type, |_| false),
        "below the floor is not searched"
    );
    assert_eq!(stack.set_resolution_start(0), 2);
    assert!(stack.pop());
    assert!(stack.pop());
    assert!(stack.pop());
}

#[test]
fn owner_adopts_the_identity_once_and_scopes_state_to_an_operation() {
    let (counters, _generation, identity, owner) = owner();
    assert!(matches!(
        CheckerOwner::new(identity.clone(), &counters, CheckerOptions::default()),
        Err(Error::Arena(ts_arena::Error::IdentityAdopted))
    ));
    let mut operation = owner.operation().unwrap();
    assert_eq!(operation.state().symbols().id(), identity.id());
    let undefined = operation
        .state_mut()
        .new_symbol(
            symbol_flags::PROPERTY,
            JsString::from_bytes(&b"undefined"[..]),
        )
        .unwrap();
    assert_eq!(
        operation.lease().validate_identity(undefined.arena()),
        Ok(())
    );
    let before = operation.state().types().len();
    let any = operation
        .state_mut()
        .new_intrinsic_type(type_flags::ANY, b"any")
        .unwrap();
    assert_eq!(
        any.get() as usize,
        before + 1,
        "type ids continue TypeCount after the NewChecker prefix"
    );
    assert_eq!(
        operation.state().types().get(any).unwrap().flags,
        type_flags::ANY
    );
    assert!(operation.state_mut().resolution_mut().push(
        TypeSystemEntity::Type(any),
        TypeSystemPropertyName::ResolvedBaseConstraint,
        |_| false
    ));
    assert!(operation.state_mut().resolution_mut().pop());
    drop(operation);
    assert_eq!(owner.operation().unwrap().state().types().len(), before + 1);
}

#[test]
fn same_thread_reentry_fails_before_waiting_and_drop_allows_another_operation() {
    let (_counters, _generation, _identity, owner) = owner();
    let held = owner.operation().unwrap();
    assert!(matches!(owner.operation(), Err(Error::Reentry)));
    drop(held);
    assert!(
        owner.operation().is_ok(),
        "the reentry record is cleared on drop"
    );
}

#[test]
fn a_panic_inside_an_operation_retires_the_generation() {
    let (_counters, generation, _identity, owner) = owner();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _operation = owner.operation().unwrap();
        panic!("injected checker failure");
    }));
    assert!(result.is_err());
    assert_eq!(generation.validate(), Err(ts_arena::Error::Retired));
    assert!(matches!(
        owner.operation(),
        Err(Error::Arena(ts_arena::Error::Retired))
    ));
}

#[test]
fn a_direct_identity_lease_cannot_bypass_operation_reentry_detection() {
    let (_counters, _generation, identity, owner) = owner();
    let lease = identity.lease().unwrap();
    assert!(matches!(owner.operation(), Err(Error::Reentry)));
    drop(lease);
    let operation = owner.operation().unwrap();
    assert!(matches!(identity.lease(), Err(ts_arena::Error::Reentry)));
    drop(operation);
    assert!(identity.lease().is_ok());
}

#[test]
fn owner_is_send_and_sync_and_ids_reject_zero() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CheckerOwner>();
    assert_eq!(TypeId::new(0), None);
    assert_eq!(crate::SignatureId::new(0), None);
}

#[test]
fn type_store_numbers_from_one_and_alias_helpers_tolerate_absence() {
    let counters = Counters::new();
    let arena = SymbolArena::<u32>::new(&counters);
    let mut store = TypeStore::new();
    let intrinsic = || {
        Payload::Intrinsic(IntrinsicData {
            name: ts_ast::JsString::from_bytes(&b"any"[..]),
        })
    };
    let first = store
        .new_type(type_flags::ANY, object_flags::NONE, intrinsic())
        .unwrap();
    let second = store
        .new_type(type_flags::ANY, object_flags::NONE, intrinsic())
        .unwrap();
    assert_eq!((first.get(), second.get(), store.len()), (1, 2, 2));
    assert!(store.get(TypeId::new(3).unwrap()).is_err());
    let alias = store
        .push_alias(TypeAlias {
            symbol: symbol(&arena, 1),
            type_arguments: Arc::from([first, second]),
        })
        .unwrap();
    store.get_mut(second).unwrap().alias = Some(alias);
    let stored = store.alias(alias).ok();
    assert_eq!(alias_symbol(stored), Some(symbol(&arena, 1)));
    assert_eq!(alias_type_arguments(stored), &[first, second]);
    assert_eq!(alias_symbol(None), None);
    assert_eq!(alias_type_arguments(None), &[] as &[TypeId]);
}

#[test]
fn node_builder_flags_are_a_mask_of_type_format_flags() {
    let flags = type_format_flags::NO_TRUNCATION
        | type_format_flags::ADD_UNDEFINED
        | type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE
        | type_format_flags::IN_ARRAY_TYPE;
    let converted = to_node_builder_flags(flags);
    assert_eq!(
        converted,
        ts_nodebuilder::flags::NO_TRUNCATION
            | ts_nodebuilder::flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE
    );
    assert_eq!(
        converted & type_format_flags::ADD_UNDEFINED,
        0,
        "TypeFormatFlags-only bits are masked out"
    );
}

// P1 exit contracts: foreign owners, retention, exhaustion and first queries
// (plan §4.1, `data/s08/ownership-fixtures.json`).

#[test]
fn a_same_numbered_handle_from_another_checker_is_rejected_before_any_read() {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let a = Arc::new(
        CheckerOwner::new(
            CheckerIdentity::new(generation.clone(), &counters),
            &counters,
            CheckerOptions::default(),
        )
        .unwrap(),
    );
    let b = Arc::new(
        CheckerOwner::new(
            CheckerIdentity::new(generation.clone(), &counters),
            &counters,
            CheckerOptions::default(),
        )
        .unwrap(),
    );
    // Equal numeric slots in both checkers of one pool generation.
    let (retained_type, retained_symbol, retained_signature, retained_node, retained_list, type_a) = {
        let mut op = a.operation().unwrap();
        let t = op.string_literal_type(b"same").unwrap();
        let symbol = op.new_symbol(symbol_flags::PROPERTY, b"p", 0).unwrap();
        let signature = op.call_signature(&[symbol], t).unwrap();
        let node = op.synthetic_expression(t).unwrap();
        (
            op.retain_type(t).unwrap(),
            op.retain_symbol(symbol).unwrap(),
            op.retain_signature(signature).unwrap(),
            op.retain_node(node).unwrap(),
            op.retain_type_list(&[t]).unwrap(),
            t,
        )
    };
    let mut op_b = b.operation().unwrap();
    let t_b = op_b.string_literal_type(b"same").unwrap();
    assert_eq!(
        t_b.id(),
        type_a.id(),
        "the same numeric slot in both checkers"
    );
    let wrong = Some(Error::Arena(ts_arena::Error::WrongOwner));
    assert_eq!(op_b.import_type(&retained_type).err(), wrong);
    assert_eq!(op_b.import_symbol(&retained_symbol).err(), wrong);
    assert_eq!(op_b.import_signature(&retained_signature).err(), wrong);
    assert_eq!(op_b.import_node(&retained_node).err(), wrong);
    assert_eq!(op_b.import_type_list(&retained_list).err(), wrong);
    // A ref minted by A's operation is rejected by B's before any read.
    assert_eq!(op_b.type_flags(type_a).err(), wrong);
    assert_eq!(op_b.union_type(&[type_a, t_b]).err(), wrong);
    drop(op_b);
    // The same handles import into their own checker.
    let op_a = a.operation().unwrap();
    assert_eq!(op_a.import_type(&retained_type).unwrap(), type_a);
    assert_eq!(op_a.import_type_list(&retained_list).unwrap(), vec![type_a]);
    assert!(op_a.import_symbol(&retained_symbol).is_ok());
    assert!(op_a.import_signature(&retained_signature).is_ok());
    assert!(op_a.import_node(&retained_node).is_ok());
}

#[test]
fn retained_results_outlive_the_operation_and_the_callers_owner_handle() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let generation = Generation::new(&counters);
    let identity = CheckerIdentity::new(generation.clone(), &counters);
    let retained = {
        let owner =
            Arc::new(CheckerOwner::new(identity, &counters, CheckerOptions::default()).unwrap());
        let mut op = owner.operation().unwrap();
        let union = {
            let string = op.builtin_type("stringType").unwrap();
            let literal = op.string_literal_type(b"kept").unwrap();
            op.union_type(&[literal, string]).unwrap()
        };
        let node = op.synthetic_expression(union).unwrap();
        let signature = op.call_signature(&[], union).unwrap();
        let retained = (
            op.retain_type(union).unwrap(),
            op.retain_node(node).unwrap(),
            op.retain_signature(signature).unwrap(),
        );
        drop(op);
        drop(owner);
        retained
    };
    // The caller's owner handle is gone; the results keep the checker alive.
    assert_eq!(Arc::strong_count(retained.0.owner()), 3);
    let owner = retained.0.owner().clone();
    let op = owner.operation().unwrap();
    let union = op.import_type(&retained.0).unwrap();
    let node = op.import_node(&retained.1).unwrap();
    let signature = op.import_signature(&retained.2).unwrap();
    assert_eq!(op.synthetic_expression_type(node).unwrap(), union);
    assert_eq!(op.signature_return_type(signature).unwrap(), Some(union));
    let declaration = op.signature_declaration(signature).unwrap().unwrap();
    assert_eq!(
        op.node_kind(declaration).unwrap().known(),
        Some(ts_ast::SyntaxKind::FunctionType)
    );
    assert_eq!(
        union,
        op.builtin_type("stringType").unwrap(),
        "`\"kept\" | string` reduces to `string`"
    );
    assert!(op.constituents(union).unwrap().is_empty());
    drop(op);
    drop(owner);
    drop(retained);
    drop(generation);
    assert_eq!(
        counters.snapshot(),
        baseline,
        "owner and allocation counts return to the baseline after the final root drops"
    );
}

#[test]
fn retirement_rejects_retained_handles_while_their_storage_stays_live() {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let owner = Arc::new(
        CheckerOwner::new(
            CheckerIdentity::new(generation.clone(), &counters),
            &counters,
            CheckerOptions::default(),
        )
        .unwrap(),
    );
    let retained = {
        let mut op = owner.operation().unwrap();
        let t = op.string_literal_type(b"x").unwrap();
        op.retain_type(t).unwrap()
    };
    generation.retire();
    assert!(matches!(
        owner.operation(),
        Err(Error::Arena(ts_arena::Error::Retired))
    ));
    assert_eq!(retained.id(), retained.id(), "the handle is intact");
    assert!(
        Arc::ptr_eq(retained.owner(), &owner),
        "and still owns its storage"
    );
}

#[test]
fn checker_local_id_exhaustion_fails_before_writing() {
    let mut store = TypeStore::with_base_for_test(u32::MAX - 1);
    let intrinsic = || {
        Payload::Intrinsic(IntrinsicData {
            name: ts_ast::JsString::from_bytes(&b"any"[..]),
        })
    };
    let last = store
        .new_type(type_flags::ANY, object_flags::NONE, intrinsic())
        .unwrap();
    assert_eq!(last.get(), u32::MAX);
    assert_eq!(
        store
            .new_type(type_flags::ANY, object_flags::NONE, intrinsic())
            .err(),
        Some(Error::IdExhausted)
    );
    assert_eq!(store.len(), 1);
    assert_eq!(store.next_id().err(), Some(Error::IdExhausted));
}

#[test]
fn the_first_queries_of_a_fresh_checker_reuse_new_checker_types() {
    let (_counters, _generation, _identity, owner) = owner();
    let mut op = owner.operation().unwrap();
    let string = op.builtin_type("stringType").unwrap();
    let number = op.builtin_type("numberType").unwrap();
    let string_or_number = op.builtin_type("stringOrNumberType").unwrap();
    let count = op.type_count();
    assert_eq!(op.union_type(&[number, string]).unwrap(), string_or_number);
    assert_eq!(
        op.union_type(&[string, number, string]).unwrap(),
        string_or_number
    );
    assert_eq!(
        op.string_literal_type(b"").unwrap(),
        op.builtin_type("emptyStringType").unwrap()
    );
    assert_eq!(
        op.number_literal_type(0.0).unwrap(),
        op.builtin_type("zeroType").unwrap()
    );
    assert_eq!(
        op.number_literal_type(-0.0).unwrap(),
        op.builtin_type("zeroType").unwrap()
    );
    assert_eq!(op.type_count(), count, "interned queries create nothing");
    let fresh = op.string_literal_type(b"new").unwrap();
    assert_eq!(
        fresh.id() as usize,
        count + 1,
        "the next type continues TypeCount"
    );
    assert_eq!(op.type_flags(fresh).unwrap(), type_flags::STRING_LITERAL);
    assert_eq!(op.builtin_type("noSuchType"), None);
    assert_eq!(
        op.union_type_with(&[string, number], crate::UnionReduction::Subtype)
            .unwrap(),
        string_or_number
    );
    assert_eq!(
        op.type_count(),
        count + 1,
        "subtype reduction reuses the intrinsic union"
    );
}
