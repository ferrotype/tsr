//! Eager parser buffers retain/drop/transfer their original requested Vec layout.
use crate::list_buffer::ListBuffer;
use tsr_arena::allocation_traffic as traffic;
use tsr_ast::NodeId;

fn assert_accounting(snapshot: &impl Fn() -> [usize; 2], before: [usize; 2]) {
    let after = snapshot();
    let counts = traffic::sum();
    assert_eq!(after[0] - before[0], counts[0] as usize);
    assert_eq!(after[1] as i64 - before[1] as i64,
        counts[0] as i64 - counts[1] as i64 - counts[3] as i64);
}

pub fn calibrate_parser_list_traffic(snapshot: impl Fn() -> [usize; 2]) {
    // The extra diagnostic enum case and Drop implementation must not change the
    // frozen 64-bit ListBuffer's 48-byte representation or its Vec capacity policy.
    assert_eq!(size_of::<ListBuffer>(), 48);
    assert_eq!(align_of::<ListBuffer>(), 8);
    let owner = tsr_ast::AstBuilder::new(tsr_jsstring::SourceText::default(), &tsr_arena::Counters::new()).id().arena();
    let node = NodeId::from_parts(owner, 1).unwrap();
    traffic::set_phase(1);
    for count in [0, 4, 5, 65, 2051] {
        traffic::reset();
        let before = snapshot();
        let mut buffer = ListBuffer::inline();
        for _ in 0..count { buffer.push(node); }
        assert_accounting(&snapshot, before);
        // Same destructor as parse_delimited_list's early None return, including
        // spilled buffers. No finish_list_buffer callback runs in that path.
        drop(buffer);
        assert_accounting(&snapshot, before);
        assert_eq!(snapshot()[1], before[1]);
    }
    for count in [5, 65, 2051] {
        traffic::reset();
        let before = snapshot();
        let mut buffer = ListBuffer::inline();
        for _ in 0..count { buffer.push(node); }
        assert_accounting(&snapshot, before);
        let at_transfer = snapshot();
        buffer.diagnostic_consume_eager(|values| {
            let values: Vec<Option<NodeId>> = values.into_iter().map(Some).collect();
            assert_eq!(snapshot(), at_transfer, "map preserves the allocation until consumption");
            std::hint::black_box(values);
        });
        assert_accounting(&snapshot, before);
        assert_eq!(snapshot()[1], before[1]);
    }
    // Appending copies suffix cells but keeps the suffix's separate backing.
    // That caller-owned vector is explicitly outside the observed family.
    let mut suffix = vec![node; 65];
    traffic::reset();
    let before = snapshot();
    let mut buffer = ListBuffer::inline();
    buffer.push(node);
    buffer.append(&mut suffix);
    assert!(suffix.is_empty());
    assert_accounting(&snapshot, before);
    drop(buffer);
    assert_accounting(&snapshot, before);
    assert_eq!(snapshot()[1], before[1]);

    traffic::reset();
    let mut lazy = ListBuffer::heap();
    for _ in 0..65 { lazy.push(node); }
    drop(lazy.into_vec().into_boxed_slice());
    assert_eq!(traffic::sum(), [0; 4], "lazy/default buffers and boxed transfers remain unclassified");
    traffic::set_phase(0);
}
