"""Access-only additions to an immutable A0-b source copy, never the checkout."""
from pathlib import Path
import difflib
import json

MARKER = '// S07-bis untimed owner census: staged access-only observer.'
ADDITIONS = {
'crates/tsr_arena/src/arena.rs': '''
#[cfg(feature = "owner-census")]
impl<T> Arena<T> {
    pub(crate) fn owner_census(&self) -> [usize; 5] {
        [self.len, self.pages.iter().map(|p| p.values.capacity()).sum(),
         self.pages.len(), self.pages.capacity(), std::mem::size_of::<T>()]
    }
}
''',
'crates/tsr_arena/src/owned.rs': '''
#[cfg(feature = "owner-census")]
impl<T> OwnedArena<T> {
    pub fn owner_census(&self) -> [usize; 5] { self.0.owner_census() }
}
#[cfg(feature = "owner-census")]
impl<T> SymbolArena<T> {
    pub fn owner_census(&self) -> [usize; 5] { self.0.owner_census() }
}
''',
'crates/tsr_arena/src/file.rs': '''
#[cfg(feature = "owner-census")]
impl<N: NodeRecord, S> StorageView<'_, N, S> {
    pub fn owner_census(&self) -> [[usize; 5]; 2] {
        [self.owner.core.owner_census(), self.owner.auxiliary.owner_census()]
    }
    pub fn owner_census_lazy(&self) -> [usize; 4] {
        self.owner.lazy.owner_census()
    }
}
''',
'crates/tsr_arena/src/lazy.rs': '''
#[cfg(feature = "owner-census")]
impl<N: NodeRecord> LazyArena<N> {
    pub(crate) fn owner_census(&self) -> [usize; 4] {
        let state = self.read();
        [state.pages.reserved, state.auxiliary.reserved,
         state.pages.directory.iter().map(|p| p.slots.iter().filter(|v| v.get().is_some()).count()).sum(),
         state.auxiliary.directory.iter().map(|p| p.slots.iter().filter(|v| v.get().is_some()).count()).sum()]
    }
}
''',
'crates/tsr_arena/src/node_slots.rs': '''
#[cfg(feature = "owner-census")]
impl<T> NodeSlots<T> {
    pub fn owner_census(&self) -> [usize; 6] {
        [self.len, self.pages.iter().filter(|p| p.is_some()).count(),
         self.pages.len(), self.pages.capacity(), self.foreign.len(), self.foreign.capacity()]
    }
}
''',
'crates/tsr_ast/src/storage.rs': '''
#[cfg(feature = "owner-census")]
impl<'a> AstView<'a> {
    pub fn owner_census_nodes(self) -> impl Iterator<Item = &'a Node> { self.0.core_nodes() }
    pub fn owner_census_aux(self) -> impl Iterator<Item = &'a AstStorageData> { self.0.core_auxiliary() }
    pub fn owner_census_arenas(self) -> [[usize; 5]; 2] { self.0.owner_census() }
    pub fn owner_census_lazy(self) -> [usize; 4] { self.0.owner_census_lazy() }
}
''',
'crates/tsr_ast/src/symbols.rs': '''
#[cfg(feature = "owner-census")]
impl SymbolTables { pub fn owner_census(&self) -> [usize; 5] { self.0.owner_census() } }
#[cfg(feature = "owner-census")]
impl DeclarationLists { pub fn owner_census(&self) -> [usize; 5] { self.0.owner_census() } }
''',
'crates/tsr_ast/src/flow.rs': '''
#[cfg(feature = "owner-census")]
impl FlowNodes { pub fn owner_census(&self) -> [usize; 5] { self.0.owner_census() } }
#[cfg(feature = "owner-census")]
impl FlowLists { pub fn owner_census(&self) -> [usize; 5] { self.0.owner_census() } }
''',
'crates/tsr_ast/src/bind_result.rs': '''
#[cfg(feature = "owner-census")]
impl BindResult {
    pub fn owner_census_maps(&self) -> [[usize; 2]; 2] {
        [[self.nodes.len(), self.nodes.capacity()], [self.bindings.len(), self.bindings.capacity()]]
    }
    pub fn owner_census_flow_slots(&self) -> [usize; 6] { self.flow_bindings.owner_census() }
}
''',
}


def apply(stage):
    stage = Path(stage)
    if (stage / 'owner-census.patch').exists():
        raise ValueError('observer already applied')
    changes = dict(ADDITIONS)
    originals = {name: (stage / name).read_text() for name in changes}
    for name, addition in changes.items():
        if MARKER in originals[name]:
            raise ValueError('observer marker already exists')
        changes[name] = originals[name] + '\n' + MARKER + '\n' + addition
    for crate, declaration in [('tsr_ast', 'owner-census = ["tsr_arena/owner-census"]'), ('tsr_arena', 'owner-census = []')]:
        name = f'crates/{crate}/Cargo.toml'
        original = (stage / name).read_text()
        if original.count('[features]\n') != 1 or 'owner-census' in original:
            raise ValueError('feature inventory changed')
        originals[name] = original
        changes[name] = original.replace('[features]\n', '[features]\n' + declaration + '\n')
    # Frozen benchmark source closure has only these members; preserve workspace
    # package/lint/profile settings while avoiding nonexistent unrelated crates.
    name = 'Cargo.toml'
    original = (stage / name).read_text()
    members = sorted(str(p.parent.relative_to(stage)) for p in (stage / 'crates').glob('*/Cargo.toml'))
    lines = original.splitlines(keepends=True)
    selected = [i for i, line in enumerate(lines) if line.startswith('members = ')]
    if len(selected) != 1:
        raise ValueError('workspace member declaration changed')
    lines[selected[0]] = 'members = ' + json.dumps(members) + '\n'
    originals[name] = original
    changes[name] = ''.join(lines)
    patch = []
    for name, updated in sorted(changes.items()):
        patch.extend(difflib.unified_diff(originals[name].splitlines(keepends=True), updated.splitlines(keepends=True), fromfile=name, tofile=name))
        (stage / name).write_text(updated)
    (stage / 'owner-census.patch').write_text(''.join(patch))
    return sorted(changes)
