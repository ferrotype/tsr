//! S09-2: builder caches and the persistent emit side tables across allocation
//! generations. A generation here is one request frame: it builds in its own
//! arena and, when it has reusable entries, is published as one `AstFile` that
//! the cache entries own. A cache hit clones the cached syntax into the current
//! frame, records `original` links back to it, and makes the current frame
//! retain the frame it cloned from.

use super::{NodeBuilder, SerializedKey};
use crate::{CheckerOptions, CheckerState, Error, TypeId};
use ts_arena::{CheckerIdentity, Counters, Counts, Generation};
use ts_ast::{AstBuilder, AstFile, FactoryMethods, JsString, NodeId};
use ts_jsstring::SourceText;
use ts_printer::{EmitTextWriter, Printer, PrinterOptions, TextWriter};

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

/// `{ <name>: <property type> }` as an anonymous object type.
fn object(checker: &mut CheckerState, name: &[u8], property_type: TypeId) -> TypeId {
    let property = checker
        .new_symbol(ts_ast::symbol_flags::PROPERTY, JsString::from_bytes(name))
        .unwrap();
    checker
        .value_symbol_links
        .get_or_default(property)
        .resolved_type = Some(property_type);
    let symbol = checker
        .new_symbol(
            ts_ast::symbol_flags::TYPE_LITERAL,
            JsString::from_bytes(b"__type".as_slice()),
        )
        .unwrap();
    let object = checker
        .new_anonymous_type(Some(symbol), None, &[], &[], &[])
        .unwrap();
    checker.types.structured_mut(object).unwrap().properties = Some(vec![property].into());
    object
}

fn key(scope: &AstFile, ty: TypeId) -> SerializedKey {
    SerializedKey {
        enclosing: scope.root(),
        ty,
        flags: 0,
        internal_flags: 0,
    }
}

/// The keys of every cache entry a generation owns. Each is a root of it.
fn roots(checker: &CheckerState, generation: NodeId) -> Vec<SerializedKey> {
    checker
        .display_builder
        .entries
        .iter()
        .filter(|(_, entry)| entry.value.node.arena() == generation.arena())
        .map(|(key, _)| *key)
        .collect()
}

fn remove_roots(checker: &mut CheckerState, generation: NodeId) -> Result<usize, Error> {
    let keys = roots(checker, generation);
    for key in &keys {
        assert!(checker.display_builder.remove(key)?);
    }
    Ok(keys.len())
}

/// Two generations: A caches `{ a: number }`, then B caches `{ b: { a: number } }`
/// and clones A's cached syntax into itself. B owns two entries, because the
/// property's type is serialized under the nested flag context and so misses
/// A's key; the clone comes from a hit on A's own key in the same request.
struct Generations {
    checker: CheckerState,
    scope: AstFile,
    inner: TypeId,
    outer: TypeId,
    /// A's cached node.
    original: NodeId,
    /// A node of B whose `original` link points at `original`.
    clone: NodeId,
    /// Counters with only the checker and the scope alive.
    empty: Counts,
    /// Counters once generation A is published.
    with_a: Counts,
    /// Counters once generation B is published too.
    with_both: Counts,
}

fn generations(counters: &Counters) -> Generations {
    let mut checker = state(counters);
    let scope = scope(counters);
    let number = checker.builtins.number_type;
    let inner = object(&mut checker, b"a", number);
    let outer = object(&mut checker, b"b", inner);
    let empty = counters.snapshot();

    let mut original = None;
    let text = NodeBuilder::with_cached(&mut checker, 0, |builder| {
        enter(builder, &scope);
        let node = builder.type_node(inner)?;
        original = Some(node);
        print(builder, node)
    })
    .unwrap();
    assert_eq!(text.as_bytes(), b"{ a: number; }");
    let original = original.unwrap();
    let with_a = counters.snapshot();

    let mut clone = None;
    let text = NodeBuilder::with_cached(&mut checker, 0, |builder| {
        enter(builder, &scope);
        let node = builder.type_node(outer)?;
        // A second hit on A's entry: a node of this generation whose id the
        // test can keep, with the same `original` link the nested clone has.
        let copy = builder.type_node(inner)?;
        assert_eq!(builder.emit.most_original(copy), original);
        clone = Some(copy);
        print(builder, node)
    })
    .unwrap();
    assert_eq!(text.as_bytes(), b"{ b: { a: number; }; }");
    let clone = clone.unwrap();
    assert_ne!(
        clone.arena(),
        original.arena(),
        "the two generations are different arenas"
    );
    assert!(roots(&checker, original) == [key(&scope, inner)]);
    assert!(roots(&checker, clone).contains(&key(&scope, outer)));
    let with_both = counters.snapshot();
    Generations {
        checker,
        scope,
        inner,
        outer,
        original,
        clone,
        empty,
        with_a,
        with_both,
    }
}

#[test]
fn a_clone_resolves_its_original_after_that_cache_entry_is_removed_and_allocation_rotates() {
    let counters = Counters::new();
    let initial = counters.snapshot();
    {
        let Generations {
            mut checker,
            scope,
            outer,
            original,
            clone,
            empty,
            with_both,
            ..
        } = generations(&counters);
        let outer_key = key(&scope, outer);

        assert_eq!(remove_roots(&mut checker, original).unwrap(), 1);
        assert!(roots(&checker, original).is_empty());
        // A's cache root is gone, and nothing was reclaimed: generation B
        // retains the generation it cloned from.
        assert_eq!(counters.snapshot(), with_both);
        let dependent = &checker.display_builder.entries[&outer_key].owner;
        assert!(dependent.view().node(original).is_ok());
        assert!(dependent.view().node(clone).is_ok());
        // The sweep kept the side-table entry, because its target is retained.
        assert_eq!(checker.display_builder.emit.original(clone), Some(original));

        // Allocation rotates: a new generation hits B's entry and still reaches
        // A's node through the chain of retained frames.
        let text = NodeBuilder::with_cached(&mut checker, 0, |builder| {
            enter(builder, &scope);
            let node = builder.type_node(outer)?;
            assert_ne!(node.arena(), clone.arena());
            assert_ne!(node.arena(), original.arena());
            assert!(builder.ast.view().node(original).is_ok());
            assert_eq!(builder.emit.most_original(clone), original);
            print(builder, node)
        })
        .unwrap();
        assert_eq!(text.as_bytes(), b"{ b: { a: number; }; }");
        assert_eq!(counters.snapshot(), with_both, "the hit published nothing");

        assert!(remove_roots(&mut checker, clone).unwrap() >= 1);
        assert_eq!(counters.snapshot(), empty, "both generations are reclaimed");
        assert_eq!(checker.display_builder.emit.original(clone), None);
        assert_eq!(checker.display_builder.emit.metadata_entries(), 0);
    }
    assert_eq!(counters.snapshot(), initial);
}

#[test]
fn a_nested_publication_replaces_a_cache_entry_without_breaking_its_dependents() {
    let counters = Counters::new();
    let initial = counters.snapshot();
    {
        let mut checker = state(&counters);
        let scope = scope(&counters);
        let number = checker.builtins.number_type;
        let inner = object(&mut checker, b"a", number);
        let outer = object(&mut checker, b"b", inner);
        let (inner_key, outer_key) = (key(&scope, inner), key(&scope, outer));
        let (mut original, mut clone, mut pending) = (None, None, None);
        // The production replacement path. The outer request serializes the
        // inner type first, unpublished. A nested request publishes the same
        // key (generation A), a second nested request clones it (generation
        // B), and the outer request then publishes its own node over A's entry.
        NodeBuilder::with_cached(&mut checker, 0, |request| {
            enter(request, &scope);
            let node = request.type_node(inner)?;
            pending = Some(node);
            NodeBuilder::with_cached(request.checker, 0, |first| {
                enter(first, &scope);
                let node = first.type_node(inner)?;
                original = Some(node);
                print(first, node)
            })?;
            NodeBuilder::with_cached(request.checker, 0, |second| {
                enter(second, &scope);
                let node = second.type_node(outer)?;
                let copy = second.type_node(inner)?;
                clone = Some(copy);
                print(second, node)
            })?;
            assert_eq!(
                request.checker.display_builder.entries[&inner_key]
                    .value
                    .node,
                original.unwrap(),
                "generation A holds the entry while the outer request runs"
            );
            print(request, node)
        })
        .unwrap();
        let (original, clone, pending) = (original.unwrap(), clone.unwrap(), pending.unwrap());

        let cache = &checker.display_builder;
        assert_eq!(cache.entries[&inner_key].value.node, pending);
        assert!(
            roots(&checker, original).is_empty(),
            "A has no cache root left"
        );
        assert_ne!(pending.arena(), original.arena(), "A's entry was replaced");
        // The final sweep ran with A's cache root gone and accepted the
        // clone's link, because B retains A.
        assert_eq!(cache.emit.original(clone), Some(original));
        assert!(cache.entries[&outer_key]
            .owner
            .view()
            .node(original)
            .is_ok());
        assert!(
            cache.entries[&inner_key]
                .owner
                .view()
                .node(original)
                .is_err(),
            "the replacing generation never cloned from A"
        );
    }
    assert_eq!(counters.snapshot(), initial);
}

#[test]
fn storage_outlives_its_cache_roots_and_a_returned_frame_while_a_factory_holds_it() {
    let counters = Counters::new();
    let initial = counters.snapshot();
    {
        let Generations {
            mut checker,
            scope,
            outer,
            original,
            clone,
            empty,
            ..
        } = generations(&counters);
        let outer_key = key(&scope, outer);
        // A returned handle: the published frame, which carries its closure.
        let returned = checker.display_builder.entries[&outer_key].owner.clone();

        let text = NodeBuilder::with_cached(&mut checker, 0, |builder| {
            enter(builder, &scope);
            let node = builder.type_node(outer)?;
            let active = counters.snapshot();
            // Cache roots first. A frame is active, so nothing is swept yet.
            remove_roots(builder.checker, original)?;
            remove_roots(builder.checker, clone)?;
            assert!(builder.checker.display_builder.entries.is_empty());
            assert_eq!(counters.snapshot(), active);
            // Then the returned handle. The factory is the last root.
            drop(returned);
            assert_eq!(counters.snapshot(), active);
            assert!(builder.ast.view().node(original).is_ok());
            assert!(builder.ast.view().node(clone).is_ok());
            assert_eq!(builder.emit.most_original(clone), original);
            print(builder, node)
        })
        .unwrap();
        assert_eq!(text.as_bytes(), b"{ b: { a: number; }; }");
        // The request ended with nothing to publish: every generation is gone
        // and the side tables hold no key into a reclaimed arena.
        assert_eq!(counters.snapshot(), empty);
        assert_eq!(checker.display_builder.emit.original(clone), None);
        assert_eq!(checker.display_builder.emit.metadata_entries(), 0);
    }
    assert_eq!(counters.snapshot(), initial);
}

#[test]
fn storage_is_reclaimed_generation_by_generation_as_the_last_root_of_each_drops() {
    let counters = Counters::new();
    let initial = counters.snapshot();
    {
        let Generations {
            mut checker,
            scope,
            inner,
            outer,
            original,
            clone,
            empty,
            with_a,
            with_both,
        } = generations(&counters);
        let (inner_key, outer_key) = (key(&scope, inner), key(&scope, outer));
        let returned = checker.display_builder.entries[&outer_key].owner.clone();

        // The factory root comes and goes first.
        NodeBuilder::with_cached(&mut checker, 0, |builder| {
            enter(builder, &scope);
            let node = builder.type_node(outer)?;
            print(builder, node)
        })
        .unwrap();
        assert_eq!(counters.snapshot(), with_both);

        // B's cache roots next. The returned frame still holds B and, through
        // it, A; the side tables follow the cache roots, so B's keys are swept
        // while A's entry keeps A's.
        assert!(remove_roots(&mut checker, clone).unwrap() >= 1);
        assert_eq!(counters.snapshot(), with_both);
        assert!(returned.view().node(clone).is_ok());
        assert!(returned.view().node(original).is_ok());
        assert_eq!(checker.display_builder.emit.original(clone), None);

        // The returned frame was B's last root. A stays for its own entry.
        drop(returned);
        assert_eq!(counters.snapshot(), with_a);
        assert!(checker.display_builder.entries[&inner_key]
            .owner
            .view()
            .node(original)
            .is_ok());

        assert_eq!(remove_roots(&mut checker, original).unwrap(), 1);
        assert_eq!(counters.snapshot(), empty);
        assert_eq!(checker.display_builder.emit.metadata_entries(), 0);
    }
    assert_eq!(counters.snapshot(), initial);
}
