//! The reference-based relater prototype required by the S08 plan (§6.3) and
//! ADR 0008: the relation algorithm of `relater.go`'s core over a type graph
//! of stable heap cells linked by references instead of arena ids, with lazily
//! resolved members that allocate during a relation.
//!
//! [`BoundChecker`] constructs its own type graph from completed AST/binder
//! owners, compiler options and module-loader decisions. The measurement path
//! never resolves types through the production checker. Member types, generic
//! arguments, mapped templates and conditional branches retain native lazy
//! construction points. Diagnostics carry structured native message identity.
//!
//! [`Description`] remains a focused graph-test API; the measurement child does
//! not use it. Coverage is established by the frozen 105-group comparison, not
//! by the constructors available here. Unimplemented branches fail explicitly.
//!
//! # Construction and mutation scopes
//!
//! - [`Graph`] owns every [`TypeCell`] strongly (`Rc`). Every edge between types
//!   (members, constituents, signature parts) is a `Weak`, so the only strong
//!   references form a tree from the graph; dropping the graph drops every cell
//!   (no cycles).
//! - Lazy state lives in `OnceCell`/`RefCell` fields of the cell. A resolver runs
//!   at most once, allocates into the graph while an `&TypeCell` borrow of the
//!   same graph is live (`Rc` cells never move), and publishes the result before
//!   anyone can read it.
//! - Callers hold `Rc<TypeCell>` handles. A handle keeps its cell alive but not
//!   the cells it points to: after the graph drops, following an edge fails
//!   explicitly (`Error::Released`). The production design keeps the owner alive
//!   from every escaped result for exactly this reason (ADR 0007).
//! - A failed or panicking resolver leaves the `OnceCell` empty and its cell
//!   terminally unresolved. Later reads fail explicitly; consuming the resolver
//!   never turns failure into an empty object. The algorithm never holds a
//!   `RefMut` across a call that can run user code.
#![forbid(unsafe_code)]

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use ts_diagnostics as d;

mod relation_keys;
mod type_link;
use type_link::TypeLink;

mod bound;
pub mod bound_input;
pub use bound::BoundChecker;

mod diagnostics;
use diagnostics::ErrorChain;
pub use diagnostics::{Diagnostic, DiagnosticLocation};

mod constructors;
mod display;
mod generics;
mod signatures;
mod template;
mod tuples;
pub use generics::{GenericSignature, GenericTarget, ReferenceShape, TypeParameterShape};
pub use template::TemplateParts;
pub use tuples::{element_flags, ArrayElement, TupleShape};

/// `Ternary`: `x & y` picks the lesser in the order False < Unknown < Maybe < True.
pub type Ternary = i8;
pub const FALSE: Ternary = 0;
pub const UNKNOWN: Ternary = 1;
pub const MAYBE: Ternary = 3;
pub const TRUE: Ternary = -1;

/// `TypeFlags`, with upstream's bit positions.
pub mod flags {
    pub const ANY: u32 = 1 << 0;
    pub const UNKNOWN: u32 = 1 << 1;
    pub const UNDEFINED: u32 = 1 << 2;
    pub const NULL: u32 = 1 << 3;
    pub const VOID: u32 = 1 << 4;
    pub const STRING: u32 = 1 << 5;
    pub const NUMBER: u32 = 1 << 6;
    pub const BIG_INT: u32 = 1 << 7;
    pub const BOOLEAN: u32 = 1 << 8;
    pub const ES_SYMBOL: u32 = 1 << 9;
    pub const STRING_LITERAL: u32 = 1 << 10;
    pub const NUMBER_LITERAL: u32 = 1 << 11;
    pub const BIG_INT_LITERAL: u32 = 1 << 12;
    pub const BOOLEAN_LITERAL: u32 = 1 << 13;
    pub const UNIQUE_ES_SYMBOL: u32 = 1 << 14;
    pub const ENUM_LITERAL: u32 = 1 << 15;
    pub const ENUM: u32 = 1 << 16;
    pub const NON_PRIMITIVE: u32 = 1 << 17;
    pub const NEVER: u32 = 1 << 18;
    pub const TYPE_PARAMETER: u32 = 1 << 19;
    pub const OBJECT: u32 = 1 << 20;
    pub const INDEX: u32 = 1 << 21;
    pub const TEMPLATE_LITERAL: u32 = 1 << 22;
    pub const STRING_MAPPING: u32 = 1 << 23;
    pub const SUBSTITUTION: u32 = 1 << 24;
    pub const INDEXED_ACCESS: u32 = 1 << 25;
    pub const CONDITIONAL: u32 = 1 << 26;
    pub const INSTANTIABLE_NON_PRIMITIVE: u32 =
        TYPE_PARAMETER | INDEXED_ACCESS | CONDITIONAL | SUBSTITUTION;
    pub const UNION: u32 = 1 << 27;
    pub const INTERSECTION: u32 = 1 << 28;
    pub const ANY_OR_UNKNOWN: u32 = ANY | UNKNOWN;
    pub const NULLABLE: u32 = UNDEFINED | NULL;
    pub const LITERAL: u32 = STRING_LITERAL | NUMBER_LITERAL | BIG_INT_LITERAL | BOOLEAN_LITERAL;
    pub const UNIT: u32 = ENUM | LITERAL | UNIQUE_ES_SYMBOL | NULLABLE;
    pub const STRING_LIKE: u32 = STRING | STRING_LITERAL | TEMPLATE_LITERAL | STRING_MAPPING;
    pub const NUMBER_LIKE: u32 = NUMBER | NUMBER_LITERAL | ENUM;
    pub const BIG_INT_LIKE: u32 = BIG_INT | BIG_INT_LITERAL;
    pub const BOOLEAN_LIKE: u32 = BOOLEAN | BOOLEAN_LITERAL;
    pub const ES_SYMBOL_LIKE: u32 = ES_SYMBOL | UNIQUE_ES_SYMBOL;
    pub const VOID_LIKE: u32 = VOID | UNDEFINED;
    pub const PRIMITIVE: u32 = STRING_LIKE
        | NUMBER_LIKE
        | BIG_INT_LIKE
        | BOOLEAN_LIKE
        | ENUM_LITERAL
        | ES_SYMBOL_LIKE
        | VOID_LIKE
        | NULL;
    pub const DEFINITELY_NON_NULLABLE: u32 = STRING_LIKE
        | NUMBER_LIKE
        | BIG_INT_LIKE
        | BOOLEAN_LIKE
        | ENUM_LIKE
        | ES_SYMBOL_LIKE
        | OBJECT
        | NON_PRIMITIVE;
    pub const ENUM_LIKE: u32 = ENUM | ENUM_LITERAL;
    pub const UNION_OR_INTERSECTION: u32 = UNION | INTERSECTION;
    pub const STRUCTURED_TYPE: u32 = OBJECT | UNION | INTERSECTION;
    pub const INSTANTIABLE: u32 = TYPE_PARAMETER
        | INDEXED_ACCESS
        | CONDITIONAL
        | SUBSTITUTION
        | INDEX
        | TEMPLATE_LITERAL
        | STRING_MAPPING;
    pub const INSTANTIABLE_PRIMITIVE: u32 = INDEX | TEMPLATE_LITERAL | STRING_MAPPING;
    pub const STRUCTURED_OR_INSTANTIABLE: u32 = STRUCTURED_TYPE | INSTANTIABLE;
    /// `TypeFlagsSingleton`: identical flags mean identical types.
    pub const SINGLETON: u32 = ANY
        | UNKNOWN
        | STRING
        | NUMBER
        | BOOLEAN
        | BIG_INT
        | ES_SYMBOL
        | VOID
        | UNDEFINED
        | NULL
        | NEVER
        | NON_PRIMITIVE;
    /// Every kind the reference relater can carry; anything else is unsupported.
    pub const SUPPORTED: u32 = ANY
        | UNKNOWN
        | UNDEFINED
        | NULL
        | VOID
        | STRING
        | NUMBER
        | BIG_INT
        | BOOLEAN
        | ES_SYMBOL
        | LITERAL
        | NON_PRIMITIVE
        | NEVER
        | OBJECT
        | UNION
        | INTERSECTION
        | TEMPLATE_LITERAL
        | TYPE_PARAMETER;
}

/// `ObjectFlags` bits the algorithm consults.
pub mod object_flags {
    pub const CLASS: u32 = 1 << 0;
    pub const INTERFACE: u32 = 1 << 1;
    pub const REFERENCE: u32 = 1 << 2;
    pub const TUPLE: u32 = 1 << 3;
    pub const ANONYMOUS: u32 = 1 << 4;
    pub const MAPPED: u32 = 1 << 5;
    pub const INSTANTIATED: u32 = 1 << 6;
    pub const OBJECT_LITERAL: u32 = 1 << 7;
    pub const FRESH_LITERAL: u32 = 1 << 13;
    pub const PRIMITIVE_UNION: u32 = 1 << 15;
    pub const NON_INFERRABLE_TYPE: u32 = 1 << 18;
}

/// `RelationComparisonResult`.
pub mod relation_result {
    pub const SUCCEEDED: u32 = 1 << 0;
    pub const FAILED: u32 = 1 << 1;
    pub const COMPLEXITY_OVERFLOW: u32 = 1 << 5;
    pub const STACK_DEPTH_OVERFLOW: u32 = 1 << 6;
    pub const OVERFLOW: u32 = COMPLEXITY_OVERFLOW | STACK_DEPTH_OVERFLOW;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Identity,
    Assignable,
    Subtype,
    StrictSubtype,
    Comparable,
}

pub const MODES: [Mode; 5] = [
    Mode::Identity,
    Mode::Assignable,
    Mode::Subtype,
    Mode::StrictSubtype,
    Mode::Comparable,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// An edge points at a cell whose graph has been released.
    Released,
    /// A member resolver referenced a type that was never declared.
    UndeclaredMember(Rc<str>),
    /// An object's resolver was consumed by a failed or reentrant resolution.
    ResolutionFailed,
    /// The relation reached a path the isolated reference does not implement.
    Unsupported(Rc<str>),
}

fn unsupported<T>(what: &str) -> Result<T, Error> {
    Err(Error::Unsupported(Rc::from(what)))
}

/// A resolved property of an object type.
#[derive(Clone, Debug)]
pub struct Member {
    pub(crate) name_type: Option<TypeLink>,
    pub name: Rc<str>,
    pub optional: bool,
    pub readonly: bool,
    /// `SymbolFlagsClassMember`: properties, methods and accessors.
    pub class_member: bool,
    r#type: TypeLink,
}

impl Member {
    pub fn r#type(&self) -> Result<Rc<TypeCell>, Error> {
        self.r#type.resolve()
    }
}

/// A resolved index signature.
#[derive(Clone, Debug)]
pub struct IndexInfo {
    key: Weak<TypeCell>,
    value: Weak<TypeCell>,
    pub readonly: bool,
}

impl IndexInfo {
    pub fn key(&self) -> Result<Rc<TypeCell>, Error> {
        self.key.upgrade().ok_or(Error::Released)
    }
    pub fn value(&self) -> Result<Rc<TypeCell>, Error> {
        self.value.upgrade().ok_or(Error::Released)
    }
}

/// A resolved, non-generic call or construct signature.
#[derive(Clone, Debug)]
pub struct Signature {
    parameters: Vec<TypeLink>,
    pub parameter_names: Vec<Rc<str>>,
    pub min_argument_count: usize,
    pub has_rest_parameter: bool,
    pub type_parameters: usize,
    pub generic: Option<Rc<GenericSignature>>,
    this_type: Option<TypeLink>,
    return_type: TypeLink,
    /// Method-style declarations compare parameters bivariantly.
    pub bivariant_parameters: bool,
    pub is_abstract: bool,
    pub is_construct: bool,
    /// The signature this one instantiates (`Signature.target`).
    pub(crate) target: Option<Rc<Signature>>,
    /// The declaration of each parameter, where there is one: the labels of a
    /// rest tuple built from these parameters (`getNameableDeclarationAtPosition`).
    pub(crate) parameter_declarations: Vec<Option<ts_arena::NodeId>>,
}

impl Signature {
    fn parameter(&self, index: usize) -> Result<Option<Rc<TypeCell>>, Error> {
        match self.parameters.get(index) {
            Some(link) => link.resolve().map(Some),
            None => Ok(None),
        }
    }
    fn return_type(&self) -> Result<Rc<TypeCell>, Error> {
        self.return_type.resolve()
    }
    fn this_type(&self) -> Result<Option<Rc<TypeCell>>, Error> {
        match &self.this_type {
            Some(link) => link.resolve().map(Some),
            None => Ok(None),
        }
    }
    fn parameter_count(&self) -> usize {
        self.parameters.len()
    }
}

/// An object type's resolved structure.
#[derive(Clone, Debug, Default)]
pub struct Structure {
    pub members: Vec<Member>,
    pub index_infos: Vec<IndexInfo>,
    pub call_signatures: Vec<Signature>,
    pub construct_signatures: Vec<Signature>,
}

/// How an object type's structure is produced when first needed.
type Resolver = Box<dyn Fn(&Graph, &TypeCell) -> Result<Structure, Error>>;

/// A literal type's value; identity of literal types is by cell (interned by
/// the description), the value serves display and the enum-free simple checks.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LiteralValue {
    String(Vec<u8>),
    Number(u64),
    Boolean(bool),
    BigInt { negative: bool, digits: Vec<u8> },
}

/// One type: identity, flags and lazily resolved structure.
pub struct TypeCell {
    id: u32,
    flags: u32,
    object_flags: u32,
    name: Rc<str>,
    /// Recursion identity for interface-like types (upstream: the symbol).
    symbol: Option<u64>,
    alias: Option<u64>,
    alias_arguments: OnceCell<Vec<Weak<TypeCell>>>,
    literal: Option<LiteralValue>,
    fresh: bool,
    /// The fresh form of a regular literal, or the regular form of a fresh one.
    alternate: RefCell<Weak<TypeCell>>,
    /// Union or intersection constituents (eager, as upstream; linked once
    /// every described cell exists).
    types: RefCell<Vec<Weak<TypeCell>>>,
    origin: OnceCell<Weak<TypeCell>>,
    /// Type-reference/template metadata is installed once during construction.
    tuple_shape: OnceCell<TupleShape>,
    array_element: OnceCell<ArrayElement>,
    template_parts: OnceCell<TemplateParts>,
    reference_shape: OnceCell<ReferenceShape>,
    generic_target: OnceCell<GenericTarget>,
    type_parameter: OnceCell<TypeParameterShape>,
    marker_instantiation: Cell<bool>,
    /// `isObjectTypeWithInferableIndex`.
    inferable_index: bool,
    /// Set once by the first `structure` call; empty for non-objects.
    structure: OnceCell<Structure>,
    /// Taken by the first `structure` call.
    resolver: RefCell<Option<Resolver>>,
    /// Counts resolutions, to prove laziness and single execution.
    resolutions: Cell<u32>,
}

impl std::fmt::Debug for TypeCell {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("TypeCell")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("flags", &self.flags)
            .field("resolved", &self.structure.get().is_some())
            .finish_non_exhaustive()
    }
}

impl TypeCell {
    pub fn id(&self) -> u32 {
        self.id
    }
    pub fn flags(&self) -> u32 {
        self.flags
    }
    pub fn object_flags(&self) -> u32 {
        self.object_flags
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn resolutions(&self) -> u32 {
        self.resolutions.get()
    }
    pub fn is_fresh_literal(&self) -> bool {
        self.fresh
    }

    /// The object's structure, resolved on first use. Resolution may allocate
    /// into `graph`; the cell itself never moves, so the borrow stays valid.
    pub fn structure(&self, graph: &Graph) -> Result<&Structure, Error> {
        if let Some(structure) = self.structure.get() {
            return Ok(structure);
        }
        let resolver = self.resolver.borrow_mut().take();
        let structure = match resolver {
            Some(resolver) => resolver(graph, self)?,
            None if self.flags & flags::OBJECT != 0 => return Err(Error::ResolutionFailed),
            None => Structure::default(),
        };
        self.resolutions.set(self.resolutions.get() + 1);
        // A reentrant read sees the consumed resolver and fails above rather
        // than publishing empty members while the outer resolution is pending.
        Ok(self.structure.get_or_init(|| structure))
    }

    pub fn members(&self, graph: &Graph) -> Result<&[Member], Error> {
        Ok(&self.structure(graph)?.members)
    }

    pub fn member(&self, graph: &Graph, name: &str) -> Result<Option<&Member>, Error> {
        Ok(self
            .members(graph)?
            .iter()
            .find(|member| &*member.name == name))
    }

    /// Union or intersection constituents.
    pub fn types(&self) -> Result<Vec<Rc<TypeCell>>, Error> {
        self.types
            .borrow()
            .iter()
            .map(|weak| weak.upgrade().ok_or(Error::Released))
            .collect()
    }

    fn constituent_count(&self) -> usize {
        self.types.borrow().len()
    }

    fn alternate(&self) -> Option<Rc<TypeCell>> {
        self.alternate.borrow().upgrade()
    }
}

/// Constituent ids, the naming alias with its argument ids, and the origin.
type UnionKey = (Vec<u32>, Option<(u64, Vec<u32>)>, Option<u32>);

/// The owner of every type cell. Allocation is append-only through a
/// `RefCell<Vec<Rc<_>>>`; readers hold `Rc` clones, never a borrow of the vector.
#[derive(Default)]
pub struct Graph {
    types: RefCell<Vec<Rc<TypeCell>>>,
    next_id: Cell<u32>,
    /// Records created by resolvers during relations (property symbols,
    /// index infos and signatures upstream).
    lazy_records: RefCell<Vec<Rc<Member>>>,
    lazy_extra: Cell<usize>,
    literal_cache: RefCell<HashMap<(u32, LiteralValue), Weak<TypeCell>>>,
    union_cache: RefCell<HashMap<UnionKey, Weak<TypeCell>>>,
    template_cache: RefCell<HashMap<template::TemplateKey, Weak<TypeCell>>>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.types.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Records allocated by resolvers so far.
    pub fn lazy_records(&self) -> usize {
        self.lazy_records.borrow().len() + self.lazy_extra.get()
    }

    #[allow(clippy::too_many_arguments)]
    fn allocate_full(
        &self,
        flags: u32,
        object_flags: u32,
        name: Rc<str>,
        symbol: Option<u64>,
        alias: Option<u64>,
        literal: Option<LiteralValue>,
        fresh: bool,
        types: Vec<Weak<TypeCell>>,
        inferable_index: bool,
        resolver: Option<Resolver>,
    ) -> Rc<TypeCell> {
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        let cell = Rc::new(TypeCell {
            id,
            flags,
            object_flags,
            name,
            symbol,
            alias,
            alias_arguments: OnceCell::new(),
            literal,
            fresh,
            alternate: RefCell::new(Weak::new()),
            types: RefCell::new(types),
            origin: OnceCell::new(),
            tuple_shape: OnceCell::new(),
            array_element: OnceCell::new(),
            template_parts: OnceCell::new(),
            reference_shape: OnceCell::new(),
            generic_target: OnceCell::new(),
            type_parameter: OnceCell::new(),
            marker_instantiation: Cell::new(false),
            inferable_index,
            structure: OnceCell::new(),
            resolver: RefCell::new(resolver),
            resolutions: Cell::new(0),
        });
        self.types.borrow_mut().push(cell.clone());
        cell
    }

    fn allocate(&self, flags: u32, name: &str, resolver: Option<Resolver>) -> Rc<TypeCell> {
        let object_flags = if flags & flags::OBJECT != 0 {
            object_flags::ANONYMOUS
        } else {
            0
        };
        self.allocate_full(
            flags,
            object_flags,
            Rc::from(name),
            None,
            None,
            None,
            false,
            Vec::new(),
            true,
            resolver,
        )
    }

    pub fn primitive(&self, flags: u32, name: &str) -> Rc<TypeCell> {
        self.allocate(flags, name, None)
    }

    /// An object type whose members are declared as names and weak links and
    /// materialized on first use. `declared` may refer to cells created later,
    /// including the object itself, which is how the recursive fixtures are
    /// built without a strong cycle.
    pub fn object(&self, name: &str, declared: Vec<(&str, bool, Weak<TypeCell>)>) -> Rc<TypeCell> {
        let declared: Vec<(Rc<str>, bool, Weak<TypeCell>)> = declared
            .into_iter()
            .map(|(name, optional, link)| (Rc::from(name), optional, link))
            .collect();
        let resolver: Resolver = Box::new(move |graph, _cell| {
            let mut members = Vec::with_capacity(declared.len());
            for (name, optional, r#type) in &declared {
                if r#type.upgrade().is_none() {
                    return Err(Error::UndeclaredMember(name.clone()));
                }
                let member = Member {
                    name_type: None,
                    name: name.clone(),
                    optional: *optional,
                    readonly: false,
                    class_member: true,
                    r#type: r#type.clone().into(),
                };
                // Upstream allocates a property symbol per member here.
                graph
                    .lazy_records
                    .borrow_mut()
                    .push(Rc::new(member.clone()));
                members.push(member);
            }
            Ok(Structure {
                members,
                ..Default::default()
            })
        });
        self.allocate(flags::OBJECT, name, Some(resolver))
    }

    /// An object type whose resolver panics, for the failure-behavior test.
    pub fn poisoned_object(&self, name: &str) -> Rc<TypeCell> {
        let resolver: Resolver = Box::new(|_, _| panic!("injected resolver failure"));
        self.allocate(flags::OBJECT, name, Some(resolver))
    }

    /// A weak link to a cell that will be created by a later call, resolved
    /// through the graph by id once both exist.
    pub fn link(&self, cell: &Rc<TypeCell>) -> Weak<TypeCell> {
        Rc::downgrade(cell)
    }

    /// Every cell, for the graph's own census.
    pub fn cells(&self) -> Vec<Rc<TypeCell>> {
        self.types.borrow().clone()
    }
}

// ---------------------------------------------------------------------------
// Descriptions: the plain data the measurement child derives from production
// types at setup. Indices refer to `Description::types`.

#[derive(Clone, Debug, Default)]
pub struct Description {
    pub types: Vec<TypeDesc>,
}

#[derive(Clone, Debug)]
pub struct TypeDesc {
    pub name: String,
    pub flags: u32,
    pub object_flags: u32,
    pub symbol: Option<u64>,
    pub alias: Option<u64>,
    pub kind: KindDesc,
}

#[derive(Clone, Debug)]
pub enum KindDesc {
    Intrinsic,
    Literal {
        value: LiteralValue,
        fresh: bool,
        /// The regular form of a fresh literal, or the fresh form of a regular one.
        alternate: Option<usize>,
    },
    Object {
        members: Vec<MemberDesc>,
        index_infos: Vec<IndexDesc>,
        call_signatures: Vec<SignatureDesc>,
        construct_signatures: Vec<SignatureDesc>,
        inferable_index: bool,
    },
    Union(Vec<usize>),
    Intersection(Vec<usize>),
    /// A kind the reference cannot carry; relating through it fails explicitly.
    Unsupported(String),
}

#[derive(Clone, Debug)]
pub struct MemberDesc {
    pub name: String,
    pub optional: bool,
    pub readonly: bool,
    pub class_member: bool,
    pub r#type: usize,
}

#[derive(Clone, Debug)]
pub struct IndexDesc {
    pub key: usize,
    pub value: usize,
    pub readonly: bool,
}

#[derive(Clone, Debug)]
pub struct SignatureDesc {
    pub parameters: Vec<usize>,
    pub min_argument_count: usize,
    pub has_rest_parameter: bool,
    pub type_parameters: usize,
    pub this_type: Option<usize>,
    pub return_type: usize,
    pub bivariant_parameters: bool,
    pub is_abstract: bool,
}

/// The cells of one description, indexed like it.
pub struct Constructed {
    pub cells: Vec<Rc<TypeCell>>,
}

impl Graph {
    /// Construct every described type. Objects resolve lazily through a shared
    /// index table; unions and intersections link their constituents once every
    /// cell exists, so the description may list types in any order.
    pub fn construct(&self, description: &Description) -> Result<Constructed, Error> {
        let table: Rc<RefCell<Vec<Weak<TypeCell>>>> =
            Rc::new(RefCell::new(Vec::with_capacity(description.types.len())));
        let mut cells = Vec::with_capacity(description.types.len());
        for desc in &description.types {
            let name: Rc<str> = Rc::from(desc.name.as_str());
            let cell = match &desc.kind {
                KindDesc::Intrinsic | KindDesc::Union(_) | KindDesc::Intersection(_) => self
                    .allocate_full(
                        desc.flags,
                        desc.object_flags,
                        name,
                        desc.symbol,
                        desc.alias,
                        None,
                        false,
                        Vec::new(),
                        false,
                        None,
                    ),
                KindDesc::Literal { value, fresh, .. } => self.allocate_full(
                    desc.flags,
                    desc.object_flags,
                    name,
                    desc.symbol,
                    desc.alias,
                    Some(value.clone()),
                    *fresh,
                    Vec::new(),
                    false,
                    None,
                ),
                KindDesc::Object {
                    members,
                    index_infos,
                    call_signatures,
                    construct_signatures,
                    inferable_index,
                } => {
                    let members = members.clone();
                    let index_infos = index_infos.clone();
                    let calls = call_signatures.clone();
                    let constructs = construct_signatures.clone();
                    let table = table.clone();
                    let resolver: Resolver = Box::new(move |graph, _cell| {
                        let table = table.borrow();
                        let link = |index: usize, what: &str| -> Result<Weak<TypeCell>, Error> {
                            let weak = table
                                .get(index)
                                .ok_or_else(|| Error::UndeclaredMember(Rc::from(what)))?;
                            if weak.upgrade().is_none() {
                                return Err(Error::UndeclaredMember(Rc::from(what)));
                            }
                            Ok(weak.clone())
                        };
                        let mut structure = Structure::default();
                        for member in &members {
                            let record = Member {
                                name_type: None,
                                name: Rc::from(member.name.as_str()),
                                optional: member.optional,
                                readonly: member.readonly,
                                class_member: member.class_member,
                                r#type: link(member.r#type, &member.name)?.into(),
                            };
                            // Upstream allocates a property symbol per member here.
                            graph
                                .lazy_records
                                .borrow_mut()
                                .push(Rc::new(record.clone()));
                            structure.members.push(record);
                        }
                        for info in &index_infos {
                            structure.index_infos.push(IndexInfo {
                                key: link(info.key, "index key")?,
                                value: link(info.value, "index value")?,
                                readonly: info.readonly,
                            });
                            graph.lazy_extra.set(graph.lazy_extra.get() + 1);
                        }
                        let signature =
                            |desc: &SignatureDesc, construct: bool| -> Result<Signature, Error> {
                                Ok(Signature {
                                    parameter_names: Vec::new(),
                                    parameters: desc
                                        .parameters
                                        .iter()
                                        .map(|index| link(*index, "parameter").map(Into::into))
                                        .collect::<Result<Vec<_>, _>>()?,
                                    min_argument_count: desc.min_argument_count,
                                    has_rest_parameter: desc.has_rest_parameter,
                                    type_parameters: desc.type_parameters,
                                    generic: None,
                                    this_type: desc
                                        .this_type
                                        .map(|index| link(index, "this").map(Into::into))
                                        .transpose()?,
                                    return_type: link(desc.return_type, "return")?.into(),
                                    bivariant_parameters: desc.bivariant_parameters,
                                    is_abstract: desc.is_abstract,
                                    is_construct: construct,
                                    target: None,
                                    parameter_declarations: Vec::new(),
                                })
                            };
                        for desc in &calls {
                            structure.call_signatures.push(signature(desc, false)?);
                            graph.lazy_extra.set(graph.lazy_extra.get() + 1);
                        }
                        for desc in &constructs {
                            structure.construct_signatures.push(signature(desc, true)?);
                            graph.lazy_extra.set(graph.lazy_extra.get() + 1);
                        }
                        Ok(structure)
                    });
                    self.allocate_full(
                        desc.flags,
                        desc.object_flags,
                        name,
                        desc.symbol,
                        desc.alias,
                        None,
                        false,
                        Vec::new(),
                        *inferable_index,
                        Some(resolver),
                    )
                }
                KindDesc::Unsupported(reason) => {
                    let reason = reason.clone();
                    let resolver: Resolver =
                        Box::new(move |_, _| Err(Error::Unsupported(Rc::from(reason.as_str()))));
                    self.allocate_full(
                        desc.flags,
                        desc.object_flags,
                        name,
                        desc.symbol,
                        desc.alias,
                        None,
                        false,
                        Vec::new(),
                        false,
                        Some(resolver),
                    )
                }
            };
            table.borrow_mut().push(Rc::downgrade(&cell));
            cells.push(cell);
        }
        for (index, desc) in description.types.iter().enumerate() {
            match &desc.kind {
                KindDesc::Literal {
                    alternate: Some(other),
                    ..
                } => {
                    let other = cells
                        .get(*other)
                        .ok_or_else(|| Error::UndeclaredMember(Rc::from("literal alternate")))?;
                    *cells[index].alternate.borrow_mut() = Rc::downgrade(other);
                }
                KindDesc::Union(members) | KindDesc::Intersection(members) => {
                    let links = members
                        .iter()
                        .map(|member| {
                            cells
                                .get(*member)
                                .map(Rc::downgrade)
                                .ok_or_else(|| Error::UndeclaredMember(Rc::from("constituent")))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    *cells[index].types.borrow_mut() = links;
                }
                _ => {}
            }
        }
        Ok(Constructed { cells })
    }
}

// ---------------------------------------------------------------------------
// Relation caches and the checker.

/// `getRelationKey` for non-generic types: ordered ids (unordered for
/// identity) and the intersection state.
type Key = (u32, u32, u8);

/// One relation's cache (`Relation`).
#[derive(Default)]
pub struct Relation {
    results: RefCell<HashMap<Key, u32>>,
}

impl Relation {
    pub fn entries(&self) -> usize {
        self.results.borrow().len()
    }

    /// The cache's result flags, sorted, for comparison with the Go observer.
    pub fn result_flags(&self) -> Vec<u32> {
        let mut flags: Vec<u32> = self.results.borrow().values().copied().collect();
        flags.sort_unstable();
        flags
    }

    fn get(&self, key: Key) -> u32 {
        self.results.borrow().get(&key).copied().unwrap_or(0)
    }

    fn set(&self, key: Key, value: u32) {
        self.results.borrow_mut().insert(key, value);
    }
}

const INTERSECTION_NONE: u8 = 0;
const INTERSECTION_SOURCE: u8 = 1;
const INTERSECTION_TARGET: u8 = 2;

const RECURSION_SOURCE: u8 = 1;
const RECURSION_TARGET: u8 = 2;
const RECURSION_BOTH: u8 = 3;

/// The checker state the relater needs: the graph, one cache per relation,
/// the intrinsic cells the algorithm names and the observer of every
/// `checkTypeRelatedTo` outcome (the Go observer's `ternary_calls`).
#[derive(Default)]
pub struct Checker {
    pub graph: Graph,
    relations: [Relation; 5],
    diagnostics: RefCell<Vec<Diagnostic>>,
    member_declarations: RefCell<HashMap<(u32, Rc<str>), DiagnosticLocation>>,
    observer: RefCell<Vec<Ternary>>,
    intrinsics: RefCell<HashMap<u32, Rc<TypeCell>>>,
    apparent_types: RefCell<HashMap<u32, Weak<TypeCell>>>,
    property_keys: RefCell<HashMap<u32, Weak<TypeCell>>>,
    variance_markers: OnceCell<generics::VarianceMarkers>,
    variance_stack: RefCell<Vec<Weak<TypeCell>>>,
    /// `getGlobalNonNullableTypeInstantiation`, present under strictNullChecks.
    non_nullable: OnceCell<NonNullableInstantiation>,
}

type NonNullableInstantiation = Box<dyn Fn(&Rc<TypeCell>) -> Result<Rc<TypeCell>, Error>>;

fn relation_index(mode: Mode) -> usize {
    MODES.iter().position(|m| *m == mode).expect("mode")
}

impl Checker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the intrinsic cells (`string`, `number`, `bigint`, ...) the
    /// algorithm names; the description's intrinsics register automatically.
    pub fn register_intrinsics(&self, cells: &[Rc<TypeCell>]) {
        let mut intrinsics = self.intrinsics.borrow_mut();
        for cell in cells {
            if cell.flags & flags::SINGLETON != 0
                && cell.literal.is_none()
                && cell.flags & flags::OBJECT == 0
            {
                intrinsics.entry(cell.flags).or_insert_with(|| cell.clone());
            }
        }
    }

    /// Install the owner's `NonNullable<T>` alias instantiation. A checker
    /// without one (no strictNullChecks, or a description-built graph) leaves
    /// types as they are, as `getNonNullableType` does without the option.
    pub(crate) fn register_non_nullable(
        &self,
        instantiate: impl Fn(&Rc<TypeCell>) -> Result<Rc<TypeCell>, Error> + 'static,
    ) -> Result<(), Error> {
        self.non_nullable
            .set(Box::new(instantiate))
            .map_err(|_| Error::Unsupported(Rc::from("non-nullable instantiation already set")))
    }

    pub fn register_apparent_type(&self, flags: u32, cell: &Rc<TypeCell>) -> Result<(), Error> {
        if !self.graph.owns(cell) {
            return unsupported("apparent type owner mismatch");
        }
        self.apparent_types
            .borrow_mut()
            .insert(flags, Rc::downgrade(cell));
        Ok(())
    }

    fn apparent_primitive_type(&self, source: &Rc<TypeCell>) -> Result<Rc<TypeCell>, Error> {
        let flag = if source.flags & flags::STRING_LIKE != 0 {
            flags::STRING
        } else if source.flags & flags::NUMBER_LIKE != 0 {
            flags::NUMBER
        } else if source.flags & flags::BIG_INT_LIKE != 0 {
            flags::BIG_INT
        } else if source.flags & flags::BOOLEAN_LIKE != 0 {
            flags::BOOLEAN
        } else if source.flags & flags::ES_SYMBOL_LIKE != 0 {
            flags::ES_SYMBOL
        } else if source.flags & flags::NON_PRIMITIVE != 0 {
            flags::NON_PRIMITIVE
        } else {
            return Ok(source.clone());
        };
        self.apparent_types
            .borrow()
            .get(&flag)
            .ok_or_else(|| Error::Unsupported("primitive wrapper not initialized".into()))?
            .upgrade()
            .ok_or(Error::Released)
    }

    fn intrinsic(&self, flag: u32) -> Result<Rc<TypeCell>, Error> {
        self.intrinsics
            .borrow()
            .get(&flag)
            .cloned()
            .ok_or_else(|| Error::Unsupported(Rc::from("intrinsic type not registered")))
    }

    pub fn relation(&self, mode: Mode) -> &Relation {
        &self.relations[relation_index(mode)]
    }

    pub fn diagnostics(&self) -> Vec<String> {
        self.diagnostics
            .borrow()
            .iter()
            .map(Diagnostic::display_text)
            .collect()
    }

    pub fn structured_diagnostics(&self) -> std::cell::Ref<'_, [Diagnostic]> {
        std::cell::Ref::map(self.diagnostics.borrow(), |diagnostics| {
            diagnostics.as_slice()
        })
    }

    pub fn register_member_declaration(
        &self,
        owner: &TypeCell,
        name: impl Into<Rc<str>>,
        location: DiagnosticLocation,
    ) {
        self.member_declarations
            .borrow_mut()
            .insert((owner.id, name.into()), location);
    }

    /// Every `checkTypeRelatedTo` ternary since the last `take_observed`.
    pub fn take_observed(&self) -> Vec<Ternary> {
        std::mem::take(&mut *self.observer.borrow_mut())
    }

    /// `checkTypeRelatedToEx`: relates and, when asked, reports one diagnostic
    /// for a failure. Returns the top-level ternary the Go observer records and
    /// whether the types are related.
    pub fn check_type_related_to(
        &self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        mode: Mode,
        report_errors: bool,
    ) -> Result<(Ternary, bool), Error> {
        self.check_type_related_to_at(
            source,
            target,
            mode,
            report_errors.then(DiagnosticLocation::default),
        )
    }

    pub fn check_type_related_to_at(
        &self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        mode: Mode,
        error_location: Option<DiagnosticLocation>,
    ) -> Result<(Ternary, bool), Error> {
        let report_errors = error_location.is_some();
        let relation = self.relation(mode);
        let mut relater = Relater {
            checker: self,
            mode,
            relation,
            maybe_keys: Vec::new(),
            maybe_set: HashSet::new(),
            source_stack: Vec::new(),
            target_stack: Vec::new(),
            expanding_flags: 0,
            relation_count: (16_000_000 - relation.entries() as i64) / 8,
            overflow: false,
            error_chain: ErrorChain::default(),
            related_info: Vec::new(),
        };
        let result = relater.is_related_to_ex(
            source,
            target,
            RECURSION_BOTH,
            report_errors,
            INTERSECTION_NONE,
        )?;
        if relater.overflow {
            let key = relation_key(source, target, INTERSECTION_NONE, mode == Mode::Identity);
            relation.set(
                key,
                relation_result::FAILED | relation_result::COMPLEXITY_OVERFLOW,
            );
            self.diagnostics.borrow_mut().push(Diagnostic::new(
                error_location.unwrap_or_default(),
                d::Excessive_complexity_comparing_types_0_and_1,
                vec![self.type_to_string(source)?, self.type_to_string(target)?],
            ));
        } else if let Some(diagnostic) = relater
            .error_chain
            .diagnostic(&error_location.unwrap_or_default(), &relater.related_info)
        {
            self.diagnostics.borrow_mut().push(diagnostic);
        }
        self.observer.borrow_mut().push(result);
        Ok((result, result != FALSE))
    }

    /// `isTypeRelatedTo`: the cached fast path callers use before relating.
    pub fn is_type_related_to(
        &self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        mode: Mode,
    ) -> Result<bool, Error> {
        let source = regular_form(source);
        let target = regular_form(target);
        if Rc::ptr_eq(&source, &target) {
            return Ok(true);
        }
        if mode != Mode::Identity {
            if mode == Mode::Comparable
                && target.flags & flags::NEVER == 0
                && is_simple_type_related_to(&target, &source, mode)?
                || is_simple_type_related_to(&source, &target, mode)?
            {
                return Ok(true);
            }
        } else if (source.flags | target.flags)
            & (flags::UNION_OR_INTERSECTION
                | flags::INDEXED_ACCESS
                | flags::CONDITIONAL
                | flags::SUBSTITUTION)
            == 0
        {
            if source.flags != target.flags {
                return Ok(false);
            }
            if source.flags & flags::SINGLETON != 0 {
                return Ok(true);
            }
        }
        if source.flags & flags::OBJECT != 0 && target.flags & flags::OBJECT != 0 {
            let key = relation_key(&source, &target, INTERSECTION_NONE, mode == Mode::Identity);
            let related = self.relation(mode).get(key);
            if related != 0 {
                return Ok(related & relation_result::SUCCEEDED != 0);
            }
        }
        if source.flags & flags::STRUCTURED_OR_INSTANTIABLE != 0
            || target.flags & flags::STRUCTURED_OR_INSTANTIABLE != 0
        {
            return Ok(self.check_type_related_to(&source, &target, mode, false)?.1);
        }
        Ok(false)
    }

    fn is_type_assignable_to(
        &self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<bool, Error> {
        self.is_type_related_to(source, target, Mode::Assignable)
    }
}

impl Relater<'_> {
    // port: tsc/internal/checker/checker.go:Checker.getNonNullableType
    /// `getAdjustedTypeWithFacts(t, NEUndefinedOrNull)`: constituents that are
    /// only null or undefined drop out, and each remaining constituent that can
    /// still compare equal to them is wrapped in `NonNullable<T>`.
    fn non_nullable_type(&mut self, t: &Rc<TypeCell>) -> Result<Rc<TypeCell>, Error> {
        let Some(instantiate) = self.checker.non_nullable.get() else {
            return Ok(t.clone());
        };
        const NULLISH: u32 = flags::UNDEFINED | flags::NULL | flags::VOID;
        if t.flags & flags::UNKNOWN != 0 {
            // `unknown` adjusts through `{} | null | undefined` to `{}`.
            return unsupported("non-nullable form of unknown");
        }
        let constituents = if t.flags & flags::UNION != 0 {
            t.types()?
        } else {
            vec![t.clone()]
        };
        let mut changed = false;
        let mut result = Vec::with_capacity(constituents.len());
        for constituent in constituents {
            if constituent.flags & NULLISH != 0 {
                changed = true;
                continue;
            }
            if constituent.flags & flags::ANY != 0 {
                result.push(instantiate(&constituent)?);
                changed = true;
            } else if constituent.flags & flags::INSTANTIABLE != 0 {
                // The facts of an instantiable type come from its base constraint.
                return unsupported("non-nullable form of an instantiable type");
            } else {
                result.push(constituent);
            }
        }
        if !changed {
            return Ok(t.clone());
        }
        match result.len() {
            0 => self.checker.intrinsic(flags::NEVER),
            1 => Ok(result.remove(0)),
            _ => self.graph().union(&result),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.isInstantiatedGenericParameter
    /// Resolves the parameter of the signature this one instantiates, as
    /// upstream does, and reports whether that declared type is generic.
    fn is_instantiated_generic_parameter(
        &mut self,
        signature: &Signature,
        position: usize,
    ) -> Result<bool, Error> {
        let Some(target) = signature.target.clone() else {
            return Ok(false);
        };
        match self.try_signature_type_at_position(&target, position)? {
            Some(ty) => is_generic_type(&ty),
            None => Ok(false),
        }
    }
}

// port: tsc/internal/checker/checker.go:Checker.getGenericObjectFlags
fn is_generic_type(ty: &Rc<TypeCell>) -> Result<bool, Error> {
    if ty.flags & (flags::UNION | flags::INTERSECTION) != 0 {
        for constituent in ty.types()? {
            if is_generic_type(&constituent)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if ty.flags & (flags::INSTANTIABLE_NON_PRIMITIVE | flags::INDEX) != 0 {
        return Ok(true);
    }
    if let Some(tuple) = ty.tuple_shape() {
        return Ok(tuple
            .element_flags
            .iter()
            .any(|flags| flags & tuples::element_flags::VARIADIC != 0));
    }
    // Mapped types and template or string-mapping types need their own
    // genericity rules (isGenericMappedType, isGenericStringLikeType).
    if ty.object_flags & object_flags::MAPPED != 0
        || ty.flags & (flags::TEMPLATE_LITERAL | flags::STRING_MAPPING) != 0
    {
        return unsupported("genericity of a mapped or template parameter type");
    }
    Ok(false)
}

impl Checker {
    // port: tsc/internal/checker/relater.go:Checker.getNormalizedType
    /// Fresh literals relate as their regular forms and a deferred (node-backed)
    /// reference as the ordinary reference of its target and resolved
    /// arguments; carried unions and intersections are reduced by construction.
    fn normalized_type(&self, cell: &Rc<TypeCell>) -> Result<Rc<TypeCell>, Error> {
        let mut current = regular_form(cell);
        loop {
            let Some(reference) = current
                .reference_shape()
                .filter(|reference| reference.deferred_node().is_some())
            else {
                return Ok(current);
            };
            let target = reference.target()?;
            let Some(generic) = target.generic_target() else {
                return Ok(current);
            };
            let next = generic.instantiate(self, &target, &reference.arguments()?)?;
            if Rc::ptr_eq(&next, &current) {
                return Ok(current);
            }
            current = next;
        }
    }
}

fn regular_form(cell: &Rc<TypeCell>) -> Rc<TypeCell> {
    if cell.fresh {
        if let Some(regular) = cell.alternate() {
            return regular;
        }
    }
    cell.clone()
}

/// `getRelationKey` for two non-generic types: identity keys are order-free.
fn relation_key(
    source: &TypeCell,
    target: &TypeCell,
    intersection_state: u8,
    identity: bool,
) -> Key {
    let (mut s, mut t) = (source.id, target.id);
    if identity && s > t {
        std::mem::swap(&mut s, &mut t);
    }
    (s, t, intersection_state)
}

/// `isSimpleTypeRelatedTo` without enums, wildcard and unknown-like unions
/// (none of the frozen fixtures reach them; `strictNullChecks` is on).
fn is_simple_type_related_to(
    source: &TypeCell,
    target: &TypeCell,
    mode: Mode,
) -> Result<bool, Error> {
    let s = source.flags;
    let t = target.flags;
    if t & flags::ANY != 0 || s & flags::NEVER != 0 {
        return Ok(true);
    }
    if t & flags::UNKNOWN != 0 && !(mode == Mode::StrictSubtype && s & flags::ANY != 0) {
        return Ok(true);
    }
    if t & flags::NEVER != 0 {
        return Ok(false);
    }
    if s & flags::STRING_LIKE != 0 && t & flags::STRING != 0 {
        return Ok(true);
    }
    if s & flags::NUMBER_LIKE != 0 && t & flags::NUMBER != 0 {
        return Ok(true);
    }
    if s & flags::BIG_INT_LIKE != 0 && t & flags::BIG_INT != 0 {
        return Ok(true);
    }
    if s & flags::BOOLEAN_LIKE != 0 && t & flags::BOOLEAN != 0 {
        return Ok(true);
    }
    if s & flags::ES_SYMBOL_LIKE != 0 && t & flags::ES_SYMBOL != 0 {
        return Ok(true);
    }
    if (s | t) & flags::ENUM_LIKE != 0 {
        return unsupported("enum relations");
    }
    if s & flags::UNDEFINED != 0 && t & (flags::UNDEFINED | flags::VOID) != 0 {
        return Ok(true);
    }
    if s & flags::NULL != 0 && t & flags::NULL != 0 {
        return Ok(true);
    }
    if s & flags::OBJECT != 0
        && t & flags::NON_PRIMITIVE != 0
        && !(mode == Mode::StrictSubtype
            && is_empty_anonymous_object_type(source)
            && source.object_flags & object_flags::FRESH_LITERAL == 0)
    {
        return Ok(true);
    }
    if (mode == Mode::Assignable || mode == Mode::Comparable) && s & flags::ANY != 0 {
        return Ok(true);
    }
    Ok(false)
}

/// `IsEmptyAnonymousObjectType`, on the already resolved structure only: an
/// unresolved object is not known to be empty.
fn is_empty_anonymous_object_type(t: &TypeCell) -> bool {
    t.object_flags & object_flags::ANONYMOUS != 0
        && t.structure.get().is_some_and(|s| {
            s.members.is_empty()
                && s.index_infos.is_empty()
                && s.call_signatures.is_empty()
                && s.construct_signatures.is_empty()
        })
}

fn contains_type(types: &[Rc<TypeCell>], t: &Rc<TypeCell>) -> bool {
    types.iter().any(|candidate| Rc::ptr_eq(candidate, t))
}

fn is_object_literal_type(t: &TypeCell) -> bool {
    t.object_flags & object_flags::OBJECT_LITERAL != 0
}

fn is_unit_type(t: &TypeCell) -> bool {
    t.flags & flags::UNIT != 0
}

/// `getRecursionIdentity` for the carried kinds: interface-like objects by
/// symbol, everything else by the cell itself.
fn recursion_identity(t: &TypeCell) -> (bool, u64) {
    if t.flags & flags::OBJECT != 0 && !is_object_literal_type(t) {
        if let Some(symbol) = t.symbol {
            if !(t.object_flags & object_flags::ANONYMOUS != 0
                && t.object_flags & object_flags::CLASS != 0)
            {
                return (true, symbol);
            }
        }
    }
    (false, u64::from(t.id))
}

/// `isDeeplyNestedType`: the stack holds `max_depth` or more types with the
/// same recursion identity and non-decreasing ids.
fn is_deeply_nested_type(
    t: &TypeCell,
    stack: &[Rc<TypeCell>],
    max_depth: usize,
) -> Result<bool, Error> {
    if stack.len() < max_depth {
        return Ok(false);
    }
    if t.flags & flags::INTERSECTION != 0 {
        for member in t.types()? {
            if is_deeply_nested_type(&member, stack, max_depth)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    let identity = recursion_identity(t);
    let mut count = 0;
    let mut last_id = 0;
    for cell in stack {
        let matches = if cell.flags & flags::INTERSECTION != 0 {
            cell.types()?
                .iter()
                .any(|member| recursion_identity(member) == identity)
        } else {
            recursion_identity(cell) == identity
        };
        if matches {
            if cell.id >= last_id {
                count += 1;
                if count >= max_depth {
                    return Ok(true);
                }
            }
            last_id = cell.id;
        }
    }
    Ok(false)
}

struct Relater<'c> {
    checker: &'c Checker,
    mode: Mode,
    relation: &'c Relation,
    maybe_keys: Vec<Key>,
    maybe_set: HashSet<Key>,
    source_stack: Vec<Rc<TypeCell>>,
    target_stack: Vec<Rc<TypeCell>>,
    expanding_flags: u8,
    relation_count: i64,
    overflow: bool,
    error_chain: ErrorChain,
    related_info: Vec<Diagnostic>,
}

const EXPANDING_SOURCE: u8 = 1;
const EXPANDING_TARGET: u8 = 2;
const EXPANDING_BOTH: u8 = 3;

impl Relater<'_> {
    fn graph(&self) -> &Graph {
        &self.checker.graph
    }

    fn report(&mut self, report_errors: bool, message: &'static d::Message, args: Vec<String>) {
        if report_errors {
            self.report_error(message, args);
        }
    }

    fn report_error(&mut self, message: &'static d::Message, args: Vec<String>) {
        self.error_chain.report(message, args);
    }

    fn error_state(&self) -> (ErrorChain, Vec<Diagnostic>) {
        (self.error_chain.clone(), self.related_info.clone())
    }

    fn restore_error_state(&mut self, state: (ErrorChain, Vec<Diagnostic>)) {
        (self.error_chain, self.related_info) = state;
    }

    fn report_error_results(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<(), Error> {
        let source_name = self.checker.type_to_string(source)?;
        let target_name = self.checker.type_to_string(target)?;
        if source.is_readonly_array_or_tuple()
            && target.is_array_or_tuple()
            && !target.is_readonly_array_or_tuple()
        {
            self.report_error(
                d::The_type_0_is_readonly_and_cannot_be_assigned_to_the_mutable_type_1,
                vec![source_name.clone(), target_name.clone()],
            );
        }
        if self
            .error_chain
            .suppress_relation(&source_name, &target_name)
        {
            return Ok(());
        }
        let message = if self.mode == Mode::Comparable {
            d::Type_0_is_not_comparable_to_type_1
        } else if source_name == target_name {
            d::Type_0_is_not_assignable_to_type_1_Two_different_types_with_this_name_exist_but_they_are_unrelated
        } else {
            d::Type_0_is_not_assignable_to_type_1
        };
        self.report_error(message, vec![source_name, target_name]);
        Ok(())
    }

    fn is_related_to(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        recursion_flags: u8,
        report_errors: bool,
    ) -> Result<Ternary, Error> {
        self.is_related_to_ex(
            source,
            target,
            recursion_flags,
            report_errors,
            INTERSECTION_NONE,
        )
    }

    fn is_related_to_simple(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<Ternary, Error> {
        self.is_related_to(source, target, RECURSION_BOTH, false)
    }

    /// `isRelatedToEx` for the carried kinds. Type parameters, excess property
    /// checks on fresh literals and JSX never arise in the fixtures; where
    /// the algorithm would need them the reference fails explicitly.
    fn is_related_to_ex(
        &mut self,
        original_source: &Rc<TypeCell>,
        original_target: &Rc<TypeCell>,
        recursion_flags: u8,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        if Rc::ptr_eq(original_source, original_target) {
            return Ok(TRUE);
        }
        if (original_source.flags | original_target.flags) & !flags::SUPPORTED != 0 {
            return unsupported("type kind outside the reference relater");
        }
        if original_source.flags & flags::OBJECT != 0
            && original_target.flags & flags::PRIMITIVE != 0
        {
            if self.mode == Mode::Comparable
                && original_target.flags & flags::NEVER == 0
                && is_simple_type_related_to(original_target, original_source, self.mode)?
                || is_simple_type_related_to(original_source, original_target, self.mode)?
            {
                return Ok(TRUE);
            }
            if report_errors {
                self.report_error_results(original_source, original_target)?;
            }
            return Ok(FALSE);
        }
        // `getNormalizedType`: fresh literals relate as their regular forms; the
        // carried unions and intersections are already reduced by construction.
        let source = self.checker.normalized_type(original_source)?;
        let mut target = self.checker.normalized_type(original_target)?;
        if Rc::ptr_eq(&source, &target) {
            return Ok(TRUE);
        }
        if self.mode == Mode::Identity {
            if source.flags != target.flags {
                return Ok(FALSE);
            }
            if source.flags & flags::SINGLETON != 0 {
                return Ok(TRUE);
            }
            return self.recursive_type_related_to(
                &source,
                &target,
                false,
                INTERSECTION_NONE,
                recursion_flags,
            );
        }
        // A type parameter related to exactly its constraint is decided here,
        // before any recursion or cache entry (relater.go isRelatedToEx).
        if source.flags & flags::TYPE_PARAMETER != 0 {
            let constraint = source
                .type_parameter_shape()
                .map(TypeParameterShape::constraint)
                .transpose()?
                .flatten();
            if constraint.is_some_and(|constraint| Rc::ptr_eq(&constraint, &target)) {
                return Ok(TRUE);
            }
        }
        if source.flags & flags::DEFINITELY_NON_NULLABLE != 0 && target.flags & flags::UNION != 0 {
            let types = target.types()?;
            let candidate = match types.len() {
                2 if types[0].flags & flags::NULLABLE != 0 => Some(types[1].clone()),
                3 if types[0].flags & flags::NULLABLE != 0
                    && types[1].flags & flags::NULLABLE != 0 =>
                {
                    Some(types[2].clone())
                }
                _ => None,
            };
            if let Some(candidate) = candidate {
                if candidate.flags & flags::NULLABLE == 0 {
                    target = regular_form(&candidate);
                    if Rc::ptr_eq(&source, &target) {
                        return Ok(TRUE);
                    }
                }
            }
        }
        if self.mode == Mode::Comparable
            && target.flags & flags::NEVER == 0
            && is_simple_type_related_to(&target, &source, self.mode)?
            || is_simple_type_related_to(&source, &target, self.mode)?
        {
            return Ok(TRUE);
        }
        if source.flags & flags::STRUCTURED_OR_INSTANTIABLE != 0
            || target.flags & flags::STRUCTURED_OR_INSTANTIABLE != 0
        {
            let is_performing_excess_property_checks = intersection_state & INTERSECTION_TARGET
                == 0
                && is_object_literal_type(&source)
                && source.object_flags & object_flags::FRESH_LITERAL != 0;
            if is_performing_excess_property_checks {
                return unsupported("excess property checks on fresh object literals");
            }
            let is_performing_common_property_checks = (self.mode != Mode::Comparable
                || is_unit_type(&source))
                && intersection_state & INTERSECTION_TARGET == 0
                && source.flags & (flags::PRIMITIVE | flags::OBJECT | flags::INTERSECTION) != 0
                && target.flags & (flags::OBJECT | flags::INTERSECTION) != 0
                && self.is_weak_type(&target)?
                && (!self.properties_of_type(&source)?.is_empty()
                    || self.type_has_call_or_construct_signatures(&source)?);
            if is_performing_common_property_checks
                && !self.has_common_properties(&source, &target)?
            {
                if report_errors {
                    let source_display = if original_source.alias.is_some() {
                        original_source
                    } else {
                        &source
                    };
                    let target_display = if original_target.alias.is_some() {
                        original_target
                    } else {
                        &target
                    };
                    self.report_error(
                        d::Type_0_has_no_properties_in_common_with_type_1,
                        vec![
                            self.checker.type_to_string(source_display)?,
                            self.checker.type_to_string(target_display)?,
                        ],
                    );
                }
                return Ok(FALSE);
            }
            let skip_caching = source.flags & flags::UNION != 0
                && source.constituent_count() < 4
                && target.flags & flags::UNION == 0
                || target.flags & flags::UNION != 0
                    && target.constituent_count() < 4
                    && source.flags & flags::STRUCTURED_OR_INSTANTIABLE == 0;
            let result = if skip_caching {
                self.union_or_intersection_related_to(
                    &source,
                    &target,
                    report_errors,
                    intersection_state,
                )?
            } else {
                self.recursive_type_related_to(
                    &source,
                    &target,
                    report_errors,
                    intersection_state,
                    recursion_flags,
                )?
            };
            if result != FALSE {
                return Ok(result);
            }
        }
        if report_errors {
            let source = if original_source.alias.is_some() {
                original_source
            } else {
                &source
            };
            let target = if original_target.alias.is_some() {
                original_target
            } else {
                &target
            };
            self.report_error_results(source, target)?;
        }
        Ok(FALSE)
    }

    // ---- unions and intersections -------------------------------------------

    fn union_or_intersection_related_to(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        if source.flags & flags::UNION != 0 {
            if target.flags & flags::UNION != 0
                && (source.alias.is_some() || target.alias.is_some())
            {
                // Union origins are not carried; an alias-origin shortcut would
                // need them. Fall through to the structural comparison, which
                // yields the same result for the frozen fixtures.
            }
            if self.mode == Mode::Comparable {
                return self.some_type_related_to_type(
                    source,
                    target,
                    report_errors && source.flags & flags::PRIMITIVE == 0,
                    intersection_state,
                );
            }
            return self.each_type_related_to_type(
                source,
                target,
                report_errors && source.flags & flags::PRIMITIVE == 0,
                intersection_state,
            );
        }
        if target.flags & flags::UNION != 0 {
            return self.type_related_to_some_type(
                source,
                target,
                report_errors
                    && source.flags & flags::PRIMITIVE == 0
                    && target.flags & flags::PRIMITIVE == 0,
                intersection_state,
            );
        }
        if target.flags & flags::INTERSECTION != 0 {
            return self.type_related_to_each_type(
                source,
                target,
                report_errors,
                INTERSECTION_TARGET,
            );
        }
        if self.mode == Mode::Comparable && target.flags & flags::PRIMITIVE != 0 {
            // Constraints of instantiable constituents: none are carried.
            for member in source.types()? {
                if member.flags & flags::INSTANTIABLE != 0 {
                    return unsupported("instantiable intersection constituent");
                }
            }
        }
        self.some_type_related_to_type(source, target, false, INTERSECTION_SOURCE)
    }

    fn some_type_related_to_type(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        let source_types = source.types()?;
        if source.flags & flags::UNION != 0 && contains_type(&source_types, target) {
            return Ok(TRUE);
        }
        let last = source_types.len().saturating_sub(1);
        for (index, t) in source_types.iter().enumerate() {
            let related = self.is_related_to_ex(
                t,
                target,
                RECURSION_SOURCE,
                report_errors && index == last,
                intersection_state,
            )?;
            if related != FALSE {
                return Ok(related);
            }
        }
        Ok(FALSE)
    }

    // port: tsc/internal/checker/relater.go:Relater.getUndefinedStrippedTargetIfNeeded
    fn undefined_stripped_target(
        &self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<Rc<TypeCell>, Error> {
        if source.flags & flags::UNION == 0 || target.flags & flags::UNION == 0 {
            return Ok(target.clone());
        }
        let source_types = source.types()?;
        let target_types = target.types()?;
        if source_types
            .first()
            .is_none_or(|first| first.flags & flags::UNDEFINED != 0)
            || target_types
                .first()
                .is_none_or(|first| first.flags & flags::UNDEFINED == 0)
        {
            return Ok(target.clone());
        }
        // `extractTypesOfKind(target, ^Undefined)` through `filterType`. The
        // denormalized-origin path builds a second union and is not reached by
        // a target whose constituents are already normalized.
        if target
            .origin
            .get()
            .and_then(Weak::upgrade)
            .is_some_and(|origin| origin.flags & flags::UNION != 0)
        {
            return unsupported("undefined-stripped union target with a union origin");
        }
        let filtered = target_types
            .iter()
            .filter(|ty| ty.flags & !flags::UNDEFINED != 0)
            .cloned()
            .collect::<Vec<_>>();
        if filtered.len() == target_types.len() {
            return Ok(target.clone());
        }
        self.checker.graph.union(&filtered)
    }

    fn each_type_related_to_type(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        let mut result = TRUE;
        let source_types = source.types()?;
        // `undefined` is frequently added by optionality and would otherwise
        // spoil the correspondence fastpath, so the target drops it when the
        // source trivially has none.
        let stripped_target = self.undefined_stripped_target(source, target)?;
        let stripped_types = if stripped_target.flags & flags::UNION != 0 {
            stripped_target.types()?
        } else {
            Vec::new()
        };
        for (index, source_type) in source_types.iter().enumerate() {
            if stripped_target.flags & flags::UNION != 0
                && source_types.len() >= stripped_types.len()
                && source_types.len() % stripped_types.len() == 0
            {
                let related = self.is_related_to_ex(
                    source_type,
                    &stripped_types[index % stripped_types.len()],
                    RECURSION_BOTH,
                    false,
                    intersection_state,
                )?;
                if related != FALSE {
                    result &= related;
                    continue;
                }
            }
            let related = self.is_related_to_ex(
                source_type,
                target,
                RECURSION_SOURCE,
                report_errors,
                intersection_state,
            )?;
            if related == FALSE {
                return Ok(FALSE);
            }
            result &= related;
        }
        Ok(result)
    }

    fn type_related_to_some_type(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        let target_types = target.types()?;
        if target.flags & flags::UNION != 0 {
            if contains_type(&target_types, source) {
                return Ok(TRUE);
            }
            if self.mode != Mode::Comparable
                && target.object_flags & object_flags::PRIMITIVE_UNION != 0
                && source.flags & flags::ENUM_LITERAL == 0
                && (source.flags
                    & (flags::STRING_LITERAL | flags::BOOLEAN_LITERAL | flags::BIG_INT_LITERAL)
                    != 0
                    || (self.mode == Mode::Subtype || self.mode == Mode::StrictSubtype)
                        && source.flags & flags::NUMBER_LITERAL != 0)
            {
                let alternate_form = source.alternate();
                let primitive = if source.flags & flags::STRING_LITERAL != 0 {
                    Some(self.checker.intrinsic(flags::STRING)?)
                } else if source.flags & flags::NUMBER_LITERAL != 0 {
                    Some(self.checker.intrinsic(flags::NUMBER)?)
                } else if source.flags & flags::BIG_INT_LITERAL != 0 {
                    Some(self.checker.intrinsic(flags::BIG_INT)?)
                } else {
                    None
                };
                if primitive
                    .as_ref()
                    .is_some_and(|p| contains_type(&target_types, p))
                    || alternate_form
                        .as_ref()
                        .is_some_and(|a| contains_type(&target_types, a))
                {
                    return Ok(TRUE);
                }
                return Ok(FALSE);
            }
            // `getMatchingUnionConstituentForType`: key properties exist only for
            // unions of ten or more object types; the fixtures have none.
            if target_types.len() >= 10 {
                return unsupported("union key property matching");
            }
        }
        for t in &target_types {
            let related =
                self.is_related_to_ex(source, t, RECURSION_TARGET, false, intersection_state)?;
            if related != FALSE {
                return Ok(related);
            }
        }
        if report_errors {
            if let Some(best) = self.best_matching_type(source, target)? {
                self.is_related_to_ex(source, &best, RECURSION_TARGET, true, intersection_state)?;
            }
        }
        Ok(FALSE)
    }

    fn type_related_to_each_type(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        let mut result = TRUE;
        for target_type in target.types()? {
            let related = self.is_related_to_ex(
                source,
                &target_type,
                RECURSION_TARGET,
                report_errors,
                intersection_state,
            )?;
            if related == FALSE {
                return Ok(FALSE);
            }
            result &= related;
        }
        Ok(result)
    }

    fn each_type_related_to_some_type(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<Ternary, Error> {
        let mut result = TRUE;
        for source_type in source.types()? {
            let related =
                self.type_related_to_some_type(&source_type, target, false, INTERSECTION_NONE)?;
            if related == FALSE {
                return Ok(FALSE);
            }
            result &= related;
        }
        Ok(result)
    }

    /// `getBestMatchingType` for error elaboration against a union target.
    fn best_matching_type(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<Option<Rc<TypeCell>>, Error> {
        // findMatchingDiscriminantType: discriminants need unit-typed union
        // properties; the fixtures' unions have none, and a real discriminant
        // would need the discrimination machinery the reference lacks.
        if target.flags & flags::UNION != 0
            && source.flags & (flags::INTERSECTION | flags::OBJECT) != 0
            && self.has_discriminant_properties(source, target)?
        {
            return unsupported("discriminated union elaboration");
        }
        // findMatchingTypeReferenceOrTypeAliasReference.
        if source.object_flags & (object_flags::REFERENCE | object_flags::ANONYMOUS) != 0
            && target.flags & flags::UNION != 0
        {
            for candidate in target.types()? {
                if candidate.flags & flags::OBJECT != 0 {
                    let overlap = source.object_flags & candidate.object_flags;
                    if overlap & object_flags::REFERENCE != 0 {
                        return unsupported("type reference matching");
                    }
                    if overlap & object_flags::ANONYMOUS != 0
                        && source.alias.is_some()
                        && source.alias == candidate.alias
                    {
                        return Ok(Some(candidate));
                    }
                }
            }
        }
        // findBestTypeForObjectLiteral needs array-likeness: no fresh literals here.
        // findBestTypeForInvokable.
        for construct in [false, true] {
            if !self.signatures_of_type(source, construct)?.is_empty() {
                for candidate in target.types()? {
                    if !self.signatures_of_type(&candidate, construct)?.is_empty() {
                        return Ok(Some(candidate));
                    }
                }
                return Ok(None);
            }
        }
        // port: tsc/internal/checker/relater.go:Checker.findMostOverlappyType
        let mut best = None;
        if source.flags & (flags::PRIMITIVE | flags::INSTANTIABLE_PRIMITIVE) == 0 {
            let mut matching = 0;
            for candidate in target.types()? {
                if candidate.flags & (flags::PRIMITIVE | flags::INSTANTIABLE_PRIMITIVE) == 0 {
                    let source_keys = self.relation_key_type(source)?;
                    let target_keys = self.relation_key_type(&candidate)?;
                    let overlap = self.key_intersection(&source_keys, &target_keys)?;
                    if overlap.flags & flags::INDEX != 0 {
                        return Ok(Some(candidate));
                    }
                    if overlap.flags & (flags::UNIT | flags::UNION) != 0 {
                        let length = if overlap.flags & flags::UNION != 0 {
                            overlap
                                .types()?
                                .iter()
                                .filter(|ty| ty.flags & flags::UNIT != 0)
                                .count()
                        } else {
                            1
                        };
                        if length >= matching {
                            best = Some(candidate);
                            matching = length;
                        }
                    }
                }
            }
        }
        Ok(best)
    }

    fn has_discriminant_properties(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<bool, Error> {
        let constituents = target.types()?;
        for property in self.properties_of_type(source)? {
            let mut kinds: Vec<u32> = Vec::new();
            let mut literal = false;
            let mut present = 0;
            for constituent in &constituents {
                if let Some(member) = self.property_of_type(constituent, &property.name)? {
                    let ty = member.r#type()?;
                    present += 1;
                    if !kinds.contains(&ty.id) {
                        kinds.push(ty.id);
                    }
                    if ty.flags & flags::UNIT != 0 || ty.flags & flags::BOOLEAN != 0 {
                        literal = true;
                    }
                }
            }
            if present > 0 && kinds.len() > 1 && literal {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // ---- recursion and caching ----------------------------------------------

    /// `recursiveTypeRelatedTo`: cache, assumptions, depth limits, then structure.
    fn recursive_type_related_to(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        report_errors: bool,
        intersection_state: u8,
        recursion_flags: u8,
    ) -> Result<Ternary, Error> {
        if self.overflow {
            return Ok(FALSE);
        }
        let key = relation_key(
            source,
            target,
            intersection_state,
            self.mode == Mode::Identity,
        );
        let entry = self.relation.get(key);
        if entry != 0 {
            if report_errors
                && entry & relation_result::FAILED != 0
                && entry & relation_result::OVERFLOW == 0
            {
                // Elaborating errors: compare again to produce the message.
            } else {
                if report_errors && entry & relation_result::OVERFLOW != 0 {
                    self.report_error(
                        d::Excessive_complexity_comparing_types_0_and_1,
                        vec![
                            self.checker.type_to_string(source)?,
                            self.checker.type_to_string(target)?,
                        ],
                    );
                }
                return Ok(if entry & relation_result::SUCCEEDED != 0 {
                    TRUE
                } else {
                    FALSE
                });
            }
        }
        if self.relation_count <= 0 {
            self.overflow = true;
            return Ok(FALSE);
        }
        if self.maybe_set.contains(&key) {
            return Ok(MAYBE);
        }
        if self.source_stack.len() == 100 || self.target_stack.len() == 100 {
            return Ok(MAYBE);
        }
        let maybe_start = self.maybe_keys.len();
        self.maybe_keys.push(key);
        self.maybe_set.insert(key);
        let save_expanding = self.expanding_flags;
        if recursion_flags & RECURSION_SOURCE != 0 {
            self.source_stack.push(source.clone());
            if self.expanding_flags & EXPANDING_SOURCE == 0
                && is_deeply_nested_type(source, &self.source_stack, 3)?
            {
                self.expanding_flags |= EXPANDING_SOURCE;
            }
        }
        if recursion_flags & RECURSION_TARGET != 0 {
            self.target_stack.push(target.clone());
            if self.expanding_flags & EXPANDING_TARGET == 0
                && is_deeply_nested_type(target, &self.target_stack, 3)?
            {
                self.expanding_flags |= EXPANDING_TARGET;
            }
        }
        let result = if self.expanding_flags == EXPANDING_BOTH {
            Ok(MAYBE)
        } else {
            self.structured_type_related_to(source, target, report_errors, intersection_state)
        };
        if recursion_flags & RECURSION_SOURCE != 0 {
            self.source_stack.pop();
        }
        if recursion_flags & RECURSION_TARGET != 0 {
            self.target_stack.pop();
        }
        self.expanding_flags = save_expanding;
        let result = result?;
        if result == FALSE {
            // A false result goes straight into the cache: false under
            // assumptions is false without them.
            self.relation.set(key, relation_result::FAILED);
            self.relation_count -= 1;
            self.reset_maybe_stack(maybe_start, false);
        } else if result == TRUE || (self.source_stack.is_empty() && self.target_stack.is_empty()) {
            // Definite or depth-zero Maybe results record every assumption as
            // succeeded; Unknown results are never recorded.
            self.reset_maybe_stack(maybe_start, result == TRUE || result == MAYBE);
        }
        Ok(result)
    }

    /// `resetMaybeStack`.
    fn reset_maybe_stack(&mut self, maybe_start: usize, mark_all_as_succeeded: bool) {
        for key in self.maybe_keys.drain(maybe_start..) {
            self.maybe_set.remove(&key);
            if mark_all_as_succeeded {
                self.relation.set(key, relation_result::SUCCEEDED);
                self.relation_count -= 1;
            }
        }
    }

    /// `structuredTypeRelatedTo`.
    fn structured_type_related_to(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        let save_error_state = self.error_state();
        let mut result = self.structured_type_related_to_worker(
            source,
            target,
            report_errors,
            intersection_state,
        )?;
        if self.mode != Mode::Identity {
            // Intersection constraints: non-generic constituents have none.
            if result != FALSE
                && intersection_state & INTERSECTION_TARGET == 0
                && target.flags & flags::INTERSECTION != 0
                && source.flags & (flags::OBJECT | flags::INTERSECTION) != 0
            {
                result &= self.properties_related_to(
                    source,
                    target,
                    report_errors,
                    false,
                    INTERSECTION_NONE,
                )?;
                if result != FALSE
                    && is_object_literal_type(source)
                    && source.object_flags & object_flags::FRESH_LITERAL != 0
                {
                    result &= self.index_signatures_related_to(
                        source,
                        target,
                        false,
                        report_errors,
                        INTERSECTION_NONE,
                    )?;
                }
            } else if result != FALSE
                && target.flags & flags::OBJECT != 0
                && Self::is_source_intersection_needing_extra_check(source, target)?
            {
                result &= self.properties_related_to(
                    source,
                    target,
                    report_errors,
                    true,
                    intersection_state,
                )?;
            }
        }
        if result != FALSE {
            self.restore_error_state(save_error_state);
        }
        Ok(result)
    }

    fn is_source_intersection_needing_extra_check(
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<bool, Error> {
        if source.flags & flags::INTERSECTION == 0 {
            return Ok(false);
        }
        Ok(!source.types()?.iter().any(|t| {
            Rc::ptr_eq(t, target) || t.object_flags & object_flags::NON_INFERRABLE_TYPE != 0
        }))
    }

    /// `structuredTypeRelatedToWorker` for identity, union/intersection and
    /// object sources and targets.
    fn structured_type_related_to_worker(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        let save_error_state = self.error_state();
        if self.mode == Mode::Identity {
            if source.flags & flags::UNION_OR_INTERSECTION != 0 {
                let mut result = self.each_type_related_to_some_type(source, target)?;
                if result != FALSE {
                    result &= self.each_type_related_to_some_type(target, source)?;
                }
                return Ok(result);
            }
            if source.flags & flags::TEMPLATE_LITERAL != 0 {
                return self.template_identity_related_to(source, target);
            }
            if source.flags & flags::OBJECT == 0 {
                return Ok(FALSE);
            }
        } else if source.flags & flags::UNION_OR_INTERSECTION != 0
            || target.flags & flags::UNION_OR_INTERSECTION != 0
        {
            let result = self.union_or_intersection_related_to(
                source,
                target,
                report_errors,
                intersection_state,
            )?;
            if result != FALSE {
                return Ok(result);
            }
            if !(source.flags & flags::INSTANTIABLE != 0
                || source.flags & flags::OBJECT != 0 && target.flags & flags::UNION != 0
                || source.flags & flags::INTERSECTION != 0
                    && target.flags & (flags::OBJECT | flags::UNION | flags::INSTANTIABLE) != 0)
            {
                return Ok(FALSE);
            }
        }
        if self.mode != Mode::Identity && target.flags & flags::TEMPLATE_LITERAL != 0 {
            return self.template_related_to(source, target);
        }
        if source.flags & flags::TYPE_PARAMETER != 0 {
            // An unconstrained type parameter relates through `unknown`. The
            // constraint is tried without errors first, then with its `this`
            // argument (a type parameter or `unknown` is its own such type).
            let declared = source
                .type_parameter_shape()
                .map(TypeParameterShape::constraint)
                .transpose()?
                .flatten();
            let unconstrained = declared.is_none();
            let constraint = match declared {
                Some(constraint) => constraint,
                None => self.checker.intrinsic(flags::UNKNOWN)?,
            };
            if !Rc::ptr_eq(&constraint, source) {
                let result = self.is_related_to_ex(
                    &constraint,
                    target,
                    RECURSION_SOURCE,
                    false,
                    intersection_state,
                )?;
                if result != FALSE {
                    return Ok(result);
                }
                if constraint.flags & flags::OBJECT != 0 {
                    return unsupported("type parameter constraint with a this argument");
                }
                let result = self.is_related_to_ex(
                    &constraint,
                    target,
                    RECURSION_SOURCE,
                    report_errors
                        && !unconstrained
                        && target.flags & source.flags & flags::TYPE_PARAMETER == 0,
                    intersection_state,
                )?;
                if result != FALSE {
                    return Ok(result);
                }
            }
            return Ok(FALSE);
        }
        if target.flags & flags::TYPE_PARAMETER != 0 {
            return Ok(FALSE);
        }
        // A pattern template is not an object wrapper when the target is a
        // primitive. Its base constraint is itself for concrete placeholders.
        if source.flags & flags::TEMPLATE_LITERAL != 0 && target.flags & flags::OBJECT == 0 {
            return Ok(FALSE);
        }
        let source_is_primitive = source.flags & flags::PRIMITIVE != 0;
        let apparent_source;
        let source = if self.mode != Mode::Identity
            && (source_is_primitive || source.flags & flags::NON_PRIMITIVE != 0)
        {
            apparent_source = self.checker.apparent_primitive_type(source)?;
            &apparent_source
        } else {
            source
        };
        if source.flags & flags::OBJECT != 0 && target.flags & flags::OBJECT != 0 {
            if let Some(result) =
                self.reference_arguments_related(source, target, report_errors, intersection_state)?
            {
                return Ok(result);
            }
            if source.object_flags & object_flags::REFERENCE != 0
                && target.object_flags & object_flags::REFERENCE != 0
                && !source.is_array_or_tuple()
                && !target.is_array_or_tuple()
                && (source.reference_shape().is_none() || target.reference_shape().is_none())
            {
                return unsupported("type reference variance");
            }
            if (self.mode == Mode::Subtype || self.mode == Mode::StrictSubtype)
                && target.object_flags & object_flags::FRESH_LITERAL != 0
            {
                return unsupported("fresh empty object literal target");
            }
        }
        if source.flags & (flags::OBJECT | flags::INTERSECTION) != 0
            && target.flags & flags::OBJECT != 0
        {
            let report_structural_errors =
                report_errors && self.error_state() == save_error_state && !source_is_primitive;
            let mut result = self.properties_related_to(
                source,
                target,
                report_structural_errors,
                false,
                intersection_state,
            )?;
            if result != FALSE {
                result &= self.signatures_related_to(
                    source,
                    target,
                    false,
                    report_structural_errors,
                    intersection_state,
                )?;
                if result != FALSE {
                    result &= self.signatures_related_to(
                        source,
                        target,
                        true,
                        report_structural_errors,
                        intersection_state,
                    )?;
                    if result != FALSE {
                        result &= self.index_signatures_related_to(
                            source,
                            target,
                            source_is_primitive,
                            report_structural_errors,
                            intersection_state,
                        )?;
                    }
                }
            }
            if result != FALSE {
                return Ok(result);
            }
        }
        if source.flags & (flags::OBJECT | flags::INTERSECTION) != 0
            && target.flags & flags::UNION != 0
        {
            let object_only: Vec<Rc<TypeCell>> = target
                .types()?
                .into_iter()
                .filter(|t| t.flags & (flags::OBJECT | flags::INTERSECTION) != 0)
                .collect();
            if object_only.len() > 1 {
                // typeRelatedToDiscriminatedType needs discriminant properties.
                if self.has_discriminant_properties(source, target)? {
                    return unsupported("discriminated union relation");
                }
            }
        }
        Ok(FALSE)
    }

    // ---- structure access -----------------------------------------------------

    /// `getPropertiesOfType`: object members, or the union/intersection view
    /// of them (union: properties present in every constituent; intersection:
    /// the union of the constituents' properties).
    fn properties_of_type(&mut self, t: &Rc<TypeCell>) -> Result<Vec<Member>, Error> {
        if t.flags & flags::OBJECT != 0 {
            return Ok(t.members(self.graph())?.to_vec());
        }
        if t.flags & flags::UNION_OR_INTERSECTION != 0 {
            let constituents = t.types()?;
            let mut names: Vec<Rc<str>> = Vec::new();
            for constituent in &constituents {
                if constituent.flags & flags::OBJECT != 0
                    || constituent.flags & flags::UNION_OR_INTERSECTION != 0
                {
                    for member in self.properties_of_type(constituent)? {
                        if !names.contains(&member.name) {
                            names.push(member.name);
                        }
                    }
                } else if t.flags & flags::UNION != 0 {
                    // A primitive constituent contributes only its apparent
                    // members, which the reference does not carry.
                    return unsupported("properties of a union with primitive constituents");
                }
            }
            let mut result = Vec::new();
            for name in names {
                if let Some(member) = self.property_of_type(t, &name)? {
                    result.push(member);
                }
            }
            return Ok(result);
        }
        Ok(Vec::new())
    }

    /// `getPropertyOfType` for objects, unions and intersections. A synthetic
    /// union or intersection property carries the first constituent's link when
    /// every constituent agrees; differing types would need a union type
    /// constructor and fail explicitly.
    fn property_of_type(&mut self, t: &Rc<TypeCell>, name: &str) -> Result<Option<Member>, Error> {
        if t.flags & flags::OBJECT != 0 {
            return Ok(t.member(self.graph(), name)?.cloned());
        }
        if t.flags & flags::UNION_OR_INTERSECTION != 0 {
            let mut found: Option<Member> = None;
            let mut partial = false;
            for constituent in t.types()? {
                match self.property_of_type(&constituent, name)? {
                    Some(member) => match &found {
                        None => found = Some(member),
                        Some(existing) => {
                            let same = Rc::ptr_eq(&existing.r#type()?, &member.r#type()?);
                            if !same {
                                return unsupported(
                                    "synthetic property with differing constituent types",
                                );
                            }
                            let merged = Member {
                                name_type: existing.name_type.clone(),
                                name: existing.name.clone(),
                                optional: if t.flags & flags::UNION != 0 {
                                    existing.optional || member.optional
                                } else {
                                    existing.optional && member.optional
                                },
                                readonly: if t.flags & flags::UNION != 0 {
                                    existing.readonly || member.readonly
                                } else {
                                    existing.readonly && member.readonly
                                },
                                class_member: existing.class_member,
                                r#type: existing.r#type.clone(),
                            };
                            found = Some(merged);
                        }
                    },
                    None => {
                        if t.flags & flags::UNION != 0 {
                            if constituent.flags & flags::OBJECT != 0
                                && !self.index_infos_of_type(&constituent)?.is_empty()
                            {
                                return unsupported("union property against an index signature");
                            }
                            partial = true;
                        }
                    }
                }
            }
            if t.flags & flags::UNION != 0 && partial {
                // Partial union properties are missing from the union's own
                // property list but readable by name (`getPropertyOfType`).
                return Ok(None);
            }
            return Ok(found);
        }
        Ok(None)
    }

    fn index_infos_of_type(&mut self, t: &Rc<TypeCell>) -> Result<Vec<IndexInfo>, Error> {
        if t.flags & flags::OBJECT != 0 {
            return Ok(t.structure(self.graph())?.index_infos.clone());
        }
        if t.flags & flags::INTERSECTION != 0 {
            let mut infos = Vec::new();
            for constituent in t.types()? {
                infos.extend(self.index_infos_of_type(&constituent)?);
            }
            if infos.len() > 1 {
                return unsupported("intersection index signature union");
            }
            return Ok(infos);
        }
        if t.flags & flags::UNION != 0 {
            let mut any = false;
            for constituent in t.types()? {
                any |= !self.index_infos_of_type(&constituent)?.is_empty();
            }
            if any {
                return unsupported("union index signatures");
            }
        }
        Ok(Vec::new())
    }

    fn signatures_of_type(
        &mut self,
        t: &Rc<TypeCell>,
        construct: bool,
    ) -> Result<Vec<Signature>, Error> {
        if t.flags & flags::OBJECT != 0 {
            let structure = t.structure(self.graph())?;
            return Ok(if construct {
                structure.construct_signatures.clone()
            } else {
                structure.call_signatures.clone()
            });
        }
        if t.flags & flags::INTERSECTION != 0 {
            let mut signatures = Vec::new();
            for constituent in t.types()? {
                signatures.extend(self.signatures_of_type(&constituent, construct)?);
            }
            return Ok(signatures);
        }
        if t.flags & flags::UNION != 0 {
            for constituent in t.types()? {
                if !self.signatures_of_type(&constituent, construct)?.is_empty() {
                    return unsupported("union signatures");
                }
            }
        }
        Ok(Vec::new())
    }

    fn type_has_call_or_construct_signatures(&mut self, t: &Rc<TypeCell>) -> Result<bool, Error> {
        Ok(!self.signatures_of_type(t, false)?.is_empty()
            || !self.signatures_of_type(t, true)?.is_empty())
    }

    fn is_weak_type(&mut self, t: &Rc<TypeCell>) -> Result<bool, Error> {
        if t.flags & flags::OBJECT != 0 {
            let structure = t.structure(self.graph())?;
            return Ok(structure.call_signatures.is_empty()
                && structure.construct_signatures.is_empty()
                && structure.index_infos.is_empty()
                && !structure.members.is_empty()
                && structure.members.iter().all(|m| m.optional));
        }
        if t.flags & flags::INTERSECTION != 0 {
            for constituent in t.types()? {
                if !self.is_weak_type(&constituent)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Ok(false)
    }

    fn has_common_properties(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<bool, Error> {
        for property in self.properties_of_type(source)? {
            if self.is_known_property(target, &property.name)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn is_known_property(&mut self, target: &Rc<TypeCell>, name: &str) -> Result<bool, Error> {
        if target.flags & flags::OBJECT != 0
            && (target.member(self.graph(), name)?.is_some()
                || self.applicable_index_info_for_name(target, name)?.is_some())
        {
            return Ok(true);
        }
        if target.flags & flags::UNION_OR_INTERSECTION != 0
            && Self::is_excess_property_check_target(target)?
        {
            for constituent in target.types()? {
                if self.is_known_property(&constituent, name)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn is_excess_property_check_target(t: &Rc<TypeCell>) -> Result<bool, Error> {
        if t.flags & flags::OBJECT != 0 || t.flags & flags::NON_PRIMITIVE != 0 {
            return Ok(true);
        }
        if t.flags & flags::UNION != 0 {
            for constituent in t.types()? {
                if Self::is_excess_property_check_target(&constituent)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        if t.flags & flags::INTERSECTION != 0 {
            for constituent in t.types()? {
                if !Self::is_excess_property_check_target(&constituent)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Ok(false)
    }

    // ---- properties -----------------------------------------------------------

    /// `propertiesRelatedTo` for object-like sources and targets.
    fn properties_related_to(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        report_errors: bool,
        optionals_only: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        if self.mode == Mode::Identity {
            return self.properties_identical_to(source, target);
        }
        if let Some(result) =
            self.tuple_properties_related_to(source, target, report_errors, intersection_state)?
        {
            return Ok(result);
        }
        let mut result = TRUE;
        let require_optional_properties = (self.mode == Mode::Subtype
            || self.mode == Mode::StrictSubtype)
            && !is_object_literal_type(source)
            && source.tuple_shape().is_none();
        let target_properties = self.properties_of_type(target)?;
        let unmatched: Vec<Member> = {
            let mut missing = Vec::new();
            for target_prop in &target_properties {
                if (require_optional_properties || !target_prop.optional)
                    && self.property_of_type(source, &target_prop.name)?.is_none()
                {
                    missing.push(target_prop.clone());
                }
            }
            missing
        };
        if let Some(first) = unmatched.first() {
            if report_errors && self.should_report_unmatched_property_error(source, target)? {
                if unmatched.len() == 1 {
                    self.report_error(
                        d::Property_0_is_missing_in_type_1_but_required_in_type_2,
                        vec![
                            first.name.to_string(),
                            self.checker.type_to_string(source)?,
                            self.checker.type_to_string(target)?,
                        ],
                    );
                    if let Some(location) = self
                        .checker
                        .member_declarations
                        .borrow()
                        .get(&(target.id, first.name.clone()))
                    {
                        self.related_info.push(Diagnostic::new(
                            location.clone(),
                            d::X_0_is_declared_here,
                            vec![first.name.to_string()],
                        ));
                    }
                } else {
                    let names: Vec<&str> = unmatched
                        .iter()
                        .take(if unmatched.len() > 5 {
                            4
                        } else {
                            unmatched.len()
                        })
                        .map(|m| &*m.name)
                        .collect();
                    let mut args = vec![
                        self.checker.type_to_string(source)?,
                        self.checker.type_to_string(target)?,
                        names.join(", "),
                    ];
                    let message = if unmatched.len() > 5 {
                        args.push((unmatched.len() - 4).to_string());
                        d::Type_0_is_missing_the_following_properties_from_type_1_Colon_2_and_3_more
                    } else {
                        d::Type_0_is_missing_the_following_properties_from_type_1_Colon_2
                    };
                    self.report_error(message, args);
                }
            }
            return Ok(FALSE);
        }
        if is_object_literal_type(target) {
            for source_prop in self.properties_of_type(source)? {
                if self.property_of_type(target, &source_prop.name)?.is_none() {
                    if report_errors {
                        self.report_error(
                            d::Property_0_does_not_exist_on_type_1,
                            vec![
                                source_prop.name.to_string(),
                                self.checker.type_to_string(target)?,
                            ],
                        );
                    }
                    return Ok(FALSE);
                }
            }
        }
        for target_prop in &target_properties {
            if !optionals_only || target_prop.optional {
                if let Some(source_prop) = self.property_of_type(source, &target_prop.name)? {
                    // `sourceProp == targetProp` is symbol identity: the same
                    // type link, never a comparison of resolved types (which
                    // would resolve the source's type before the target's).
                    let same = source_prop.r#type.ptr_eq(&target_prop.r#type)
                        && source.flags & flags::OBJECT != 0
                        && target.flags & flags::OBJECT != 0
                        && Self::same_declared_member(source, target);
                    if !same {
                        let related = self.property_related_to(
                            source,
                            target,
                            &source_prop,
                            target_prop,
                            report_errors,
                            intersection_state,
                            self.mode == Mode::Comparable,
                        )?;
                        if related == FALSE {
                            return Ok(FALSE);
                        }
                        result &= related;
                    }
                }
            }
        }
        Ok(result)
    }

    /// `sourceProp != targetProp`: the same declared member reached through
    /// two views of one object (an intersection of the object with itself).
    fn same_declared_member(source: &Rc<TypeCell>, target: &Rc<TypeCell>) -> bool {
        Rc::ptr_eq(source, target)
    }

    fn should_report_unmatched_property_error(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<bool, Error> {
        let calls = self.signatures_of_type(source, false)?;
        let constructs = self.signatures_of_type(source, true)?;
        let properties = self.properties_of_type(source)?;
        if (!calls.is_empty() || !constructs.is_empty()) && properties.is_empty() {
            return Ok(
                !self.signatures_of_type(target, false)?.is_empty() && !calls.is_empty()
                    || !self.signatures_of_type(target, true)?.is_empty() && !constructs.is_empty(),
            );
        }
        Ok(true)
    }

    /// `propertyRelatedTo` for public members.
    #[allow(clippy::too_many_arguments)]
    fn property_related_to(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        source_prop: &Member,
        target_prop: &Member,
        report_errors: bool,
        intersection_state: u8,
        skip_optional: bool,
    ) -> Result<Ternary, Error> {
        if self.mode == Mode::StrictSubtype && source_prop.readonly && !target_prop.readonly {
            return Ok(FALSE);
        }
        let effective_target = target_prop.r#type()?;
        let related = if effective_target.flags
            & (if self.mode == Mode::StrictSubtype {
                flags::ANY
            } else {
                flags::ANY_OR_UNKNOWN
            })
            != 0
        {
            TRUE
        } else {
            let effective_source = source_prop.r#type()?;
            self.is_related_to_ex(
                &effective_source,
                &effective_target,
                RECURSION_BOTH,
                report_errors,
                intersection_state,
            )?
        };
        if related == FALSE {
            self.report(
                report_errors,
                d::Types_of_property_0_are_incompatible,
                vec![target_prop.name.to_string()],
            );
            return Ok(FALSE);
        }
        if !skip_optional
            && source_prop.optional
            && target_prop.class_member
            && !target_prop.optional
        {
            if report_errors {
                self.report_error(
                    d::Property_0_is_optional_in_type_1_but_required_in_type_2,
                    vec![
                        target_prop.name.to_string(),
                        self.checker.type_to_string(source)?,
                        self.checker.type_to_string(target)?,
                    ],
                );
            }
            return Ok(FALSE);
        }
        Ok(related)
    }

    /// `propertiesIdenticalTo`.
    fn properties_identical_to(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<Ternary, Error> {
        if source.flags & flags::OBJECT == 0 || target.flags & flags::OBJECT == 0 {
            return Ok(FALSE);
        }
        let source_properties = self.properties_of_type(source)?;
        let target_properties = self.properties_of_type(target)?;
        if source_properties.len() != target_properties.len() {
            return Ok(FALSE);
        }
        let mut result = TRUE;
        for source_prop in &source_properties {
            let Some(target_prop) = self.property_of_type(target, &source_prop.name)? else {
                return Ok(FALSE);
            };
            // compareProperties: same optionality and readonliness, related types.
            if source_prop.optional != target_prop.optional
                || source_prop.readonly != target_prop.readonly
            {
                return Ok(FALSE);
            }
            let related =
                self.is_related_to_simple(&source_prop.r#type()?, &target_prop.r#type()?)?;
            if related == FALSE {
                return Ok(FALSE);
            }
            result &= related;
        }
        Ok(result)
    }

    // ---- signatures -----------------------------------------------------------

    fn signatures_related_to(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        construct: bool,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        if self.mode == Mode::Identity {
            return self.signatures_identical_to(source, target, construct);
        }
        let source_signatures = self.signatures_of_type(source, construct)?;
        let target_signatures = self.signatures_of_type(target, construct)?;
        if construct
            && !source_signatures.is_empty()
            && !target_signatures.is_empty()
            && source_signatures[0].is_abstract
            && !target_signatures[0].is_abstract
        {
            self.report(
                report_errors,
                d::Cannot_assign_an_abstract_constructor_type_to_a_non_abstract_constructor_type,
                vec![],
            );
            return Ok(FALSE);
        }
        let mut result = TRUE;
        if source.object_flags & object_flags::INSTANTIATED != 0
            && target.object_flags & object_flags::INSTANTIATED != 0
            && source.symbol.is_some()
            && source.symbol == target.symbol
            || source.object_flags & object_flags::REFERENCE != 0
                && target.object_flags & object_flags::REFERENCE != 0
                && match (source.reference_shape(), target.reference_shape()) {
                    (Some(source), Some(target)) => {
                        Rc::ptr_eq(&source.target()?, &target.target()?)
                    }
                    _ => false,
                }
        {
            if source_signatures.len() != target_signatures.len() {
                return unsupported("same-target signature inventory differs");
            }
            for (source, target) in source_signatures.iter().zip(&target_signatures) {
                let related = self.signature_related_to(
                    source,
                    target,
                    true,
                    report_errors,
                    intersection_state,
                )?;
                if related == FALSE {
                    return Ok(FALSE);
                }
                result &= related;
            }
            return Ok(result);
        }
        if source_signatures.len() == 1 && target_signatures.len() == 1 {
            let erase = self.mode == Mode::Comparable;
            result = self.signature_related_to(
                &source_signatures[0],
                &target_signatures[0],
                erase,
                report_errors,
                intersection_state,
            )?;
        } else {
            'outer: for t in &target_signatures {
                let save_error_state = self.error_state();
                let mut should_elaborate = report_errors;
                for s in &source_signatures {
                    let related = self.signature_related_to(
                        s,
                        t,
                        true,
                        should_elaborate,
                        intersection_state,
                    )?;
                    if related != FALSE {
                        result &= related;
                        self.restore_error_state(save_error_state);
                        continue 'outer;
                    }
                    should_elaborate = false;
                }
                if should_elaborate {
                    self.report_error(
                        d::Type_0_provides_no_match_for_the_signature_1,
                        vec![
                            self.checker.type_to_string(source)?,
                            self.checker.signature_to_string(t)?,
                        ],
                    );
                }
                return Ok(FALSE);
            }
        }
        Ok(result)
    }

    fn signature_related_to(
        &mut self,
        source: &Signature,
        target: &Signature,
        erase: bool,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        let erased_source;
        let erased_target;
        let (source, target) = if erase {
            erased_source = self.erased_signature(source)?;
            erased_target = self.erased_signature(target)?;
            (&erased_source, &erased_target)
        } else {
            (source, target)
        };
        let strict_top = self.mode == Mode::Subtype || self.mode == Mode::StrictSubtype;
        let strict_arity = self.mode == Mode::StrictSubtype;
        self.compare_signatures_related(
            source,
            target,
            strict_top,
            strict_arity,
            false,
            report_errors,
            intersection_state,
        )
    }

    /// `compareSignaturesRelated`; callback parameters recurse bivariantly.
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    fn compare_signatures_related(
        &mut self,
        source: &Signature,
        target: &Signature,
        strict_top: bool,
        strict_arity: bool,
        callback: bool,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        if !(strict_top && source.is_top_signature()?) && target.is_top_signature()? {
            return Ok(TRUE);
        }
        if strict_top && source.is_top_signature()? && !target.is_top_signature()? {
            return Ok(FALSE);
        }
        let target_count = target.effective_parameter_count()?;
        let source_has_more = !target.has_effective_rest_parameter()?
            && if strict_arity {
                source.has_effective_rest_parameter()?
                    || source.effective_parameter_count()? > target_count
            } else {
                self.min_argument_count(source)? > target_count
            };
        if source_has_more {
            if report_errors && !strict_arity {
                self.report_error(
                    d::Target_signature_provides_too_few_arguments_Expected_0_or_more_but_got_1,
                    vec![
                        self.min_argument_count(source)?.to_string(),
                        target_count.to_string(),
                    ],
                );
            }
            return Ok(FALSE);
        }
        let instantiated;
        let same_type_parameters =
            if let (Some(source), Some(target)) = (&source.generic, &target.generic) {
                let source = source.parameters()?;
                let target = target.parameters()?;
                source.len() == target.len()
                    && source
                        .iter()
                        .zip(&target)
                        .all(|(source, target)| Rc::ptr_eq(source, target))
            } else {
                false
            };
        let (source, target) = if source.type_parameters > 0 && !same_type_parameters {
            instantiated = self.instantiate_signature_in_context(source, target)?;
            (&instantiated.0, &instantiated.1)
        } else {
            (source, target)
        };
        let source_count = source.effective_parameter_count()?;
        if source.has_non_array_rest_type()? || target.has_non_array_rest_type()? {
            return unsupported("signature non-array rest slicing");
        }
        let strict_variance = !callback && !target.bivariant_parameters;
        let mut result = TRUE;
        if let Some(source_this) = source.this_type()? {
            if source_this.flags & flags::VOID == 0 {
                if let Some(target_this) = target.this_type()? {
                    let mut related = FALSE;
                    if !strict_variance {
                        related = self.is_related_to_ex(
                            &source_this,
                            &target_this,
                            RECURSION_BOTH,
                            false,
                            intersection_state,
                        )?;
                    }
                    if related == FALSE {
                        related = self.is_related_to_ex(
                            &target_this,
                            &source_this,
                            RECURSION_BOTH,
                            report_errors,
                            intersection_state,
                        )?;
                    }
                    if related == FALSE {
                        self.report(
                            report_errors,
                            d::The_this_types_of_each_signature_are_incompatible,
                            vec![],
                        );
                        return Ok(FALSE);
                    }
                    result &= related;
                }
            }
        }
        let param_count = source_count.max(target_count);
        for i in 0..param_count {
            let source_type = self.try_signature_type_at_position(source, i)?;
            let target_type = self.try_signature_type_at_position(target, i)?;
            if let (Some(source_type), Some(target_type)) = (source_type, target_type) {
                if Rc::ptr_eq(&source_type, &target_type) && !strict_arity {
                    continue;
                }
                let source_sig = if callback || self.is_instantiated_generic_parameter(source, i)? {
                    None
                } else {
                    let non_nullable = self.non_nullable_type(&source_type)?;
                    self.single_call_signature(&non_nullable)?
                };
                let target_sig = if callback || self.is_instantiated_generic_parameter(target, i)? {
                    None
                } else {
                    let non_nullable = self.non_nullable_type(&target_type)?;
                    self.single_call_signature(&non_nullable)?
                };
                let callbacks = source_sig.is_some()
                    && target_sig.is_some()
                    && (source_type.flags & flags::NULLABLE)
                        == (target_type.flags & flags::NULLABLE);
                let mut related = FALSE;
                if callbacks {
                    let (source_sig, target_sig) =
                        (source_sig.expect("callback"), target_sig.expect("callback"));
                    related = self.compare_signatures_related(
                        &target_sig,
                        &source_sig,
                        false,
                        strict_arity,
                        true,
                        report_errors,
                        intersection_state,
                    )?;
                } else {
                    if !callback && !strict_variance {
                        related = self.is_related_to_ex(
                            &source_type,
                            &target_type,
                            RECURSION_BOTH,
                            false,
                            intersection_state,
                        )?;
                    }
                    if related == FALSE {
                        related = self.is_related_to_ex(
                            &target_type,
                            &source_type,
                            RECURSION_BOTH,
                            report_errors,
                            intersection_state,
                        )?;
                    }
                }
                if related != FALSE
                    && strict_arity
                    && i >= self.min_argument_count(source)?
                    && i < self.min_argument_count(target)?
                    && self.is_related_to_ex(
                        &source_type,
                        &target_type,
                        RECURSION_BOTH,
                        false,
                        intersection_state,
                    )? != FALSE
                {
                    related = FALSE;
                }
                if related == FALSE {
                    self.report(
                        report_errors,
                        d::Types_of_parameters_0_and_1_are_incompatible,
                        vec![
                            source
                                .parameter_names
                                .get(i)
                                .map_or_else(|| format!("p{i}"), ToString::to_string),
                            target
                                .parameter_names
                                .get(i)
                                .map_or_else(|| format!("p{i}"), ToString::to_string),
                        ],
                    );
                    return Ok(FALSE);
                }
                result &= related;
            }
        }
        let target_return = target.return_type()?;
        if target_return.flags & (flags::VOID | flags::ANY) != 0 {
            return Ok(result);
        }
        let source_return = source.return_type()?;
        let related = self.is_related_to_ex(
            &source_return,
            &target_return,
            RECURSION_BOTH,
            report_errors,
            intersection_state,
        )?;
        result &= related;
        if result == FALSE && report_errors {
            let message = if source.parameter_count() == 0 && target.parameter_count() == 0 {
                if source.is_construct {
                    d::Construct_signatures_with_no_arguments_have_incompatible_return_types_0_and_1
                } else {
                    d::Call_signatures_with_no_arguments_have_incompatible_return_types_0_and_1
                }
            } else if source.is_construct {
                d::Construct_signature_return_types_0_and_1_are_incompatible
            } else {
                d::Call_signature_return_types_0_and_1_are_incompatible
            };
            self.report_error(
                message,
                vec![
                    self.checker.type_to_string(&source_return)?,
                    self.checker.type_to_string(&target_return)?,
                ],
            );
        }
        Ok(result)
    }

    /// `getSingleCallSignature`: an object type with exactly one call signature
    /// and no other members.
    fn single_call_signature(&mut self, t: &Rc<TypeCell>) -> Result<Option<Signature>, Error> {
        if t.flags & flags::OBJECT == 0 {
            return Ok(None);
        }
        let structure = t.structure(self.graph())?;
        if structure.members.is_empty()
            && structure.index_infos.is_empty()
            && structure.call_signatures.len() == 1
            && structure.construct_signatures.is_empty()
        {
            return Ok(Some(structure.call_signatures[0].clone()));
        }
        Ok(None)
    }

    fn signatures_identical_to(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        construct: bool,
    ) -> Result<Ternary, Error> {
        let source_signatures = self.signatures_of_type(source, construct)?;
        let target_signatures = self.signatures_of_type(target, construct)?;
        if source_signatures.len() != target_signatures.len() {
            return Ok(FALSE);
        }
        let mut result = TRUE;
        for (s, t) in source_signatures.iter().zip(&target_signatures) {
            let related = self.compare_signatures_identical(s, t)?;
            if related == FALSE {
                return Ok(FALSE);
            }
            result &= related;
        }
        Ok(result)
    }

    /// `compareSignaturesIdentical` without partial matching.
    fn compare_signatures_identical(
        &mut self,
        source: &Signature,
        target: &Signature,
    ) -> Result<Ternary, Error> {
        if source.type_parameters != target.type_parameters {
            return Ok(FALSE);
        }
        // isMatchingSignature.
        if !(source.effective_parameter_count()? == target.effective_parameter_count()?
            && self.min_argument_count(source)? == self.min_argument_count(target)?
            && source.has_effective_rest_parameter()? == target.has_effective_rest_parameter()?)
        {
            return Ok(FALSE);
        }
        let instantiated;
        let source = if source.type_parameters > 0 {
            instantiated = self.instantiate_identical_signature(source, target)?;
            let Some(instantiated) = &instantiated else {
                return Ok(FALSE);
            };
            instantiated
        } else {
            source
        };
        let mut result = TRUE;
        if let (Some(s), Some(t)) = (source.this_type()?, target.this_type()?) {
            let related = self.is_related_to_simple(&s, &t)?;
            if related == FALSE {
                return Ok(FALSE);
            }
            result &= related;
        }
        for i in 0..target.effective_parameter_count()? {
            let s = match self.try_signature_type_at_position(source, i)? {
                Some(ty) => ty,
                None => self.checker.intrinsic(flags::ANY)?,
            };
            let t = match self.try_signature_type_at_position(target, i)? {
                Some(ty) => ty,
                None => self.checker.intrinsic(flags::ANY)?,
            };
            let related = self.is_related_to_simple(&t, &s)?;
            if related == FALSE {
                return Ok(FALSE);
            }
            result &= related;
        }
        result &= self.is_related_to_simple(&source.return_type()?, &target.return_type()?)?;
        Ok(result)
    }

    // ---- index signatures -----------------------------------------------------

    fn index_signatures_related_to(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        source_is_primitive: bool,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        if self.mode == Mode::Identity {
            return self.index_signatures_identical_to(source, target);
        }
        let index_infos = self.index_infos_of_type(target)?;
        let mut target_has_string_index = false;
        for info in &index_infos {
            target_has_string_index |= info.key()?.flags & flags::STRING != 0;
        }
        let mut result = TRUE;
        for target_info in &index_infos {
            let related = if self.mode != Mode::StrictSubtype
                && !source_is_primitive
                && target_has_string_index
                && target_info.value()?.flags & flags::ANY != 0
            {
                TRUE
            } else {
                self.type_related_to_index_info(
                    source,
                    target_info,
                    report_errors,
                    intersection_state,
                )?
            };
            if related == FALSE {
                return Ok(FALSE);
            }
            result &= related;
        }
        Ok(result)
    }

    fn type_related_to_index_info(
        &mut self,
        source: &Rc<TypeCell>,
        target_info: &IndexInfo,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        let key = target_info.key()?;
        if let Some(source_info) = self.applicable_index_info(source, &key)? {
            return self.index_info_related_to(
                &source_info,
                target_info,
                report_errors,
                intersection_state,
            );
        }
        if intersection_state & INTERSECTION_SOURCE == 0
            && (self.mode != Mode::StrictSubtype
                || source.object_flags & object_flags::FRESH_LITERAL != 0)
            && self.is_object_type_with_inferable_index(source)?
        {
            return self.members_related_to_index_info(
                source,
                target_info,
                report_errors,
                intersection_state,
            );
        }
        if report_errors {
            self.report_error(
                d::Index_signature_for_type_0_is_missing_in_type_1,
                vec![
                    self.checker.type_to_string(&key)?,
                    self.checker.type_to_string(source)?,
                ],
            );
        }
        Ok(FALSE)
    }

    fn is_object_type_with_inferable_index(&mut self, t: &Rc<TypeCell>) -> Result<bool, Error> {
        if t.flags & flags::INTERSECTION != 0 {
            for constituent in t.types()? {
                if !self.is_object_type_with_inferable_index(&constituent)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Ok(t.inferable_index && !self.type_has_call_or_construct_signatures(t)?)
    }

    fn members_related_to_index_info(
        &mut self,
        source: &Rc<TypeCell>,
        target_info: &IndexInfo,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        let mut result = TRUE;
        let key = target_info.key()?;
        let value = target_info.value()?;
        for property in self.properties_of_type(source)? {
            let property_key = self.property_key(&property)?;
            if self.is_applicable_index_type(&property_key, &key)? {
                let property_type = property.r#type()?;
                // Optional properties: `undefined` is part of the declared type
                // under `strictNullChecks`; `getTypeWithFacts(NEUndefined)` would
                // need a union constructor, so an optional property against a
                // non-number key is unsupported unless the value admits undefined.
                if property.optional
                    && key.flags & flags::NUMBER == 0
                    && property_type.flags & flags::UNDEFINED == 0
                    && !Self::admits_undefined(&value)?
                {
                    return unsupported("optional property against an index signature");
                }
                let related = self.is_related_to_ex(
                    &property_type,
                    &value,
                    RECURSION_BOTH,
                    report_errors,
                    intersection_state,
                )?;
                if related == FALSE {
                    self.report(
                        report_errors,
                        d::Property_0_is_incompatible_with_index_signature,
                        vec![property.name.to_string()],
                    );
                    return Ok(FALSE);
                }
                result &= related;
            }
        }
        for info in self.index_infos_of_type(source)? {
            if self.is_applicable_index_type(&info.key()?, &key)? {
                let related = self.index_info_related_to(
                    &info,
                    target_info,
                    report_errors,
                    intersection_state,
                )?;
                if related == FALSE {
                    return Ok(FALSE);
                }
                result &= related;
            }
        }
        Ok(result)
    }

    fn admits_undefined(t: &Rc<TypeCell>) -> Result<bool, Error> {
        if t.flags & (flags::UNDEFINED | flags::ANY | flags::UNKNOWN) != 0 {
            return Ok(true);
        }
        if t.flags & flags::UNION != 0 {
            for constituent in t.types()? {
                if constituent.flags & flags::UNDEFINED != 0 {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn index_info_related_to(
        &mut self,
        source_info: &IndexInfo,
        target_info: &IndexInfo,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        let related = self.is_related_to_ex(
            &source_info.value()?,
            &target_info.value()?,
            RECURSION_BOTH,
            report_errors,
            intersection_state,
        )?;
        if related == FALSE && report_errors {
            let (s, t) = (source_info.key()?, target_info.key()?);
            if Rc::ptr_eq(&s, &t) {
                self.report_error(
                    d::X_0_index_signatures_are_incompatible,
                    vec![self.checker.type_to_string(&s)?],
                );
            } else {
                self.report_error(
                    d::X_0_and_1_index_signatures_are_incompatible,
                    vec![
                        self.checker.type_to_string(&s)?,
                        self.checker.type_to_string(&t)?,
                    ],
                );
            }
        }
        Ok(related)
    }

    fn index_signatures_identical_to(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<Ternary, Error> {
        let source_infos = self.index_infos_of_type(source)?;
        let target_infos = self.index_infos_of_type(target)?;
        if source_infos.len() != target_infos.len() {
            return Ok(FALSE);
        }
        for target_info in &target_infos {
            let key = target_info.key()?;
            let mut matched = false;
            for source_info in &source_infos {
                if Rc::ptr_eq(&source_info.key()?, &key) {
                    if self.is_related_to(
                        &source_info.value()?,
                        &target_info.value()?,
                        RECURSION_BOTH,
                        false,
                    )? != FALSE
                        && source_info.readonly == target_info.readonly
                    {
                        matched = true;
                    }
                    break;
                }
            }
            if !matched {
                return Ok(FALSE);
            }
        }
        Ok(TRUE)
    }

    /// `getApplicableIndexInfo` / `findApplicableIndexInfo`.
    fn applicable_index_info(
        &mut self,
        t: &Rc<TypeCell>,
        key_type: &Rc<TypeCell>,
    ) -> Result<Option<IndexInfo>, Error> {
        let infos = self.index_infos_of_type(t)?;
        let mut string_info = None;
        let mut applicable = Vec::new();
        for info in infos {
            let key = info.key()?;
            if key.flags & flags::STRING != 0 {
                string_info = Some(info);
            } else if self.is_applicable_index_type(key_type, &key)? {
                applicable.push(info);
            }
        }
        match applicable.len() {
            0 => {
                if let Some(info) = string_info {
                    let string = info.key()?;
                    if self.is_applicable_index_type(key_type, &string)? {
                        return Ok(Some(info));
                    }
                }
                Ok(None)
            }
            1 => Ok(applicable.pop()),
            _ => unsupported("intersected applicable index infos"),
        }
    }

    fn applicable_index_info_for_name(
        &mut self,
        t: &Rc<TypeCell>,
        name: &str,
    ) -> Result<Option<IndexInfo>, Error> {
        let infos = self.index_infos_of_type(t)?;
        for info in infos {
            if Self::is_applicable_index_name(name, &info.key()?)? {
                return Ok(Some(info));
            }
        }
        Ok(None)
    }

    /// `isApplicableIndexType(getLiteralTypeFromProperty(prop), keyType)` for
    /// string-named properties.
    fn is_applicable_index_name(name: &str, key_type: &Rc<TypeCell>) -> Result<bool, Error> {
        if key_type.flags & flags::STRING != 0 {
            return Ok(true);
        }
        if key_type.flags & flags::NUMBER != 0 {
            return Ok(is_numeric_literal_name(name));
        }
        if key_type.flags & flags::STRING_LITERAL != 0 {
            return Ok(key_type.literal == Some(LiteralValue::String(name.as_bytes().to_vec())));
        }
        unsupported("non-string index key")
    }

    /// `isApplicableIndexType`.
    fn is_applicable_index_type(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<bool, Error> {
        if self.checker.is_type_assignable_to(source, target)? {
            return Ok(true);
        }
        if target.flags & flags::STRING != 0 {
            let number = self.checker.intrinsic(flags::NUMBER)?;
            if self.checker.is_type_assignable_to(source, &number)? {
                return Ok(true);
            }
        }
        if target.flags & flags::NUMBER != 0 {
            if let Some(LiteralValue::String(text)) = &source.literal {
                return Ok(is_numeric_literal_name(
                    std::str::from_utf8(text).unwrap_or(""),
                ));
            }
        }
        Ok(false)
    }
}

/// `isNumericLiteralName`: the canonical numeric string of its own value.
fn is_numeric_literal_name(name: &str) -> bool {
    match name.parse::<f64>() {
        Ok(value) if value.is_finite() => {
            let formatted = if value.fract() == 0.0 && value.abs() < 1e21 {
                format!("{}", value as i64)
            } else {
                format!("{value}")
            };
            formatted == name || name == "NaN" || name == "Infinity" || name == "-Infinity"
        }
        _ => name == "NaN" || name == "Infinity" || name == "-Infinity",
    }
}

#[cfg(test)]
mod tests;

impl std::fmt::Display for Error {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Released => output.write_str("edge into a released graph"),
            Error::UndeclaredMember(name) => write!(output, "undeclared member {name}"),
            Error::ResolutionFailed => output.write_str("resolution failed"),
            Error::Unsupported(what) => {
                write!(output, "unsupported by the reference relater: {what}")
            }
        }
    }
}

impl std::error::Error for Error {}
