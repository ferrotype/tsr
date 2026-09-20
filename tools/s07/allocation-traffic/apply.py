"""Instrument backing growth in a frozen stage; never edit production crates."""
import hashlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def patch(source):
    source = Path(source).resolve()
    assert source != ROOT and source.is_relative_to(ROOT / 'target')
    before, after = {}, {}

    def edit(name, replacements):
        path = source / name
        original = path.read_text()
        value = original
        for old, new in replacements:
            assert value.count(old) == 1, (name, old[:90], value.count(old))
            value = value.replace(old, new)
        before.setdefault(name, hashlib.sha256(original.encode()).hexdigest())
        path.write_text(value)
        after[name] = hashlib.sha256(value.encode()).hexdigest()

    edit('crates/tsr_arena/src/lib.rs', [('mod arena;', 'mod arena;\npub mod allocation_traffic;')])
    path = source / 'crates/tsr_arena/src/allocation_traffic.rs'
    assert not path.exists()
    path.write_bytes((HERE / 'counters.rs').read_bytes())
    after[str(path.relative_to(source))] = hashlib.sha256(path.read_bytes()).hexdigest()
    edit('crates/tsr_arena/src/arena.rs', [
        ('            self.pages.push(Page {', '            let old_directory = self.pages.capacity();\n            self.pages.push(Page {'),
        ('        debug_assert_eq!(self.pages[page].values.len(), offset);',
         '''        if offset == 0 {
            let family = crate::allocation_traffic::arena_family::<T>();
            crate::allocation_traffic::allocation(family, self.pages[page].values.capacity() * size_of::<T>(), 0);
        }
        debug_assert_eq!(self.pages[page].values.len(), offset);'''),
        ('                _allocation: self.counters.allocation(),\n            });',
         '''                _allocation: self.counters.allocation(),
            });
            crate::allocation_traffic::growth(crate::allocation_traffic::arena_family::<T>() + 1, old_directory, &self.pages);''')])
    edit('crates/tsr_ast/src/compact/pages.rs', [
        ('        if ordinal.is_multiple_of(4) {', '''        if ordinal.is_multiple_of(4) {
            let old_directory = match &self.directory { Directory::Many(v) => v.capacity(), _ => 0 };
            tsr_arena::allocation_traffic::allocation(12, size_of::<[T; 4]>(), 0);'''),
        ('        self.len = next;', '''        if ordinal.is_multiple_of(4) {
            // Directory::One owns its page inline in the enum; only Many allocates a vector.
            // The old capacity is captured in the same allocation branch below.
        }
        self.len = next;'''),
        ('            };\n        }\n        if ordinal.is_multiple_of(4)', '''            };
            if let Directory::Many(values) = &self.directory {
                tsr_arena::allocation_traffic::growth(13, old_directory, values);
            }
        }
        if ordinal.is_multiple_of(4)''')])
    # Remove the placeholder after locating the original branch exactly.
    edit('crates/tsr_ast/src/compact/pages.rs', [('''        if ordinal.is_multiple_of(4) {
            // Directory::One owns its page inline in the enum; only Many allocates a vector.
            // The old capacity is captured in the same allocation branch below.
        }
''', '')])
    edit('crates/tsr_ast/src/compact/lists.rs', [
        ('                self.pages.push(Box::new([0; PAGE_WORDS]));', '''                let old = self.pages.capacity();
                self.pages.push(Box::new([0; PAGE_WORDS]));
                tsr_arena::allocation_traffic::allocation(14, size_of::<[u32; PAGE_WORDS]>(), 0);
                tsr_arena::allocation_traffic::growth(15, old, &self.pages);''')])
    edit('crates/tsr_ast/src/compact/text.rs', [
        ('            self.entries.push(Some(value));', '''            let old = self.entries.capacity();
            self.entries.push(Some(value));
            tsr_arena::allocation_traffic::growth(16, old, &self.entries);'''),
        ('            self.free.push(index);', '''            let old = self.free.capacity();
            self.free.push(index);
            tsr_arena::allocation_traffic::growth(17, old, &self.free);''')])
    edit('crates/tsr_parser/src/list_buffer.rs', [
        ('    Heap(Vec<NodeId>),', '    Heap(Vec<NodeId>),\n    UntrackedHeap(Vec<NodeId>),'),
        ('        Self::Heap(Vec::new())', '        Self::UntrackedHeap(Vec::new())'),
        ('            Self::Heap(nodes) => nodes.len(),', '            Self::Heap(nodes) | Self::UntrackedHeap(nodes) => nodes.len(),'),
        ('            Self::Heap(_) => None,', '            Self::Heap(_) | Self::UntrackedHeap(_) => None,'),
        ('    pub(crate) fn push(&mut self, node: NodeId) {', '''    pub(crate) fn diagnostic_capacity(&self) -> usize {
        match self { Self::Heap(values) => values.capacity(), Self::Inline { .. } | Self::UntrackedHeap(_) => 0 }
    }
    pub(crate) fn diagnostic_consume_eager<R>(mut self, consume: impl FnOnce(Vec<NodeId>) -> R) -> R {
        let Self::Heap(values) = &mut self else { unreachable!("eager spill owns tracked backing") };
        let values = std::mem::take(values);
        let _release = DiagnosticBackingRelease(values.capacity() * size_of::<NodeId>());
        consume(values)
    }
    pub(crate) fn push(&mut self, node: NodeId) {
        let old = self.diagnostic_capacity();'''),
        ('    pub(crate) fn append(&mut self, suffix: &mut Vec<NodeId>) {', '''    pub(crate) fn append(&mut self, suffix: &mut Vec<NodeId>) {
        let old = self.diagnostic_capacity();'''),
        ('            Self::Heap(nodes) => nodes.push(node),\n        }', '''            Self::Heap(nodes) | Self::UntrackedHeap(nodes) => nodes.push(node),
        }
        tsr_arena::allocation_traffic::allocation(18, self.diagnostic_capacity()*size_of::<NodeId>(), old*size_of::<NodeId>());'''),
        ('            Self::Heap(nodes) => nodes.append(suffix),\n        }', '''            Self::Heap(nodes) | Self::UntrackedHeap(nodes) => nodes.append(suffix),
        }
        tsr_arena::allocation_traffic::allocation(18, self.diagnostic_capacity()*size_of::<NodeId>(), old*size_of::<NodeId>());'''),
        ('    pub(crate) fn into_vec(self) -> Vec<NodeId> {\n        match self {\n            Self::Heap(nodes) => nodes,', '''    pub(crate) fn into_vec(mut self) -> Vec<NodeId> {
        // Only the untracked default/lazy path transfers backing through this
        // method in production. Eager spills use diagnostic_consume_eager.
        match &mut self {
            Self::Heap(nodes) | Self::UntrackedHeap(nodes) => std::mem::take(nodes),'''),
        ('Vec::with_capacity(if len == 0 { 0 } else { 4 })', 'Vec::with_capacity(if *len == 0 { 0 } else { 4 })'),
        ('                        .into_iter()\n                        .take(len)', '                        .iter()\n                        .take(*len)')])
    edit('crates/tsr_parser/src/factory.rs', [
        ('''            self.node_slice_from_slice(values)
                .expect("factory slice edges")
        } else {
            self.alloc_nodes(nodes.into_vec().into_iter().map(Some).collect())''',
         '''            self.node_slice_from_slice(values)
                .expect("factory slice edges")
        } else {
            nodes.diagnostic_consume_eager(|values| {
                self.alloc_nodes(values.into_iter().map(Some).collect())
            })''')])
    edit('crates/tsr_parser/src/factory_tests.rs', [
        ('    assert!(matches!(buffer, ListBuffer::Heap(_)));\n    for &id in ids {', '    assert!(matches!(buffer, ListBuffer::UntrackedHeap(_)));\n    for &id in ids {'),
        ('    let ListBuffer::Heap(nodes) = &buffer else {', '    let ListBuffer::UntrackedHeap(nodes) = &buffer else {'),
        ('            assert!(matches!(buffer, ListBuffer::Heap(_)));', '            assert!(matches!(buffer, ListBuffer::UntrackedHeap(_)));')])
    edit('crates/tsr_parser/src/lists_tests.rs', [
        ('        let mut parsed = 0;\n        let result = parser.parse_delimited_list', '''        let mut parsed = 0;
        let traffic_before = tsr_arena::allocation_traffic::family_sum(18);
        let result = parser.parse_delimited_list'''),
        ('        assert_eq!(result, None);\n        assert_eq!(parsed, accepted_count);', '''        assert_eq!(result, None);
        let traffic_after = tsr_arena::allocation_traffic::family_sum(18);
        let delta: [u64; 4] = std::array::from_fn(|i| traffic_after[i] - traffic_before[i]);
        assert_eq!(delta[0], delta[1] + delta[3], "aborted eager list releases all requested backing");
        assert_eq!(delta[0] > 0, accepted_count > 4);
        assert_eq!(parsed, accepted_count);''')])
    def append(name, value):
        path = source / name
        original = path.read_text()
        before.setdefault(name, hashlib.sha256(original.encode()).hexdigest())
        path.write_text(original + value)
        after[name] = hashlib.sha256(path.read_bytes()).hexdigest()
    append('crates/tsr_parser/src/list_buffer.rs', '''
impl Drop for ListBuffer {
    fn drop(&mut self) {
        tsr_arena::allocation_traffic::release(18, self.diagnostic_capacity() * size_of::<NodeId>());
    }
}
struct DiagnosticBackingRelease(usize);
impl Drop for DiagnosticBackingRelease {
    fn drop(&mut self) { tsr_arena::allocation_traffic::release(18, self.0); }
}
#[cfg(test)]
mod traffic_tests {
    use super::*;
    #[test]
    fn eager_transfer_unwind_releases_once() {
        let owner = tsr_ast::AstBuilder::new(tsr_jsstring::SourceText::default(), &tsr_arena::Counters::new()).id().arena();
        let node = NodeId::from_parts(owner, 1).unwrap();
        let before = tsr_arena::allocation_traffic::family_sum(18);
        let outcome = std::panic::catch_unwind(|| {
            let mut buffer = ListBuffer::inline();
            for _ in 0..65 { buffer.push(node); }
            buffer.diagnostic_consume_eager(|values| {
                std::hint::black_box(values);
                panic!("intentional consumed-buffer unwind");
            });
        });
        assert!(outcome.is_err());
        let after = tsr_arena::allocation_traffic::family_sum(18);
        let delta: [u64; 4] = std::array::from_fn(|i| after[i] - before[i]);
        assert_eq!(delta[0], delta[1] + delta[3]);
        assert_eq!(delta[3], 128 * size_of::<NodeId>() as u64);
    }
}
''')
    append('crates/tsr_arena/src/arena.rs', '''
impl<T> Drop for Arena<T> {
    fn drop(&mut self) {
        let family = crate::allocation_traffic::arena_family::<T>();
        for page in &self.pages {
            crate::allocation_traffic::release(family, page.values.capacity() * size_of::<T>());
        }
        crate::allocation_traffic::release(family + 1, self.pages.capacity() * size_of::<Page<T>>());
    }
}
''')
    append('crates/tsr_ast/src/compact/pages.rs', '''
impl<T> Drop for RowPages<T> {
    fn drop(&mut self) {
        let pages = match &self.directory {
            Directory::Empty => 0,
            Directory::One(_) => 1,
            Directory::Many(values) => {
                tsr_arena::allocation_traffic::release(13, values.capacity() * size_of::<Box<[T; 4]>>());
                values.len()
            }
        };
        tsr_arena::allocation_traffic::release(12, pages * size_of::<[T; 4]>());
    }
}
''')
    append('crates/tsr_ast/src/compact/lists.rs', '''
impl Drop for EdgePages {
    fn drop(&mut self) {
        tsr_arena::allocation_traffic::release(14, self.pages.len() * size_of::<[u32; PAGE_WORDS]>());
        tsr_arena::allocation_traffic::release(15, self.pages.capacity() * size_of::<Box<[u32; PAGE_WORDS]>>());
    }
}
''')
    append('crates/tsr_ast/src/compact/text.rs', '''
impl Drop for TextPool {
    fn drop(&mut self) {
        tsr_arena::allocation_traffic::release(16, self.entries.capacity() * size_of::<Option<JsString>>());
        tsr_arena::allocation_traffic::release(17, self.free.capacity() * size_of::<u32>());
    }
}
''')
    append('crates/tsr_ast/src/compact/text.rs', (HERE / 'text_calibration.rs').read_text())
    edit('crates/tsr_ast/src/compact/mod.rs', [('mod text;', 'mod text;\npub(crate) use text::calibrate_text_pool_traffic;')])
    return {'before': before, 'after': after}


def apply(source):
    """Compose the hash-verified existing name observer with the new backing observer."""
    import importlib.util
    import json
    import tarfile
    import tempfile
    archive = ROOT / 'tools/s07/performance-experiments/results/2026-09-09-name-table-attribution'
    manifest = json.loads((archive/'archive.json').read_text())
    packed = archive/'review.tar.xz'
    assert hashlib.sha256(packed.read_bytes()).hexdigest() == manifest['archive']['sha256']
    with tempfile.TemporaryDirectory(prefix='traffic-name-observer-', dir=ROOT/'target') as directory:
        directory = Path(directory)
        with tarfile.open(packed, 'r:xz') as contents:
            for name in ['apply.py', 'symbol_tables.rs', 'module-source.json', 'name_table_probe.rs']:
                member = 'target/s07-bis/name-table-adapter/' + name
                data = contents.extractfile(member).read()
                assert hashlib.sha256(data).hexdigest() == manifest['members'][member]['sha256']
                (directory/name).write_bytes(data)
        spec = importlib.util.spec_from_file_location('verified_name_observer', directory/'apply.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        receipt = module.apply(source)
    additional = patch(source)
    for key in ['before', 'after']:
        receipt[key].update(additional[key])
    main = source/'crates/tsr_bench/src/main.rs'
    value = main.read_text()
    assert value.count('let parsed = tsr_parser::parse_source_file(') == 1
    value = value.replace('let parsed = tsr_parser::parse_source_file(', '''tsr_arena::allocation_traffic::set_phase(1);
                        let parse_before = [ALLOCATOR.total_allocated(), ALLOCATOR.allocated()];
                        let parsed = tsr_parser::parse_source_file(''')
    value = value.replace('tsr_binder::bind_parsed_file(parsed).expect("workload binding must complete")', '''let parse_after = [ALLOCATOR.total_allocated(), ALLOCATOR.allocated()];
                        tsr_arena::allocation_traffic::window(1, parse_before, parse_after);
                        tsr_arena::allocation_traffic::set_phase(2);
                        let bind_before = [ALLOCATOR.total_allocated(), ALLOCATOR.allocated()];
                        let bound = tsr_binder::bind_parsed_file(parsed).expect("workload binding must complete");
                        let bind_after = [ALLOCATOR.total_allocated(), ALLOCATOR.allocated()];
                        tsr_arena::allocation_traffic::window(2, bind_before, bind_after);
                        tsr_arena::allocation_traffic::set_phase(0);
                        bound''')
    value = value.replace('tsr_ast::SymbolTables::diagnostic_reset_traffic();', 'tsr_ast::SymbolTables::diagnostic_reset_traffic();\n        tsr_arena::allocation_traffic::reset();')
    value = value.replace('        release.wait();\n        let files:', '''        let backing_traffic = tsr_arena::allocation_traffic::snapshot();
        let phase_windows = tsr_arena::allocation_traffic::windows();
        release.wait();
        let files:''')
    value = value.replace('"domain": "name-table-requested-memory",', '''"domain": "current-backing-traffic",
                "backing_families": tsr_arena::allocation_traffic::FAMILIES,
                "backing_phases": tsr_arena::allocation_traffic::PHASES,
                "backing_fields": tsr_arena::allocation_traffic::FIELDS,
                "backing_traffic": backing_traffic,
                "phase_process_windows": phase_windows,
                "phase_window_fields": ["requested_bytes", "positive_live_change", "negative_live_change", "files"],
                "phase_window_scope": "process-wide cap deltas while the sole worker parses/binds; concurrent main-thread traffic may be included",''')
    main.write_text(value)
    receipt['after']['crates/tsr_bench/src/main.rs'] = hashlib.sha256(main.read_bytes()).hexdigest()
    for crate, module in [('tsr_ast', 'calibrate_allocation_traffic'), ('tsr_parser', 'calibrate_parser_list_traffic')]:
        lib = source/f'crates/{crate}/src/lib.rs'
        receipt['before'][str(lib.relative_to(source))] = hashlib.sha256(lib.read_bytes()).hexdigest()
        lib.write_text(lib.read_text() + f'\nmod allocation_traffic_calibration;\npub use allocation_traffic_calibration::{module};\n')
        receipt['after'][str(lib.relative_to(source))] = hashlib.sha256(lib.read_bytes()).hexdigest()
    for name, filename in [('crates/tsr_ast/src/allocation_traffic_calibration.rs', 'calibration.rs'),
                           ('crates/tsr_parser/src/allocation_traffic_calibration.rs', 'parser_calibration.rs'),
                           ('crates/tsr_bench/examples/allocation_traffic_probe.rs', 'probe.rs')]:
        path = source/name
        assert not path.exists()
        path.write_bytes((HERE/filename).read_bytes())
        receipt['after'][name] = hashlib.sha256(path.read_bytes()).hexdigest()
    receipt['scope'] = 'current backing requests/replacements/releases plus calibrated name tables; process phase windows include possible main-thread traffic; parser list family covers eager spills only, lazy Vec-to-Box backing remains residual'
    return receipt
