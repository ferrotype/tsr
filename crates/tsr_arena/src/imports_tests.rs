use crate::{Counters, Error, Node, StorageBuilder, StorageBundle, StorageCensus, StorageImports};
use std::sync::Arc;

fn builder(counters: &Counters) -> StorageBuilder<Node<()>> {
    StorageBuilder::new(Arc::from(&b""[..]), counters)
}

#[test]
fn many_outputs_share_owners_without_reacquiring_every_file() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let mut input = builder(&counters);
    let node = input.push(Node::new(7, ()));
    let input = input.finish();
    let root = input.file_owner().unwrap();
    let weak = Arc::downgrade(&root);
    let imports = StorageImports::new([input]);
    let references = Arc::strong_count(&root);
    let outputs: Vec<_> = (0..64)
        .map(|_| {
            let mut output = builder(&counters);
            output.retain_imports(&imports);
            output.finish()
        })
        .collect();
    assert_eq!(Arc::strong_count(&root), references);
    drop(root);
    drop(imports);
    for output in &outputs {
        assert_eq!(output.view().node(node).unwrap().kind, 7);
    }
    assert!(weak.upgrade().is_some());
    drop(outputs);
    assert!(weak.upgrade().is_none());
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn extending_one_output_preserves_other_outputs_and_owner_boundaries() {
    let counters = Counters::new();
    let mut first = builder(&counters);
    let a = first.push(Node::new(1, ()));
    let mut second = builder(&counters);
    let b = second.push(Node::new(2, ()));
    let second = second.finish();
    let imports = StorageImports::new([first.finish()]);
    let mut left = builder(&counters);
    let mut right = builder(&counters);
    left.retain_imports(&imports);
    right.retain_imports(&imports);
    left.retain_file(second);
    assert_eq!(left.view().node(b).unwrap().kind, 2);
    assert!(matches!(right.view().node(b), Err(Error::WrongOwner)));
    // A's own retained graph must not gain B from the caller's wider lookup.
    let selected = left
        .view()
        .for_node_owner(a)
        .unwrap()
        .owner_retention()
        .unwrap();
    assert!(matches!(selected.node(b), Err(Error::WrongOwner)));
    // Publishing and then importing an output still retains the whole closure.
    let mut parent = builder(&counters);
    parent.retain_file(left.finish());
    drop(imports);
    drop(right);
    assert_eq!(parent.view().node(a).unwrap().kind, 1);
    assert_eq!(parent.view().node(b).unwrap().kind, 2);
}

#[test]
fn shared_imports_keep_mapped_siblings_and_their_dependencies_alive() {
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let mut dependency = builder(&counters);
    let d = dependency.push(Node::new(3, ()));
    let mut canonical = builder(&counters);
    let c = canonical.push(Node::new(1, ()));
    let mut supplemental = builder(&counters);
    let s = supplemental.push(Node::new(2, ()));
    supplemental.retain_file(dependency.finish());
    let bundle = StorageBundle::new(canonical, vec![supplemental]);
    let canonical_id = bundle.file(0).unwrap().id();
    let imports = StorageImports::new([bundle.file(1).unwrap()]);
    let mut output = builder(&counters);
    output.retain_imports(&imports);
    let output = output.finish();
    drop(imports);
    drop(bundle);
    for (id, kind) in [(c, 1), (s, 2), (d, 3)] {
        assert_eq!(output.view().node(id).unwrap().kind, kind);
    }
    assert_eq!(
        output
            .file(canonical_id)
            .unwrap()
            .view()
            .node(s)
            .unwrap()
            .kind,
        2
    );
    drop(output);
    assert_eq!(counters.snapshot(), baseline);
}

#[test]
fn census_counts_a_shared_index_once_but_counts_each_output() {
    let counters = Counters::new();
    let imports = StorageImports::new([builder(&counters).finish()]);
    let mut a = builder(&counters);
    let mut b = builder(&counters);
    a.retain_imports(&imports);
    b.retain_imports(&imports);
    let a = a.finish();
    let b = b.finish();
    let empty = builder(&counters).finish();
    let mut census = StorageCensus::default();
    let first = a.structural_bytes_with(&|(), _| (0, 0), &mut census);
    let second = b.structural_bytes_with(&|(), _| (0, 0), &mut census);
    assert_eq!(second, empty.structural_bytes(|()| (0, 0)));
    assert!(first.0 > second.0);
}
