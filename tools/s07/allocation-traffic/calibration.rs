//! Run with one thread, before the workload, against the actual cap allocator.
use tsr_arena::allocation_traffic as traffic;

pub(crate) fn assert_accounting(snapshot: &impl Fn() -> [usize; 2], before: [usize; 2]) {
    let after = snapshot();
    let observed = traffic::sum();
    assert_eq!(after[0] - before[0], observed[0] as usize, "actual requested layouts");
    assert_eq!(after[1] as i64 - before[1] as i64,
        observed[0] as i64 - observed[1] as i64 - observed[3] as i64, "live backing at this checkpoint");
}

pub fn calibrate_allocation_traffic(snapshot: impl Fn() -> [usize; 2]) {
    fn check(snapshot: &impl Fn() -> [usize; 2], run: impl FnOnce(&dyn Fn())) {
        traffic::reset();
        let before = snapshot();
        run(&|| assert_accounting(snapshot, before));
        assert_accounting(snapshot, before);
        assert_eq!(snapshot()[1], before[1], "all fixture backing dropped");
    }
    assert_eq!(size_of::<crate::StoredAux>(), 8);
    assert_eq!(traffic::arena_family::<crate::StoredAux>(), 2);
    assert_eq!(traffic::arena_family::<crate::compact::StoredNode>(), 0);
    traffic::set_phase(1);
    check(&snapshot, |checkpoint| {
        let mut rows = crate::compact::RowPages::<u64>::default();
        for n in 0..2051 { rows.push(n); }
        checkpoint();
        drop(rows);
        checkpoint();
    });
    let owner = crate::AstBuilder::new(tsr_jsstring::SourceText::default(), &tsr_arena::Counters::new()).id().arena();
    let node = crate::NodeId::from_parts(owner, 1).unwrap();
    check(&snapshot, |checkpoint| {
        let mut edges = crate::compact::lists::EdgePages::default();
        edges.append_iter(owner, std::iter::repeat_n(Some(node), 4097)).unwrap();
        checkpoint();
        drop(edges);
        checkpoint();
    });
    let counters = tsr_arena::Counters::new();
    check(&snapshot, |checkpoint| {
        let mut values = tsr_arena::OwnedArena::new(&counters);
        for n in 0_u64..4097 { values.push(n); }
        checkpoint();
        drop(values);
        checkpoint();
    });
    // Calibrate the generic capacity observer's shrink semantics separately from
    // the grow-only page policies. cap charges the replacement requested layout.
    check(&snapshot, |checkpoint| {
        let mut values = Vec::<u64>::with_capacity(128);
        traffic::growth(10, 0, &values);
        values.extend(0..65);
        checkpoint();
        let old = values.capacity();
        values.shrink_to_fit();
        traffic::growth(10, old, &values);
        checkpoint();
        let bytes = values.capacity() * size_of::<u64>();
        drop(values);
        traffic::release(10, bytes);
        checkpoint();
    });
    crate::compact::calibrate_text_pool_traffic(&snapshot);
    let mut nodes = Vec::with_capacity(128);
    nodes.extend(std::iter::repeat_n(node, 65));
    let before = snapshot();
    let values: Vec<Option<crate::NodeId>> = nodes.into_iter().map(Some).collect();
    let after = snapshot();
    assert_eq!(before, after, "parser input map reuses its backing without allocating");
    std::hint::black_box(values);
    traffic::set_phase(0);
}
