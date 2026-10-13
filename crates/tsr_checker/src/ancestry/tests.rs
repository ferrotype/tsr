use super::*;
use tsr_arena::Counters;
use tsr_ast::{AstBuilder, Factory, FactoryMethods, JsString};
use tsr_jsstring::SourceText;

fn new_builder() -> AstBuilder {
    AstBuilder::new(SourceText::from_bytes(b"".as_slice()), &Counters::new())
}

fn node(builder: &mut AstBuilder, kind: K, parent: Option<NodeId>) -> NodeId {
    // These walks inspect headers only, except for the separately constructed
    // call expression in the IIFE test.
    let id = builder.new_modifier(kind.into());
    builder.set_node_parent(id, parent);
    id
}

#[test]
fn type_ancestry_does_not_inherit_ambient_flags() {
    let mut builder = new_builder();
    let root = node(&mut builder, K::SourceFile, None);
    let ambient = node(&mut builder, K::Block, Some(root));
    builder.set_node_flags(ambient, nf::AMBIENT);
    let ordinary = node(&mut builder, K::Identifier, Some(ambient));
    let type_literal = node(&mut builder, K::TypeLiteral, Some(root));
    let typed = node(&mut builder, K::Identifier, Some(type_literal));
    let mut cache = AncestryCache::default();
    assert!(cache
        .in_ambient_or_type_node(builder.view(), ambient, true)
        .unwrap());
    assert!(!cache
        .in_ambient_or_type_node(builder.view(), ordinary, true)
        .unwrap());
    assert!(cache
        .in_ambient_or_type_node(builder.view(), ambient, true)
        .unwrap());
    assert!(cache
        .in_ambient_or_type_node(builder.view(), typed, true)
        .unwrap());
    assert!(cache
        .in_ambient_or_type_node(builder.view(), type_literal, true)
        .unwrap());
    assert!(!cache
        .in_ambient_or_type_node(builder.view(), root, true)
        .unwrap());
}

#[test]
fn containers_exclude_the_start_and_skip_immediately_invoked_functions() {
    let mut builder = new_builder();
    let root = node(&mut builder, K::SourceFile, None);
    let outer = node(&mut builder, K::FunctionDeclaration, Some(root));
    let property = node(&mut builder, K::PropertyDeclaration, Some(outer));
    let nested = node(&mut builder, K::Identifier, Some(property));
    let function = builder.new_function_expression(None, None, None, None, None, None, None, None);
    let wrapper = builder.new_parenthesized_expression(Some(function));
    builder.set_node_parent(function, Some(wrapper));
    let invocation = builder.new_call_expression(Some(wrapper), None, None, None, 0);
    builder.set_node_parent(wrapper, Some(invocation));
    builder.set_node_parent(invocation, Some(outer));
    let inside_iife = node(&mut builder, K::Identifier, Some(function));
    let mut cache = AncestryCache::default();
    for _ in 0..2 {
        assert_eq!(
            cache
                .control_flow_container(builder.view(), nested, true)
                .unwrap(),
            Some(property)
        );
        assert_eq!(
            cache
                .control_flow_container(builder.view(), property, true)
                .unwrap(),
            Some(outer)
        );
        assert_eq!(
            cache
                .control_flow_container(builder.view(), inside_iife, true)
                .unwrap(),
            Some(outer)
        );
        assert_eq!(
            cache
                .control_flow_container(builder.view(), outer, true)
                .unwrap(),
            Some(root)
        );
        assert_eq!(
            cache
                .control_flow_container(builder.view(), root, true)
                .unwrap(),
            None
        );
    }
}

#[test]
fn failed_walks_publish_no_entries_and_can_retry() {
    let mut builder = new_builder();
    let root = node(&mut builder, K::SourceFile, None);
    let invalid = NodeId::from_parts(root.arena(), root.slot() + 100).unwrap();
    let child = node(&mut builder, K::Identifier, Some(invalid));
    let mut cache = AncestryCache::default();
    assert!(cache
        .control_flow_container(builder.view(), child, true)
        .is_err());
    assert!(cache
        .in_ambient_or_type_node(builder.view(), child, true)
        .is_err());
    assert!(cache.containers.entries.is_empty());
    assert!(cache.type_nodes.entries.is_empty());
    assert!(cache.path.is_empty());
    builder.set_node_parent(child, Some(root));
    assert_eq!(
        cache
            .control_flow_container(builder.view(), child, true)
            .unwrap(),
        Some(root)
    );
    assert!(!cache
        .in_ambient_or_type_node(builder.view(), child, true)
        .unwrap());
    // An invalid slot in a cached arena cannot borrow a valid entry's answer.
    assert!(cache
        .control_flow_container(builder.view(), invalid, true)
        .is_err());
}

#[test]
fn collisions_preserve_owner_identity_and_path_memory_is_bounded() {
    let mut builder = new_builder();
    let root = node(&mut builder, K::SourceFile, None);
    let mut leaf = root;
    for _ in 0..CACHE_ENTRIES * 3 {
        leaf = node(&mut builder, K::Block, Some(leaf));
    }
    let mut cache = AncestryCache::default();
    assert_eq!(
        cache
            .control_flow_container(builder.view(), leaf, true)
            .unwrap(),
        Some(root)
    );
    assert!(!cache
        .in_ambient_or_type_node(builder.view(), leaf, true)
        .unwrap());
    assert_eq!(cache.containers.entries.len(), CACHE_ENTRIES);
    assert_eq!(cache.type_nodes.entries.len(), CACHE_ENTRIES);
    assert!(cache.path.capacity() <= CACHE_ENTRIES);
    let other = new_builder();
    let other_id = NodeId::from_parts(other.id().arena(), leaf.slot()).unwrap();
    assert_eq!(cache.containers.get(other_id), None);
    assert!(cache
        .control_flow_container(other.view(), other_id, true)
        .is_err());
    let collision = NodeId::from_parts(root.arena(), leaf.slot() + CACHE_ENTRIES as u32).unwrap();
    cache.containers.insert_path(&[collision], None);
    assert_eq!(cache.containers.get(leaf), None);
    assert_eq!(
        cache
            .control_flow_container(builder.view(), leaf, true)
            .unwrap(),
        Some(root)
    );
}

#[test]
fn checker_factory_mutation_never_uses_ancestry_cache() {
    let (_counters, _generation, _identity, owner) = crate::tests::owner();
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    let root = node(&mut state.factory, K::SourceFile, None);
    let typed = node(&mut state.factory, K::TypeLiteral, Some(root));
    let child = state
        .factory
        .new_identifier(JsString::from_bytes(b"x".as_slice()));
    state.factory.set_node_parent(child, Some(typed));
    assert!(state.in_ambient_or_type_node(child).unwrap());
    assert_eq!(state.control_flow_container(child).unwrap(), root);
    let container = node(&mut state.factory, K::ModuleBlock, Some(root));
    state.factory.set_node_parent(child, Some(container));
    assert!(!state.in_ambient_or_type_node(child).unwrap());
    assert_eq!(state.control_flow_container(child).unwrap(), container);
    assert!(state.ancestry.type_nodes.entries.is_empty());
    assert!(state.ancestry.containers.entries.is_empty());
}

#[test]
fn census_charges_ancestry_to_the_existing_family_inventory() {
    let mut builder = new_builder();
    let root = node(&mut builder, K::SourceFile, None);
    let child = node(&mut builder, K::Identifier, Some(root));
    let (_counters, _generation, _identity, owner) = crate::tests::owner();
    let mut operation = owner.operation().unwrap();
    let state = operation.state_mut();
    state
        .ancestry
        .control_flow_container(builder.view(), child, true)
        .unwrap();
    state
        .ancestry
        .in_ambient_or_type_node(builder.view(), child, true)
        .unwrap();
    let report = state.census(&[]).unwrap();
    let families = report["families"].as_object().unwrap();
    let actual: std::collections::BTreeSet<_> = families.keys().map(String::as_str).collect();
    let expected: std::collections::BTreeSet<_> =
        crate::census::ALL_FAMILIES.iter().copied().collect();
    assert_eq!(actual, expected);
    assert!(!families.contains_key("ancestry_caches"));
    assert_eq!(
        families["program_indices"]["bytes"].as_u64().unwrap(),
        (std::mem::size_of_val(state.ancestry.type_nodes.entries.as_ref())
            + std::mem::size_of_val(state.ancestry.containers.entries.as_ref())
            + state.ancestry.path.capacity() * size_of::<NodeId>()) as u64
    );
}
