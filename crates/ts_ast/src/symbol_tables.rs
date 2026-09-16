//! Result-owned canonical byte names and nullable symbol-table entries.
//!
//! A table key is an owner-local canonical name, never a source-text offset.
//! Lookup hashes and compares the selected bytes. Full identities remain cold
//! table values when their namespace cannot use the owner's compact encoding.

// The outer Option distinguishes absence from a present null symbol value.
#![allow(clippy::option_option)]

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::ops::Range;

use hashbrown::HashTable;
use ts_arena::{ArenaId, AuxId, Counters, Error, OwnedArena, SymbolId};
use ts_jsstring::JsString;

/// Owned construction input. Stored tables expose borrowed byte keys instead.
pub type SymbolTable = HashMap<JsString, Option<SymbolId>>;
pub(crate) type NameId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SymbolTableId(AuxId);
impl SymbolTableId {
    pub(crate) fn from_parts(arena: ArenaId, slot: u32) -> Result<Self, Error> {
        AuxId::from_parts(arena, slot).map(Self)
    }
    pub(crate) fn slot(self) -> u32 {
        self.0.slot()
    }
    pub fn bits(self) -> u64 {
        self.0.bits()
    }
    pub fn arena(self) -> ArenaId {
        self.0.arena()
    }
}

#[derive(Clone, Copy, Debug)]
struct NameRange {
    start: u32,
    len: u32,
}
impl NameRange {
    const ESCAPE: Self = Self {
        start: u32::MAX,
        len: u32::MAX,
    };
    fn compact(range: &Range<usize>) -> Option<Self> {
        let start = u32::try_from(range.start).ok()?;
        let len = u32::try_from(range.end.checked_sub(range.start)?).ok()?;
        (start != u32::MAX || len != u32::MAX).then_some(Self { start, len })
    }
    fn is_escape(self) -> bool {
        self.start == u32::MAX && self.len == u32::MAX
    }
}

struct NamePool {
    bytes: Vec<u8>,
    ranges: Vec<NameRange>,
    // Keep the ordinary owner's root small; full-width ranges are exceptional.
    #[allow(clippy::box_collection)]
    wide_ranges: Option<Box<HashMap<NameId, Range<usize>>>>,
    names: HashTable<NameId>,
    hash_builder: RandomState,
}
impl std::fmt::Debug for NamePool {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("NamePool")
            .field("names", &self.ranges.len())
            .field("bytes", &self.bytes.len())
            .field(
                "wide_ranges",
                &self.wide_ranges.as_ref().map_or(0, |ranges| ranges.len()),
            )
            .finish_non_exhaustive()
    }
}
impl NamePool {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            ranges: Vec::new(),
            wide_ranges: None,
            names: HashTable::new(),
            hash_builder: RandomState::new(),
        }
    }
    fn hash(&self, bytes: &[u8]) -> u64 {
        self.hash_builder.hash_one(bytes)
    }
    fn range(&self, id: NameId) -> Range<usize> {
        let range = self.ranges[id - 1];
        if range.is_escape() {
            self.wide_ranges
                .as_ref()
                .and_then(|ranges| ranges.get(&id))
                .expect("wide canonical name range is retained")
                .clone()
        } else {
            let start = range.start as usize;
            start..start + range.len as usize
        }
    }
    fn bytes(&self, id: NameId) -> &[u8] {
        if id == 0 {
            &[]
        } else {
            &self.bytes[self.range(id)]
        }
    }
    fn append_range(&mut self, range: Range<usize>) -> NameId {
        let id = self
            .ranges
            .len()
            .checked_add(1)
            .expect("name index exhausted");
        let compact = NameRange::compact(&range).unwrap_or_else(|| {
            self.wide_ranges
                .get_or_insert_with(Default::default)
                .insert(id, range);
            NameRange::ESCAPE
        });
        self.ranges.push(compact);
        id
    }
    fn intern(&mut self, bytes: &[u8]) -> NameId {
        self.intern_hashed(bytes, self.hash(bytes))
    }
    fn intern_hashed(&mut self, bytes: &[u8], hash: u64) -> NameId {
        if bytes.is_empty() {
            return 0;
        }
        if let Some(&id) = self.names.find(hash, |&id| self.bytes(id) == bytes) {
            return id;
        }
        let start = self.bytes.len();
        self.bytes.extend_from_slice(bytes);
        let id = self.append_range(start..self.bytes.len());
        let Self {
            bytes,
            ranges,
            wide_ranges,
            names,
            hash_builder,
        } = self;
        names.insert_unique(hash, id, |&name| {
            let range = ranges[name - 1];
            let range = if range.is_escape() {
                wide_ranges.as_ref().expect("wide ranges")[&name].clone()
            } else {
                let start = range.start as usize;
                start..start + range.len as usize
            };
            hash_builder.hash_one(&bytes[range])
        });
        id
    }
}

#[derive(Clone, Copy, Debug)]
struct CompactEntry {
    name: u32,
    symbol: u32,
}
#[derive(Clone, Copy, Debug)]
struct FullEntry {
    name: NameId,
    symbol: Option<SymbolId>,
}
#[derive(Debug)]
enum TableRecord {
    Compact(HashTable<CompactEntry>),
    Full(HashTable<FullEntry>),
}
fn decode_symbol(slot: u32, symbols: Option<ArenaId>) -> Option<SymbolId> {
    (slot != 0).then(|| {
        SymbolId::from_parts(symbols.expect("compact symbol namespace is assigned"), slot)
            .expect("nonzero stored symbol slot")
    })
}
impl TableRecord {
    fn len(&self) -> usize {
        match self {
            Self::Compact(table) => table.len(),
            Self::Full(table) => table.len(),
        }
    }
    fn find(
        &self,
        hash: u64,
        symbols: Option<ArenaId>,
        mut equal_name: impl FnMut(NameId) -> bool,
    ) -> Option<Option<SymbolId>> {
        match self {
            Self::Compact(table) => table
                .find(hash, |entry| equal_name(entry.name as usize))
                .map(|entry| decode_symbol(entry.symbol, symbols)),
            Self::Full(table) => table
                .find(hash, |entry| equal_name(entry.name))
                .map(|entry| entry.symbol),
        }
    }
    fn make_full(&mut self, symbols: Option<ArenaId>, hash_name: &impl Fn(NameId) -> u64) {
        let Self::Compact(compact) = self else {
            return;
        };
        let mut full = HashTable::with_capacity(compact.len());
        for entry in compact.iter() {
            let entry = FullEntry {
                name: entry.name as usize,
                symbol: decode_symbol(entry.symbol, symbols),
            };
            full.insert_unique(hash_name(entry.name), entry, |entry| hash_name(entry.name));
        }
        *self = Self::Full(full);
    }
    fn insert(
        &mut self,
        name: NameId,
        symbol: Option<SymbolId>,
        symbols: Option<ArenaId>,
        hash: u64,
        hash_name: impl Fn(NameId) -> u64,
    ) -> Option<Option<SymbolId>> {
        let name_word = u32::try_from(name).ok();
        let symbol_word = match symbol {
            None => Some(0),
            Some(id) if Some(id.arena()) == symbols => Some(id.slot()),
            Some(_) => None,
        };
        if name_word.is_none() || symbol_word.is_none() {
            self.make_full(symbols, &hash_name);
        }
        match self {
            Self::Compact(table) => {
                let name = name_word.expect("compact name checked");
                let symbol = symbol_word.expect("compact symbol checked");
                if let Some(entry) = table.find_mut(hash, |entry| entry.name == name) {
                    return Some(decode_symbol(
                        std::mem::replace(&mut entry.symbol, symbol),
                        symbols,
                    ));
                }
                table.insert_unique(hash, CompactEntry { name, symbol }, |entry| {
                    hash_name(entry.name as usize)
                });
                None
            }
            Self::Full(table) => {
                if let Some(entry) = table.find_mut(hash, |entry| entry.name == name) {
                    return Some(std::mem::replace(&mut entry.symbol, symbol));
                }
                table.insert_unique(hash, FullEntry { name, symbol }, |entry| {
                    hash_name(entry.name)
                });
                None
            }
        }
    }
    fn remove(
        &mut self,
        hash: u64,
        symbols: Option<ArenaId>,
        mut equal_name: impl FnMut(NameId) -> bool,
    ) -> Option<Option<SymbolId>> {
        match self {
            Self::Compact(table) => table
                .find_entry(hash, |entry| equal_name(entry.name as usize))
                .ok()
                .map(|entry| decode_symbol(entry.remove().0.symbol, symbols)),
            Self::Full(table) => table
                .find_entry(hash, |entry| equal_name(entry.name))
                .ok()
                .map(|entry| entry.remove().0.symbol),
        }
    }
}

#[derive(Debug)]
pub struct SymbolTables {
    tables: OwnedArena<TableRecord>,
    names: NamePool,
    symbols: Option<ArenaId>,
}
impl SymbolTables {
    pub fn new(counters: &Counters) -> Self {
        Self {
            tables: OwnedArena::new(counters),
            names: NamePool::new(),
            symbols: None,
        }
    }
    pub fn id(&self) -> ArenaId {
        self.tables.id()
    }
    /// Configure once before inserting symbol values. Unconfigured standalone
    /// tables instead learn the namespace of their first non-null value.
    pub(crate) fn configure_symbols(&mut self, symbols: ArenaId) {
        assert!(
            self.symbols.is_none_or(|old| old == symbols),
            "symbol table namespace cannot change"
        );
        self.symbols = Some(symbols);
    }
    // Consuming the temporary releases its backing after the selected bytes
    // have been canonicalized; the pool retains no per-name JsString handle.
    #[allow(clippy::needless_pass_by_value)]
    pub(crate) fn intern_name(&mut self, name: JsString) -> NameId {
        self.names.intern(name.as_bytes())
    }
    pub(crate) fn name_bytes(&self, name: NameId) -> &[u8] {
        self.names.bytes(name)
    }
    /// Explicit owning conversion; the caller receives selected bytes that
    /// remain live independently of this mutable/published pool.
    pub(crate) fn name_to_owned(&self, name: NameId) -> JsString {
        JsString::from_bytes(self.name_bytes(name))
    }
    pub fn alloc(&mut self, input: SymbolTable) -> SymbolTableId {
        let id = SymbolTableId(
            self.tables
                .push(TableRecord::Compact(HashTable::with_capacity(input.len()))),
        );
        let mut table = self.get_mut(id).expect("new symbol table belongs to owner");
        for (name, symbol) in input {
            table.insert(name, symbol);
        }
        id
    }
    pub fn get(&self, id: SymbolTableId) -> Result<SymbolTableRead<'_>, Error> {
        Ok(SymbolTableRead {
            table: self.tables.get(id.0)?,
            names: &self.names,
            symbols: self.symbols,
        })
    }
    pub fn get_mut(&mut self, id: SymbolTableId) -> Result<SymbolTableMut<'_>, Error> {
        Ok(SymbolTableMut {
            table: self.tables.get_mut(id.0)?,
            names: &mut self.names,
            symbols: &mut self.symbols,
        })
    }
    #[inline]
    pub(crate) fn get_slot(&self, slot: u32) -> Result<SymbolTableRead<'_>, Error> {
        Ok(SymbolTableRead {
            table: self.tables.get_slot(slot)?,
            names: &self.names,
            symbols: self.symbols,
        })
    }
    #[inline]
    pub(crate) fn get_slot_mut(&mut self, slot: u32) -> Result<SymbolTableMut<'_>, Error> {
        Ok(SymbolTableMut {
            table: self.tables.get_slot_mut(slot)?,
            names: &mut self.names,
            symbols: &mut self.symbols,
        })
    }
    /// Bytes held or reserved by every table, the table directory and the
    /// canonical name pool, for storage censuses.
    pub fn structural_bytes(&self) -> usize {
        let tables: usize = self
            .tables
            .iter()
            .map(|(_, record)| match record {
                TableRecord::Compact(table) => table.allocation_size(),
                TableRecord::Full(table) => table.allocation_size(),
            })
            .sum();
        let names = self.names.bytes.capacity()
            + self.names.ranges.capacity() * size_of::<NameRange>()
            + self.names.names.allocation_size()
            + self.names.wide_ranges.as_ref().map_or(0, |ranges| {
                size_of::<HashMap<NameId, Range<usize>>>()
                    + ranges.capacity() * (size_of::<NameId>() + size_of::<Range<usize>>())
            });
        self.tables.structural_bytes() + tables + names
    }
    pub fn table_count(&self) -> usize {
        self.tables.len()
    }
    /// Bytes reserved by one table's own hash storage, for censuses that
    /// attribute a member table to the record owning it. `None` for a table of
    /// another store.
    pub fn table_structural_bytes(&self, id: SymbolTableId) -> Option<usize> {
        if id.arena() != self.tables.id() {
            return None;
        }
        self.tables.get(id.0).ok().map(|record| match record {
            TableRecord::Compact(table) => table.allocation_size(),
            TableRecord::Full(table) => table.allocation_size(),
        })
    }
    pub fn iter(&self) -> impl Iterator<Item = (SymbolTableId, SymbolTableRead<'_>)> {
        self.tables.iter().map(|(id, table)| {
            (
                SymbolTableId(id),
                SymbolTableRead {
                    table,
                    names: &self.names,
                    symbols: self.symbols,
                },
            )
        })
    }
    pub fn get_or_create(
        &mut self,
        id: &mut Option<SymbolTableId>,
    ) -> Result<SymbolTableMut<'_>, Error> {
        let id = *id.get_or_insert_with(|| self.alloc(SymbolTable::new()));
        self.get_mut(id)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SymbolTableRead<'a> {
    table: &'a TableRecord,
    names: &'a NamePool,
    symbols: Option<ArenaId>,
}
impl<'a> SymbolTableRead<'a> {
    pub fn len(self) -> usize {
        self.table.len()
    }
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
    /// Absent entries and present entries containing a null symbol differ.
    #[allow(clippy::option_option)]
    pub fn get(self, bytes: &[u8]) -> Option<Option<SymbolId>> {
        self.table
            .find(self.names.hash(bytes), self.symbols, |name| {
                self.names.bytes(name) == bytes
            })
    }
    pub fn contains_key(self, bytes: &[u8]) -> bool {
        self.get(bytes).is_some()
    }
    pub fn iter(self) -> SymbolTableIter<'a> {
        let entries = match self.table {
            TableRecord::Compact(table) => Entries::Compact(table.iter()),
            TableRecord::Full(table) => Entries::Full(table.iter()),
        };
        SymbolTableIter {
            entries,
            names: self.names,
            symbols: self.symbols,
        }
    }
    pub fn keys(self) -> impl ExactSizeIterator<Item = &'a [u8]> {
        self.iter().map(|(name, _)| name)
    }
}
impl<'a> IntoIterator for SymbolTableRead<'a> {
    type Item = (&'a [u8], Option<SymbolId>);
    type IntoIter = SymbolTableIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Debug)]
pub struct SymbolTableMut<'a> {
    table: &'a mut TableRecord,
    names: &'a mut NamePool,
    symbols: &'a mut Option<ArenaId>,
}
impl SymbolTableMut<'_> {
    pub fn read(&self) -> SymbolTableRead<'_> {
        SymbolTableRead {
            table: self.table,
            names: self.names,
            symbols: *self.symbols,
        }
    }
    pub fn len(&self) -> usize {
        self.table.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    #[allow(clippy::option_option)]
    pub fn get(&self, bytes: &[u8]) -> Option<Option<SymbolId>> {
        self.read().get(bytes)
    }
    pub fn contains_key(&self, bytes: &[u8]) -> bool {
        self.get(bytes).is_some()
    }
    pub fn iter(&self) -> SymbolTableIter<'_> {
        self.read().iter()
    }
    pub fn keys(&self) -> impl ExactSizeIterator<Item = &[u8]> {
        self.iter().map(|(name, _)| name)
    }
    /// Insertion stores the exact input identity without target validation.
    #[allow(clippy::option_option, clippy::needless_pass_by_value)]
    pub fn insert(&mut self, name: JsString, symbol: Option<SymbolId>) -> Option<Option<SymbolId>> {
        let hash = self.names.hash(name.as_bytes());
        let name = self.names.intern_hashed(name.as_bytes(), hash);
        if let Some(symbol) = symbol {
            self.symbols.get_or_insert(symbol.arena());
        }
        self.table
            .insert(name, symbol, *self.symbols, hash, |name| {
                self.names.hash(self.names.bytes(name))
            })
    }
    #[allow(clippy::option_option)]
    pub fn remove(&mut self, bytes: &[u8]) -> Option<Option<SymbolId>> {
        self.table
            .remove(self.names.hash(bytes), *self.symbols, |name| {
                self.names.bytes(name) == bytes
            })
    }
}

enum Entries<'a> {
    Compact(hashbrown::hash_table::Iter<'a, CompactEntry>),
    Full(hashbrown::hash_table::Iter<'a, FullEntry>),
}
pub struct SymbolTableIter<'a> {
    entries: Entries<'a>,
    names: &'a NamePool,
    symbols: Option<ArenaId>,
}
impl<'a> Iterator for SymbolTableIter<'a> {
    type Item = (&'a [u8], Option<SymbolId>);
    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.entries {
            Entries::Compact(entries) => entries.next().map(|entry| {
                (
                    self.names.bytes(entry.name as usize),
                    decode_symbol(entry.symbol, self.symbols),
                )
            }),
            Entries::Full(entries) => entries
                .next()
                .map(|entry| (self.names.bytes(entry.name), entry.symbol)),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.entries {
            Entries::Compact(entries) => entries.size_hint(),
            Entries::Full(entries) => entries.size_hint(),
        }
    }
}
impl ExactSizeIterator for SymbolTableIter<'_> {}
impl std::iter::FusedIterator for SymbolTableIter<'_> {}

impl<'a> IntoIterator for &'a SymbolTableMut<'_> {
    type Item = (&'a [u8], Option<SymbolId>);
    type IntoIter = SymbolTableIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_arena::SymbolArena;

    fn js(bytes: &[u8]) -> JsString {
        JsString::from_bytes(bytes)
    }

    #[test]
    fn canonical_names_compare_selected_bytes_and_outlive_inputs() {
        let counters = Counters::new();
        let mut tables = SymbolTables::new(&counters);
        let raw = [0xff, 0xed, 0xa0, 0x80];
        let first = tables.intern_name(js(&raw));
        let framed = js(&[b'!', 0xff, 0xed, 0xa0, 0x80, b'?']);
        let selected = framed.slice(1..5).unwrap();
        assert_eq!(tables.intern_name(selected), first);
        drop(framed);
        assert_eq!(tables.name_bytes(first), raw);
        let generated = tables.intern_name(js(b"\xfeconstructor"));
        assert_ne!(generated, first);
        assert_eq!(tables.name_bytes(generated), b"\xfeconstructor");
        assert_eq!(tables.intern_name(js(b"")), 0);
        assert_eq!(tables.name_bytes(0), b"");
        assert_eq!(tables.names.ranges.len(), 2);
        assert_eq!(
            tables.names.bytes.len(),
            raw.len() + b"\xfeconstructor".len()
        );
        let owned = tables.name_to_owned(first);
        drop(tables);
        assert_eq!(owned.as_bytes(), raw);

        // The private insertion seam permits deliberately colliding hashes;
        // reserve first so this case specifically tests equality, not rehashing.
        let mut colliding = NamePool::new();
        colliding.names = HashTable::with_capacity(4);
        let one = colliding.intern_hashed(b"one", 42);
        let two = colliding.intern_hashed(b"two", 42);
        assert_ne!(one, two);
        assert_eq!(colliding.intern_hashed(b"one", 42), one);
        assert_eq!(colliding.bytes(two), b"two");
    }

    #[test]
    fn tables_share_pool_without_sharing_membership_or_null_values() {
        let counters = Counters::new();
        let mut symbols = SymbolArena::new(&counters);
        let symbol = symbols.push(());
        let mut tables = SymbolTables::new(&counters);
        tables.configure_symbols(symbols.id());
        let name = tables.intern_name(js(b"same"));
        let one = tables.alloc(SymbolTable::from([(js(b"same"), None)]));
        let two = tables.alloc(SymbolTable::from([(js(b"same"), Some(symbol))]));
        assert_eq!(tables.names.ranges.len(), 1);
        assert_eq!(tables.name_bytes(name), b"same");
        assert_eq!(tables.get(one).unwrap().get(b"same"), Some(None));
        assert_eq!(tables.get(two).unwrap().get(b"same"), Some(Some(symbol)));
        assert_eq!(tables.get(one).unwrap().get(b"absent"), None);
        assert_eq!(
            tables.get(one).unwrap().keys().collect::<Vec<_>>(),
            [b"same".as_slice()]
        );
        assert_eq!(
            tables
                .get_mut(one)
                .unwrap()
                .insert(js(b"same"), Some(symbol)),
            Some(None)
        );
        assert_eq!(
            tables.get_mut(one).unwrap().remove(b"same"),
            Some(Some(symbol))
        );
        assert!(tables.get(one).unwrap().is_empty());
        assert_eq!(tables.get(two).unwrap().len(), 1);
        assert_eq!(tables.names.ranges.len(), 1);
    }

    #[test]
    fn namespace_learning_preserves_full_slots_and_only_promotes_foreign_table() {
        let counters = Counters::new();
        let symbols = SymbolArena::<()>::new(&counters);
        let foreign = SymbolArena::<()>::new(&counters);
        let full_slot = SymbolId::from_parts(symbols.id(), u32::MAX).unwrap();
        let foreign_id = SymbolId::from_parts(foreign.id(), 1).unwrap();
        let mut tables = SymbolTables::new(&counters);
        let first = tables.alloc(SymbolTable::from([(js(b"empty"), None)]));
        let untouched = tables.alloc(SymbolTable::new());
        assert!(tables.symbols.is_none());
        assert_eq!(tables.get(first).unwrap().get(b"empty"), Some(None));
        tables
            .get_mut(first)
            .unwrap()
            .insert(js(b"full"), Some(full_slot));
        assert_eq!(tables.symbols, Some(symbols.id()));
        assert!(matches!(
            tables.tables.get(first.0).unwrap(),
            TableRecord::Compact(_)
        ));
        assert_eq!(
            tables.get(first).unwrap().get(b"full"),
            Some(Some(full_slot))
        );
        // Neither identity names an allocated symbol; insertion still succeeds.
        tables
            .get_mut(first)
            .unwrap()
            .insert(js(b"foreign"), Some(foreign_id));
        assert!(matches!(
            tables.tables.get(first.0).unwrap(),
            TableRecord::Full(_)
        ));
        assert!(matches!(
            tables.tables.get(untouched.0).unwrap(),
            TableRecord::Compact(_)
        ));
        assert_eq!(tables.get(first).unwrap().get(b"empty"), Some(None));
        assert_eq!(
            tables.get(first).unwrap().get(b"full"),
            Some(Some(full_slot))
        );
        assert_eq!(
            tables.get(first).unwrap().get(b"foreign"),
            Some(Some(foreign_id))
        );
        assert_eq!(
            tables.get_mut(first).unwrap().insert(js(b"foreign"), None),
            Some(Some(foreign_id))
        );
        assert_eq!(
            tables.get_mut(first).unwrap().remove(b"foreign"),
            Some(None)
        );
        assert_eq!(tables.get_mut(first).unwrap().remove(b"foreign"), None);
        tables.configure_symbols(symbols.id());
    }

    #[test]
    fn wide_names_promote_without_narrowing_and_collisions_check_canonical_identity() {
        let counters = Counters::new();
        let symbols = SymbolArena::<()>::new(&counters);
        let id = SymbolId::from_parts(symbols.id(), u32::MAX).unwrap();
        let namespace = Some(symbols.id());
        let mut table = TableRecord::Compact(HashTable::new());
        let wide = usize::try_from(u64::from(u32::MAX) + 1).unwrap();
        let names = [0, 17, u32::MAX as usize, wide, usize::MAX];
        for (index, &name) in names.iter().enumerate() {
            // Force every table hash to collide, including growth rehashes.
            let value = (index % 2 == 0).then_some(id);
            assert_eq!(table.insert(name, value, namespace, 42, |_| 42), None);
        }
        assert!(matches!(table, TableRecord::Full(_)));
        for (index, &name) in names.iter().enumerate() {
            assert_eq!(
                table.find(42, namespace, |found| found == name),
                Some((index % 2 == 0).then_some(id))
            );
        }
        assert_eq!(table.find(42, namespace, |found| found == 19), None);
        assert_eq!(
            table.insert(wide, Some(id), namespace, 42, |_| 42),
            Some(None)
        );
        assert_eq!(
            table.remove(42, namespace, |name| name == usize::MAX),
            Some(Some(id))
        );
        assert_eq!(
            table.find(42, namespace, |name| name == wide),
            Some(Some(id))
        );
    }

    #[test]
    fn compact_and_full_growth_preserve_byte_keys_and_nullable_values() {
        let counters = Counters::new();
        let symbols = SymbolArena::<()>::new(&counters);
        let other = SymbolArena::<()>::new(&counters);
        let local = SymbolId::from_parts(symbols.id(), 3).unwrap();
        let foreign = SymbolId::from_parts(other.id(), 9).unwrap();
        let mut tables = SymbolTables::new(&counters);
        tables.configure_symbols(symbols.id());
        let id = tables.alloc(SymbolTable::new());
        let mut expected = SymbolTable::new();
        for index in 0u32..300 {
            let name = index.to_le_bytes();
            let value = if index % 3 == 0 { None } else { Some(local) };
            assert_eq!(
                tables.get_mut(id).unwrap().insert(js(&name), value),
                expected.insert(js(&name), value)
            );
        }
        assert!(matches!(
            tables.tables.get(id.0).unwrap(),
            TableRecord::Compact(_)
        ));
        tables
            .get_mut(id)
            .unwrap()
            .insert(js(b"foreign"), Some(foreign));
        expected.insert(js(b"foreign"), Some(foreign));
        for index in 300u32..600 {
            let name = index.to_le_bytes();
            let value = if index % 3 == 0 { None } else { Some(local) };
            assert_eq!(
                tables.get_mut(id).unwrap().insert(js(&name), value),
                expected.insert(js(&name), value)
            );
        }
        for (name, &value) in &expected {
            assert_eq!(tables.get(id).unwrap().get(name.as_bytes()), Some(value));
        }
        let observed: SymbolTable = tables
            .get(id)
            .unwrap()
            .iter()
            .map(|(key, value)| (js(key), value))
            .collect();
        assert_eq!(observed, expected);
        assert_eq!(tables.get(id).unwrap().iter().len(), expected.len());
    }

    #[test]
    fn wide_ranges_preserve_full_usize_endpoints_without_large_allocations() {
        let mut names = NamePool::new();
        let full = u32::MAX as usize;
        let ordinary = names.append_range(full..full + 1);
        assert_eq!(names.range(ordinary), full..full + 1);
        assert!(names.wide_ranges.is_none());
        let wide = names.append_range(full + 1..full + 4);
        assert_eq!(names.range(wide), full + 1..full + 4);
        let sentinel_pair = names.append_range(full..full * 2);
        assert_eq!(names.range(sentinel_pair), full..full * 2);
        let end = names.append_range(usize::MAX - 3..usize::MAX);
        assert_eq!(names.range(end), usize::MAX - 3..usize::MAX);
        assert_eq!(names.wide_ranges.as_ref().unwrap().len(), 3);
    }

    #[test]
    fn table_ids_and_get_or_create_preserve_owner_and_stale_rejection() {
        let counters = Counters::new();
        let before = counters.snapshot();
        let stale = {
            let mut tables = SymbolTables::new(&counters);
            let mut id = None;
            assert!(tables.get_or_create(&mut id).unwrap().is_empty());
            let first = id.unwrap();
            tables
                .get_or_create(&mut id)
                .unwrap()
                .insert(js(b"a"), None);
            assert_eq!(id, Some(first));
            assert_eq!(tables.iter().count(), 1);
            let future = SymbolTableId::from_parts(tables.id(), first.slot() + 1).unwrap();
            assert!(matches!(tables.get(future), Err(Error::InvalidSlot)));
            assert!(matches!(tables.get_mut(future), Err(Error::InvalidSlot)));
            first
        };
        assert_eq!(counters.snapshot(), before);
        let mut replacement = SymbolTables::new(&counters);
        replacement.alloc(SymbolTable::new());
        assert!(matches!(replacement.get(stale), Err(Error::WrongOwner)));
        assert!(matches!(replacement.get_mut(stale), Err(Error::WrongOwner)));
        assert_eq!(std::mem::size_of::<CompactEntry>(), 8);
        assert_eq!(std::mem::size_of::<NameRange>(), 8);
    }
}
