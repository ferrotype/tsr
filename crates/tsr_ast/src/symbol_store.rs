//! Compact binding symbols. The surrounding bind result owns both this store
//! and its shared table/name pool; reads never retain either one independently.

use crate::symbols::{DeclarationSlice, Symbol, SymbolTableId, SymbolTables};
use crate::{JsString, NodeId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tsr_arena::{ArenaId, AuxId, Counters, Error, SymbolArena, SymbolId};

#[derive(Debug, Default)]
#[repr(C)]
struct StoredSymbol {
    flags: u32,
    check_flags: u32,
    name: u32,
    declaration_backing: u32,
    declaration_start: u32,
    declaration_len: u32,
    declaration_capacity: u32,
    value_declaration: u32,
    members: u32,
    exports: u32,
    parent: u32,
    export_symbol: u32,
    runtime_id: AtomicU64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Field {
    Name,
    Declarations,
    ValueDeclaration,
    Members,
    Exports,
    Parent,
    ExportSymbol,
}

#[derive(Clone, Copy, Debug)]
enum Escape {
    Name(usize),
    Node(NodeId),
    Backing(AuxId),
    Table(SymbolTableId),
    Symbol(SymbolId),
}

#[derive(Debug)]
pub(crate) struct Symbols {
    rows: SymbolArena<StoredSymbol>,
    references: References,
}

#[derive(Debug, Default)]
struct References {
    nodes: Option<ArenaId>,
    tables: Option<ArenaId>,
    declarations: Option<ArenaId>,
    escapes: HashMap<(u32, Field), Escape>,
}

impl Symbols {
    pub(crate) fn new(counters: &Counters) -> Self {
        Self {
            rows: SymbolArena::new(counters),
            references: References::default(),
        }
    }
    pub(crate) fn initialize_reference_arenas(
        &mut self,
        nodes: ArenaId,
        tables: ArenaId,
        declarations: ArenaId,
    ) {
        for (base, value) in [
            (&mut self.references.nodes, nodes),
            (&mut self.references.tables, tables),
            (&mut self.references.declarations, declarations),
        ] {
            assert!(base.is_none_or(|previous| previous == value));
            *base = Some(value);
        }
    }
    pub(crate) fn id(&self) -> ArenaId {
        self.rows.id()
    }
    pub(crate) fn read<'a>(&'a self, tables: &'a SymbolTables) -> SymbolsRead<'a> {
        SymbolsRead {
            store: self,
            tables,
        }
    }
    pub(crate) fn write<'a>(&'a mut self, tables: &'a mut SymbolTables) -> SymbolsMut<'a> {
        SymbolsMut {
            store: self,
            tables,
        }
    }
    fn replace(&mut self, id: SymbolId, value: Symbol, tables: &mut SymbolTables) {
        let slot = id.slot();
        let name = tables.intern_name(value.name);
        let name = self.references.encode_name(slot, name);
        let declaration_backing = self.references.encode(
            id.arena(),
            slot,
            Field::Declarations,
            value
                .declarations
                .backing_id()
                .map(|id| (id.arena(), id.slot(), Escape::Backing(id))),
        );
        let value_declaration = self.references.encode(
            id.arena(),
            slot,
            Field::ValueDeclaration,
            value
                .value_declaration
                .map(|id| (id.arena(), id.slot(), Escape::Node(id))),
        );
        let members = self.references.encode(
            id.arena(),
            slot,
            Field::Members,
            value
                .members
                .map(|id| (id.arena(), id.slot(), Escape::Table(id))),
        );
        let exports = self.references.encode(
            id.arena(),
            slot,
            Field::Exports,
            value
                .exports
                .map(|id| (id.arena(), id.slot(), Escape::Table(id))),
        );
        let parent = self.references.encode(
            id.arena(),
            slot,
            Field::Parent,
            value
                .parent
                .map(|id| (id.arena(), id.slot(), Escape::Symbol(id))),
        );
        let export_symbol = self.references.encode(
            id.arena(),
            slot,
            Field::ExportSymbol,
            value
                .export_symbol
                .map(|id| (id.arena(), id.slot(), Escape::Symbol(id))),
        );
        *self.rows.get_mut(id).expect("checked receiver") = StoredSymbol {
            flags: value.flags,
            check_flags: value.check_flags,
            name,
            declaration_backing,
            declaration_start: value.declarations.start(),
            declaration_len: u32::try_from(value.declarations.len())
                .expect("u32 declaration length"),
            declaration_capacity: u32::try_from(value.declarations.capacity())
                .expect("u32 declaration capacity"),
            value_declaration,
            members,
            exports,
            parent,
            export_symbol,
            runtime_id: AtomicU64::new(value.runtime_id.load(Ordering::SeqCst)),
        };
    }
}

impl References {
    fn clear(&mut self, slot: u32, field: Field) {
        if !self.escapes.is_empty() {
            self.escapes.remove(&(slot, field));
        }
    }
    fn encode(
        &mut self,
        symbols: ArenaId,
        slot: u32,
        field: Field,
        value: Option<(ArenaId, u32, Escape)>,
    ) -> u32 {
        self.clear(slot, field);
        let Some((arena, target, escape)) = value else {
            return 0;
        };
        let local = match field {
            Field::Declarations => *self.declarations.get_or_insert(arena),
            Field::ValueDeclaration => *self.nodes.get_or_insert(arena),
            Field::Members | Field::Exports => *self.tables.get_or_insert(arena),
            Field::Parent | Field::ExportSymbol => symbols,
            Field::Name => unreachable!("name uses its own index domain"),
        };
        if local == arena && target != u32::MAX {
            target
        } else {
            self.escapes.insert((slot, field), escape);
            u32::MAX
        }
    }
    fn encode_name(&mut self, slot: u32, name: usize) -> u32 {
        self.clear(slot, Field::Name);
        match u32::try_from(name) {
            Ok(word) if word != u32::MAX => word,
            _ => {
                self.escapes.insert((slot, Field::Name), Escape::Name(name));
                u32::MAX
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct SymbolsRead<'a> {
    store: &'a Symbols,
    tables: &'a SymbolTables,
}
impl<'a> SymbolsRead<'a> {
    pub fn id(self) -> ArenaId {
        self.store.id()
    }
    pub fn len(self) -> usize {
        self.store.rows.len()
    }
    pub fn is_empty(self) -> bool {
        self.store.rows.is_empty()
    }
    pub fn get(self, id: SymbolId) -> Result<SymbolRead<'a>, Error> {
        let row = self.store.rows.get(id)?;
        Ok(SymbolRead {
            row,
            slot: id.slot(),
            store: self.store,
            tables: self.tables,
        })
    }
    #[inline]
    pub(crate) fn get_slot(self, slot: u32) -> Result<SymbolRead<'a>, Error> {
        Ok(SymbolRead {
            row: self.store.rows.get_slot(slot)?,
            slot,
            store: self.store,
            tables: self.tables,
        })
    }
    pub fn iter(self) -> impl Iterator<Item = (SymbolId, SymbolRead<'a>)> {
        self.store.rows.iter().map(move |(id, row)| {
            (
                id,
                SymbolRead {
                    row,
                    slot: id.slot(),
                    store: self.store,
                    tables: self.tables,
                },
            )
        })
    }
}

/// A borrowed symbol resolves local words through its exact binding owner.
/// It cannot escape the bound file that retains its record and name pool.
///
/// ```compile_fail,E0515
/// use tsr_ast::{BoundFile, SymbolId, SymbolRead};
/// fn escape<'a>(file: BoundFile, id: SymbolId) -> SymbolRead<'a> {
///     file.view().symbol(id).unwrap()
/// }
/// ```
///
/// A selected name also keeps construction storage borrowed while it is used.
///
/// ```compile_fail,E0502
/// use tsr_ast::{BindBuilder, Symbol, SymbolId};
/// fn grow(builder: &mut BindBuilder<'_>, id: SymbolId, new_symbol: Symbol) {
///     let name = builder.symbols().get(id).unwrap().name_bytes();
///     builder.symbols_mut().push(new_symbol);
///     assert!(!name.is_empty());
/// }
/// ```
#[derive(Clone, Copy)]
pub struct SymbolRead<'a> {
    row: &'a StoredSymbol,
    slot: u32,
    store: &'a Symbols,
    tables: &'a SymbolTables,
}
impl<'a> SymbolRead<'a> {
    pub fn flags(self) -> u32 {
        self.row.flags
    }
    pub fn check_flags(self) -> u32 {
        self.row.check_flags
    }
    fn escape(self, field: Field) -> Escape {
        *self
            .store
            .references
            .escapes
            .get(&(self.slot, field))
            .expect("stored symbol escape")
    }
    fn name_id(self) -> usize {
        if self.row.name == u32::MAX {
            let Escape::Name(name) = self.escape(Field::Name) else {
                unreachable!()
            };
            name
        } else {
            self.row.name as usize
        }
    }
    pub fn name_bytes(self) -> &'a [u8] {
        self.tables.name_bytes(self.name_id())
    }
    pub fn name_to_owned(self) -> JsString {
        self.tables.name_to_owned(self.name_id())
    }
    pub fn declarations(self) -> DeclarationSlice {
        let backing = match self.row.declaration_backing {
            0 => None,
            u32::MAX => {
                let Escape::Backing(id) = self.escape(Field::Declarations) else {
                    unreachable!()
                };
                Some(id)
            }
            slot => Some(
                AuxId::from_parts(
                    self.store
                        .references
                        .declarations
                        .expect("stored backing namespace"),
                    slot,
                )
                .expect("nonzero stored link"),
            ),
        };
        DeclarationSlice::from_storage_parts(
            backing,
            self.row.declaration_start,
            self.row.declaration_len,
            self.row.declaration_capacity,
        )
    }
    pub fn value_declaration(self) -> Option<NodeId> {
        match self.row.value_declaration {
            0 => None,
            u32::MAX => {
                let Escape::Node(id) = self.escape(Field::ValueDeclaration) else {
                    unreachable!()
                };
                Some(id)
            }
            slot => Some(
                NodeId::from_parts(
                    self.store.references.nodes.expect("stored node namespace"),
                    slot,
                )
                .expect("nonzero stored link"),
            ),
        }
    }
    fn table(self, word: u32, field: Field) -> Option<SymbolTableId> {
        match word {
            0 => None,
            u32::MAX => {
                let Escape::Table(id) = self.escape(field) else {
                    unreachable!()
                };
                Some(id)
            }
            slot => Some(
                SymbolTableId::from_parts(
                    self.store
                        .references
                        .tables
                        .expect("stored table namespace"),
                    slot,
                )
                .expect("nonzero stored link"),
            ),
        }
    }
    pub fn members(self) -> Option<SymbolTableId> {
        self.table(self.row.members, Field::Members)
    }
    pub fn exports(self) -> Option<SymbolTableId> {
        self.table(self.row.exports, Field::Exports)
    }
    fn symbol(self, word: u32, field: Field) -> Option<SymbolId> {
        match word {
            0 => None,
            u32::MAX => {
                let Escape::Symbol(id) = self.escape(field) else {
                    unreachable!()
                };
                Some(id)
            }
            slot => Some(SymbolId::from_parts(self.store.id(), slot).expect("nonzero stored link")),
        }
    }
    pub fn parent(self) -> Option<SymbolId> {
        self.symbol(self.row.parent, Field::Parent)
    }
    pub fn export_symbol(self) -> Option<SymbolId> {
        self.symbol(self.row.export_symbol, Field::ExportSymbol)
    }
    pub(crate) fn runtime_cell(self) -> &'a AtomicU64 {
        &self.row.runtime_id
    }
    pub fn to_owned(self) -> Symbol {
        Symbol {
            flags: self.flags(),
            check_flags: self.check_flags(),
            name: self.name_to_owned(),
            declarations: self.declarations(),
            value_declaration: self.value_declaration(),
            members: self.members(),
            exports: self.exports(),
            parent: self.parent(),
            export_symbol: self.export_symbol(),
            runtime_id: AtomicU64::new(self.runtime_cell().load(Ordering::SeqCst)),
        }
    }
}
impl std::fmt::Debug for SymbolRead<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SymbolRead")
            .field("flags", &self.flags())
            .field("name", &self.name_bytes())
            .finish_non_exhaustive()
    }
}

/// Exclusive symbol access also borrows the shared name pool exclusively.
pub struct SymbolsMut<'a> {
    store: &'a mut Symbols,
    tables: &'a mut SymbolTables,
}
impl<'a> SymbolsMut<'a> {
    pub fn id(&self) -> ArenaId {
        self.store.id()
    }
    #[cfg_attr(feature = "creation-trace", track_caller)]
    pub fn push(self, value: Symbol) -> SymbolId {
        let id = self.store.rows.push(StoredSymbol::default());
        self.store.replace(id, value, self.tables);
        #[cfg(feature = "creation-trace")]
        crate::creation_trace::symbol_birth(
            &self.store.read(self.tables).get(id).expect("new symbol"),
        );
        id
    }
    pub fn get_mut(self, id: SymbolId) -> Result<SymbolMut<'a>, Error> {
        let value = self.store.read(self.tables).get(id)?.to_owned();
        Ok(SymbolMut {
            value,
            id,
            store: self.store,
            tables: self.tables,
        })
    }
    pub fn set_flags(self, id: SymbolId, value: u32) -> Result<(), Error> {
        self.store.rows.get_mut(id)?.flags = value;
        Ok(())
    }
    pub fn set_check_flags(self, id: SymbolId, value: u32) -> Result<(), Error> {
        self.store.rows.get_mut(id)?.check_flags = value;
        Ok(())
    }
    /// A flags-only borrow cannot alter reference encodings or names.
    pub fn flags_mut(self, id: SymbolId) -> Result<&'a mut u32, Error> {
        Ok(&mut self.store.rows.get_mut(id)?.flags)
    }
    pub fn check_flags_mut(self, id: SymbolId) -> Result<&'a mut u32, Error> {
        Ok(&mut self.store.rows.get_mut(id)?.check_flags)
    }
    #[inline]
    pub(crate) fn flags_slot_mut(self, slot: u32) -> Result<&'a mut u32, Error> {
        Ok(&mut self.store.rows.get_slot_mut(slot)?.flags)
    }
    #[inline]
    pub(crate) fn check_flags_slot_mut(self, slot: u32) -> Result<&'a mut u32, Error> {
        Ok(&mut self.store.rows.get_slot_mut(slot)?.check_flags)
    }
    pub fn set_declarations(self, id: SymbolId, value: DeclarationSlice) -> Result<(), Error> {
        if id.arena() != self.store.rows.id() {
            return Err(Error::WrongOwner);
        }
        self.set_declarations_slot(id.slot(), value)
    }
    #[inline]
    pub(crate) fn set_declarations_slot(
        self,
        slot: u32,
        value: DeclarationSlice,
    ) -> Result<(), Error> {
        let arena = self.store.rows.id();
        let row = self.store.rows.get_slot_mut(slot)?;
        let word = self.store.references.encode(
            arena,
            slot,
            Field::Declarations,
            value
                .backing_id()
                .map(|id| (id.arena(), id.slot(), Escape::Backing(id))),
        );
        row.declaration_backing = word;
        row.declaration_start = value.start();
        row.declaration_len = u32::try_from(value.len()).expect("u32 declaration length");
        row.declaration_capacity =
            u32::try_from(value.capacity()).expect("u32 declaration capacity");
        Ok(())
    }
    pub fn set_value_declaration(self, id: SymbolId, value: Option<NodeId>) -> Result<(), Error> {
        if id.arena() != self.store.rows.id() {
            return Err(Error::WrongOwner);
        }
        self.set_value_declaration_slot(id.slot(), value)
    }
    #[inline]
    pub(crate) fn set_value_declaration_slot(
        self,
        slot: u32,
        value: Option<NodeId>,
    ) -> Result<(), Error> {
        let arena = self.store.rows.id();
        let row = self.store.rows.get_slot_mut(slot)?;
        let word = self.store.references.encode(
            arena,
            slot,
            Field::ValueDeclaration,
            value.map(|id| (id.arena(), id.slot(), Escape::Node(id))),
        );
        row.value_declaration = word;
        Ok(())
    }
    pub fn set_members(self, id: SymbolId, value: Option<SymbolTableId>) -> Result<(), Error> {
        if id.arena() != self.store.rows.id() {
            return Err(Error::WrongOwner);
        }
        self.set_members_slot(id.slot(), value)
    }
    #[inline]
    pub(crate) fn set_members_slot(
        self,
        slot: u32,
        value: Option<SymbolTableId>,
    ) -> Result<(), Error> {
        let arena = self.store.rows.id();
        let row = self.store.rows.get_slot_mut(slot)?;
        let word = self.store.references.encode(
            arena,
            slot,
            Field::Members,
            value.map(|id| (id.arena(), id.slot(), Escape::Table(id))),
        );
        row.members = word;
        Ok(())
    }
    pub fn set_exports(self, id: SymbolId, value: Option<SymbolTableId>) -> Result<(), Error> {
        if id.arena() != self.store.rows.id() {
            return Err(Error::WrongOwner);
        }
        self.set_exports_slot(id.slot(), value)
    }
    #[inline]
    pub(crate) fn set_exports_slot(
        self,
        slot: u32,
        value: Option<SymbolTableId>,
    ) -> Result<(), Error> {
        let arena = self.store.rows.id();
        let row = self.store.rows.get_slot_mut(slot)?;
        let word = self.store.references.encode(
            arena,
            slot,
            Field::Exports,
            value.map(|id| (id.arena(), id.slot(), Escape::Table(id))),
        );
        row.exports = word;
        Ok(())
    }
    pub fn set_parent(self, id: SymbolId, value: Option<SymbolId>) -> Result<(), Error> {
        if id.arena() != self.store.rows.id() {
            return Err(Error::WrongOwner);
        }
        self.set_parent_slot(id.slot(), value)
    }
    #[inline]
    pub(crate) fn set_parent_slot(self, slot: u32, value: Option<SymbolId>) -> Result<(), Error> {
        let arena = self.store.rows.id();
        let row = self.store.rows.get_slot_mut(slot)?;
        let word = self.store.references.encode(
            arena,
            slot,
            Field::Parent,
            value.map(|id| (id.arena(), id.slot(), Escape::Symbol(id))),
        );
        row.parent = word;
        Ok(())
    }
    pub fn set_export_symbol(self, id: SymbolId, value: Option<SymbolId>) -> Result<(), Error> {
        if id.arena() != self.store.rows.id() {
            return Err(Error::WrongOwner);
        }
        self.set_export_symbol_slot(id.slot(), value)
    }
    #[inline]
    pub(crate) fn set_export_symbol_slot(
        self,
        slot: u32,
        value: Option<SymbolId>,
    ) -> Result<(), Error> {
        let arena = self.store.rows.id();
        let row = self.store.rows.get_slot_mut(slot)?;
        let word = self.store.references.encode(
            arena,
            slot,
            Field::ExportSymbol,
            value.map(|id| (id.arena(), id.slot(), Escape::Symbol(id))),
        );
        row.export_symbol = word;
        Ok(())
    }
}

/// Cold unrestricted edits stage an owned value. Binder writes use narrow APIs.
pub struct SymbolMut<'a> {
    value: Symbol,
    id: SymbolId,
    store: &'a mut Symbols,
    tables: &'a mut SymbolTables,
}
impl std::ops::Deref for SymbolMut<'_> {
    type Target = Symbol;
    fn deref(&self) -> &Symbol {
        &self.value
    }
}
impl std::ops::DerefMut for SymbolMut<'_> {
    fn deref_mut(&mut self) -> &mut Symbol {
        &mut self.value
    }
}
impl Drop for SymbolMut<'_> {
    fn drop(&mut self) {
        self.store
            .replace(self.id, std::mem::take(&mut self.value), self.tables);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{existing_runtime_symbol_id, runtime_symbol_id, DeclarationLists, SymbolTable};
    use std::sync::Barrier;
    use tsr_arena::OwnedArena;

    #[test]
    fn compact_links_roundtrip_full_slots_foreign_owners_and_nil_overwrites() {
        assert_eq!(std::mem::size_of::<StoredSymbol>(), 56);
        let counters = Counters::new();
        let nodes = OwnedArena::<()>::new(&counters);
        let declarations = DeclarationLists::new(&counters);
        let foreign = OwnedArena::<()>::new(&counters);
        let mut tables = SymbolTables::new(&counters);
        let mut symbols = Symbols::new(&counters);
        symbols.initialize_reference_arenas(nodes.id(), tables.id(), declarations.id());
        let id = symbols.write(&mut tables).push(Symbol::default());
        for (ast, backing, table, symbol, full) in [
            (
                nodes.id(),
                declarations.id(),
                tables.id(),
                symbols.id(),
                true,
            ),
            (
                foreign.id(),
                foreign.id(),
                foreign.id(),
                foreign.id(),
                false,
            ),
            (
                nodes.id(),
                declarations.id(),
                tables.id(),
                symbols.id(),
                false,
            ),
            (
                foreign.id(),
                foreign.id(),
                foreign.id(),
                foreign.id(),
                false,
            ),
        ] {
            let slot = |ordinary| if full { u32::MAX } else { ordinary };
            let declaration = DeclarationSlice::from_storage_parts(
                Some(AuxId::from_parts(backing, slot(11)).unwrap()),
                7,
                3,
                19,
            );
            let value = Some(NodeId::from_parts(ast, slot(13)).unwrap());
            let members = Some(SymbolTableId::from_parts(table, slot(17)).unwrap());
            let exports = Some(SymbolTableId::from_parts(table, slot(23)).unwrap());
            let parent = Some(SymbolId::from_parts(symbol, slot(29)).unwrap());
            let export = Some(SymbolId::from_parts(symbol, slot(31)).unwrap());
            symbols
                .write(&mut tables)
                .set_declarations(id, declaration)
                .unwrap();
            symbols
                .write(&mut tables)
                .set_value_declaration(id, value)
                .unwrap();
            symbols.write(&mut tables).set_members(id, members).unwrap();
            symbols.write(&mut tables).set_exports(id, exports).unwrap();
            symbols.write(&mut tables).set_parent(id, parent).unwrap();
            symbols
                .write(&mut tables)
                .set_export_symbol(id, export)
                .unwrap();
            symbols
                .write(&mut tables)
                .set_flags(id, 0x8123_4567)
                .unwrap();
            *symbols.write(&mut tables).flags_mut(id).unwrap() |= 0x4000_0000;
            symbols
                .write(&mut tables)
                .set_check_flags(id, 0x9234_5678)
                .unwrap();
            *symbols.write(&mut tables).check_flags_mut(id).unwrap() ^= 0x4000_0000;
            let read = symbols.read(&tables).get(id).unwrap();
            assert_eq!(read.flags(), 0xc123_4567);
            assert_eq!(read.check_flags(), 0xd234_5678);
            assert_eq!(
                (
                    read.declarations(),
                    read.value_declaration(),
                    read.members(),
                    read.exports(),
                    read.parent(),
                    read.export_symbol()
                ),
                (declaration, value, members, exports, parent, export),
            );
            assert_eq!(
                symbols.references.escapes.len(),
                if full || ast == foreign.id() { 6 } else { 0 }
            );
        }
        symbols
            .write(&mut tables)
            .set_declarations(id, DeclarationSlice::empty())
            .unwrap();
        symbols
            .write(&mut tables)
            .set_value_declaration(id, None)
            .unwrap();
        symbols.write(&mut tables).set_members(id, None).unwrap();
        symbols.write(&mut tables).set_exports(id, None).unwrap();
        symbols.write(&mut tables).set_parent(id, None).unwrap();
        symbols
            .write(&mut tables)
            .set_export_symbol(id, None)
            .unwrap();
        let read = symbols.read(&tables).get(id).unwrap();
        assert_eq!(read.declarations(), DeclarationSlice::empty());
        assert_eq!(read.value_declaration(), None);
        assert_eq!(read.members(), None);
        assert_eq!(read.exports(), None);
        assert_eq!(read.parent(), None);
        assert_eq!(read.export_symbol(), None);
        assert!(symbols.references.escapes.is_empty());
        assert_eq!(symbols.read(&tables).len(), 1);
    }

    #[test]
    fn invalid_receivers_fail_before_reference_inference_or_escape_mutation() {
        let counters = Counters::new();
        let foreign = OwnedArena::<()>::new(&counters);
        let mut tables = SymbolTables::new(&counters);
        let mut symbols = Symbols::new(&counters);
        let id = symbols.write(&mut tables).push(Symbol::default());
        let declaration = DeclarationSlice::from_storage_parts(
            Some(AuxId::from_parts(foreign.id(), u32::MAX).unwrap()),
            5,
            1,
            7,
        );
        let node = Some(NodeId::from_parts(foreign.id(), u32::MAX).unwrap());
        let table = Some(SymbolTableId::from_parts(foreign.id(), u32::MAX).unwrap());
        let symbol = Some(SymbolId::from_parts(foreign.id(), u32::MAX).unwrap());
        for (receiver, error) in [
            (
                SymbolId::from_parts(foreign.id(), u32::MAX).unwrap(),
                Error::WrongOwner,
            ),
            (
                SymbolId::from_parts(symbols.id(), u32::MAX).unwrap(),
                Error::InvalidSlot,
            ),
        ] {
            assert_eq!(
                symbols
                    .write(&mut tables)
                    .set_declarations(receiver, declaration),
                Err(error)
            );
            assert_eq!(
                symbols
                    .write(&mut tables)
                    .set_value_declaration(receiver, node),
                Err(error)
            );
            assert_eq!(
                symbols.write(&mut tables).set_members(receiver, table),
                Err(error)
            );
            assert_eq!(
                symbols.write(&mut tables).set_exports(receiver, table),
                Err(error)
            );
            assert_eq!(
                symbols.write(&mut tables).set_parent(receiver, symbol),
                Err(error)
            );
            assert_eq!(
                symbols
                    .write(&mut tables)
                    .set_export_symbol(receiver, symbol),
                Err(error)
            );
            assert_eq!(
                symbols.write(&mut tables).flags_mut(receiver).unwrap_err(),
                error
            );
            assert_eq!(
                symbols
                    .write(&mut tables)
                    .check_flags_mut(receiver)
                    .unwrap_err(),
                error
            );
            assert_eq!(
                symbols.write(&mut tables).get_mut(receiver).err(),
                Some(error)
            );
            assert_eq!(symbols.references.nodes, None);
            assert_eq!(symbols.references.tables, None);
            assert_eq!(symbols.references.declarations, None);
            assert!(symbols.references.escapes.is_empty());
        }
        let read = symbols.read(&tables).get(id).unwrap();
        assert_eq!(read.flags(), 0);
        assert_eq!(read.check_flags(), 0);
        assert!(read.declarations().is_nil());
        assert_eq!(read.value_declaration(), None);
        assert_eq!(read.members(), None);
        assert_eq!(read.exports(), None);
        assert_eq!(read.parent(), None);
        assert_eq!(read.export_symbol(), None);
    }

    #[test]
    fn stored_declaration_headers_keep_backing_aliases_nil_and_capacity() {
        let counters = Counters::new();
        let nodes = OwnedArena::<()>::new(&counters);
        let node = NodeId::from_parts(nodes.id(), 1).unwrap();
        let mut declarations = DeclarationLists::new(&counters);
        let mut tables = SymbolTables::new(&counters);
        let mut symbols = Symbols::new(&counters);
        symbols.initialize_reference_arenas(nodes.id(), tables.id(), declarations.id());
        let backing = declarations
            .alloc_with_capacity(vec![Some(node), None], 5)
            .unwrap();
        let header = backing.slice_with_capacity(1..2, 4).unwrap();
        let id = symbols.write(&mut tables).push(Symbol {
            declarations: header,
            ..Symbol::default()
        });
        let copied = symbols.read(&tables).get(id).unwrap().declarations();
        assert_eq!(copied, header);
        assert_eq!((copied.start(), copied.len(), copied.capacity()), (1, 1, 3));
        declarations.append(copied, Some(node)).unwrap();
        assert_eq!(
            declarations
                .get(header.slice(0..2).unwrap())
                .unwrap()
                .to_vec(),
            [None, Some(node)]
        );
        for empty in [
            declarations.alloc(Vec::new()).unwrap(),
            DeclarationSlice::empty(),
        ] {
            symbols
                .write(&mut tables)
                .set_declarations(id, empty)
                .unwrap();
            let stored = symbols.read(&tables).get(id).unwrap().declarations();
            assert_eq!(stored, empty);
            assert_eq!(stored.is_nil(), empty.is_nil());
        }
        let wide =
            DeclarationSlice::from_storage_parts(backing.backing_id(), 0, u32::MAX, u32::MAX);
        symbols
            .write(&mut tables)
            .set_declarations(id, wide)
            .unwrap();
        assert_eq!(symbols.read(&tables).get(id).unwrap().declarations(), wide);
        assert!(matches!(declarations.get(wide), Err(Error::InvalidSlot)));
    }

    #[test]
    fn shared_names_and_runtime_identity_survive_cold_edits_and_reset_on_replacement() {
        let counters = Counters::new();
        let mut tables = SymbolTables::new(&counters);
        let mut symbols = Symbols::new(&counters);
        let raw: &[u8] = b"\xfe\xffsame";
        let id = symbols
            .write(&mut tables)
            .push(Symbol::new(1, JsString::from_bytes(raw)));
        let second = symbols
            .write(&mut tables)
            .push(Symbol::new(2, JsString::from_bytes(raw)));
        let table = tables.alloc(SymbolTable::from_iter([(
            JsString::from_bytes(raw),
            Some(id),
        )]));
        let read = symbols.read(&tables).get(id).unwrap();
        assert_eq!(
            read.name_id(),
            symbols.read(&tables).get(second).unwrap().name_id()
        );
        assert_eq!(tables.get(table).unwrap().get(raw), Some(Some(id)));
        assert_eq!(
            read.name_bytes().as_ptr(),
            tables.get(table).unwrap().keys().next().unwrap().as_ptr()
        );
        assert_eq!(existing_runtime_symbol_id(&read), 0);
        let barrier = Barrier::new(8);
        let assigned = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        runtime_symbol_id(&read)
                    })
                })
                .collect();
            let values: Vec<_> = handles
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect();
            assert!(values[0] != 0 && values.iter().all(|value| *value == values[0]));
            values[0]
        });
        assert_eq!(
            existing_runtime_symbol_id(&symbols.read(&tables).get(second).unwrap()),
            0
        );
        let selected = read.name_to_owned();
        {
            let mut edit = symbols.write(&mut tables).get_mut(id).unwrap();
            edit.flags |= 4;
            edit.name = JsString::from_bytes(b"renamed".as_slice());
        }
        let read = symbols.read(&tables).get(id).unwrap();
        assert_eq!(read.flags(), 5);
        assert_eq!(read.name_bytes(), b"renamed");
        assert_eq!(existing_runtime_symbol_id(&read), assigned);
        *symbols.write(&mut tables).get_mut(id).unwrap() =
            Symbol::new(8, JsString::from_bytes(b"fresh".as_slice()));
        let fresh = symbols.read(&tables).get(id).unwrap();
        assert_eq!(existing_runtime_symbol_id(&fresh), 0);
        assert_ne!(runtime_symbol_id(&fresh), assigned);
        drop(symbols);
        drop(tables);
        assert_eq!(selected.as_bytes(), raw);
    }
}
