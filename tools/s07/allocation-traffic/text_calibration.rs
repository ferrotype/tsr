
pub(crate) fn calibrate_text_pool_traffic(snapshot: &impl Fn() -> [usize; 2]) {
    use crate::allocation_traffic_calibration::assert_accounting;
    use tsr_arena::allocation_traffic as traffic;
    // Keep the shared text bytes outside this fixture: only entry/free-slot Vec
    // backing belongs to these two families, never the nested JsString storage.
    let value = JsString::from_bytes(b"shared fixture bytes".as_slice());
    traffic::reset();
    let before = snapshot();
    let mut pool = TextPool::default();
    for row in 0..2051 {
        assert_eq!(pool.insert_pool(FieldKey::new(1, row, 0), value.clone(), POOL_LIMIT), row * 2 + 1);
    }
    assert_accounting(snapshot, before);
    for row in 0..2051 { pool.release(FieldKey::new(1, row, 0), row * 2 + 1); }
    assert_accounting(snapshot, before);
    let requests = snapshot()[0];
    for row in 0..2051 { pool.insert_pool(FieldKey::new(1, row, 0), value.clone(), POOL_LIMIT); }
    assert_eq!(snapshot()[0], requests, "free-slot reuse adds no backing request");
    assert_accounting(snapshot, before);
    drop(pool);
    assert_accounting(snapshot, before);
    assert_eq!(snapshot()[1], before[1], "text pool backing released");
}
