//! File-owned binding symbols. Graph links and slice/table identities do not
//! retain storage; their enclosing bind result supplies checked resolution.

use crate::{
    modifier_flags, symbol_flags, AstView, JsString, NodeAccess, NodeDataRead, NodeId, NodeText,
};
use std::{
    borrow::Cow,
    sync::atomic::{AtomicU64, Ordering},
};
#[cfg(test)]
use ts_arena::Counters;
use ts_arena::Error;
pub use ts_arena::SymbolId;

pub type SymbolFlags = u32;
pub type CheckFlags = u32;

/// port: tsc/internal/ast/utilities.go:IsNonLocalAlias
pub fn is_non_local_alias(symbol: Option<&dyn crate::SymbolAccess>, excludes: SymbolFlags) -> bool {
    symbol.is_some_and(|symbol| {
        symbol.flags() & (symbol_flags::ALIAS | excludes) == symbol_flags::ALIAS
            || symbol.flags() & symbol_flags::ALIAS != 0
                && symbol.flags() & symbol_flags::ASSIGNMENT != 0
    })
}

/// port: tsc/internal/ast/utilities.go:IsAliasSymbolDeclaration
pub fn is_alias_symbol_declaration(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    use crate::SyntaxKind as K;
    let node = view.node(id)?;
    match node.kind().known() {
        Some(
            K::ImportEqualsDeclaration
            | K::NamespaceExportDeclaration
            | K::NamespaceImport
            | K::NamespaceExport
            | K::ImportSpecifier
            | K::ExportSpecifier,
        ) => Ok(true),
        Some(K::ImportClause) => Ok(node
            .data_source()
            .as_import_clause()
            .expect("ImportClause payload")
            .name()
            .is_some()),
        Some(K::ExportAssignment) => {
            crate::expression_is_alias(view, node.expression().expect("nil alias expression"))
        }
        Some(K::VariableDeclaration | K::BindingElement) => {
            crate::is_variable_declaration_initialized_to_require(view, id)
        }
        Some(K::BinaryExpression) => {
            if matches!(
                crate::get_assignment_declaration_kind(view, id)?,
                crate::JSDeclarationKind::ModuleExports | crate::JSDeclarationKind::ExportsProperty
            ) {
                crate::expression_is_alias(
                    view,
                    node.data_source()
                        .as_binary_expression()
                        .expect("BinaryExpression payload")
                        .right()
                        .expect("nil alias expression"),
                )
            } else {
                Ok(false)
            }
        }
        _ => Ok(false),
    }
}

#[derive(Debug, Default)]
pub struct Symbol {
    pub flags: SymbolFlags,
    pub check_flags: CheckFlags,
    pub name: JsString,
    pub declarations: DeclarationSlice,
    pub value_declaration: Option<NodeId>,
    pub members: Option<SymbolTableId>,
    pub exports: Option<SymbolTableId>,
    pub parent: Option<SymbolId>,
    pub export_symbol: Option<SymbolId>,
    pub(crate) runtime_id: AtomicU64,
}
impl Symbol {
    pub fn new(flags: SymbolFlags, name: JsString) -> Self {
        Self {
            flags,
            name,
            ..Self::default()
        }
    }
    // port: tsc/internal/ast/symbol.go:Symbol.IsExternalModule
    pub fn is_external_module(&self) -> bool {
        self.flags & symbol_flags::MODULE != 0
            && is_ambient_module_symbol_name(self.name.as_bytes())
    }
    // port: tsc/internal/ast/symbol.go:Symbol.IsStatic
    pub fn is_static(&self, view: AstView<'_>) -> Result<bool, Error> {
        self.value_declaration.map_or(Ok(false), |node| {
            Ok(view.node(node)?.modifier_flags(view)? & modifier_flags::STATIC != 0)
        })
    }
    // port: tsc/internal/ast/symbol.go:Symbol.CombinedLocalAndExportSymbolFlags
    pub fn combined_local_and_export_symbol_flags(
        &self,
        resolve_flags: impl FnOnce(SymbolId) -> Result<SymbolFlags, Error>,
    ) -> Result<SymbolFlags, Error> {
        self.export_symbol
            .map_or(Ok(self.flags), |id| Ok(self.flags | resolve_flags(id)?))
    }
}

static NEXT_SYMBOL_ID: AtomicU64 = AtomicU64::new(0);
/// Observe a previously assigned source identity without assigning one.
pub fn existing_runtime_symbol_id(symbol: &(impl crate::SymbolAccess + ?Sized)) -> u64 {
    symbol.observe_runtime_identity()
}
/// The source's comparison/wire identity is distinct from checked storage IDs.
// port: tsc/internal/ast/utilities.go:GetSymbolId
pub fn runtime_symbol_id(symbol: &(impl crate::SymbolAccess + ?Sized)) -> u64 {
    symbol.assign_runtime_identity()
}
pub(crate) fn observe_runtime_cell(cell: &AtomicU64) -> u64 {
    cell.load(Ordering::SeqCst)
}
pub(crate) fn assign_runtime_cell(cell: &AtomicU64) -> u64 {
    let mut id = cell.load(Ordering::SeqCst);
    if id == 0 {
        id = NEXT_SYMBOL_ID
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);
        if cell
            .compare_exchange(0, id, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            id = cell.load(Ordering::SeqCst);
        }
    }
    id
}

pub const INTERNAL_SYMBOL_NAME_PREFIX: &[u8] = b"\xfe";
pub mod internal_symbol_names {
    pub const CALL: &[u8] = b"\xfecall";
    pub const CONSTRUCTOR: &[u8] = b"\xfeconstructor";
    pub const NEW: &[u8] = b"\xfenew";
    pub const INDEX: &[u8] = b"\xfeindex";
    pub const EXPORT_STAR: &[u8] = b"\xfeexport";
    pub const GLOBAL: &[u8] = b"\xfeglobal";
    pub const MISSING: &[u8] = b"\xfemissing";
    pub const TYPE: &[u8] = b"\xfetype";
    pub const OBJECT: &[u8] = b"\xfeobject";
    pub const JSX_ATTRIBUTES: &[u8] = b"\xfejsxAttributes";
    pub const CLASS: &[u8] = b"\xfeclass";
    pub const FUNCTION: &[u8] = b"\xfefunction";
    pub const COMPUTED: &[u8] = b"\xfecomputed";
    pub const ASSIGNMENT_DECLARATION: &[u8] = b"\xfeassignment";
    pub const INSTANTIATION_EXPRESSION: &[u8] = b"\xfeinstantiationExpression";
    pub const IMPORT_ATTRIBUTES: &[u8] = b"\xfeimportAttributes";
    pub const EXPORT_EQUALS: &[u8] = b"export=";
    pub const DEFAULT: &[u8] = b"default";
    pub const THIS: &[u8] = b"this";
    pub const MODULE_EXPORTS: &[u8] = b"module.exports";
}

pub enum SymbolName<'a> {
    Stored(&'a [u8]),
    Private(NodeText<'a>),
}
impl SymbolName<'_> {
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Stored(name) => name,
            Self::Private(name) => name.as_bytes(),
        }
    }
}
impl std::ops::Deref for SymbolName<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_bytes()
    }
}
// port: tsc/internal/ast/symbol.go:SymbolName
pub fn symbol_name<'a>(
    symbol: &'a (impl crate::SymbolAccess + ?Sized),
    view: AstView<'a>,
) -> Result<SymbolName<'a>, Error> {
    if let Some(declaration) = symbol.value_declaration() {
        if crate::utilities::is_private_identifier_class_element_declaration(view, declaration)? {
            let name = view
                .node(declaration)?
                .name()
                .expect("private declaration has a name");
            return Ok(SymbolName::Private(view.node_text(name)?));
        }
    }
    Ok(SymbolName::Stored(symbol.name_bytes()))
}
// port: tsc/internal/ast/symbol.go:EscapeAllInternalSymbolNames
pub fn escape_all_internal_symbol_names(name: &[u8]) -> Cow<'_, [u8]> {
    if !name.contains(&0xfe) {
        return Cow::Borrowed(name);
    }
    let mut result = Vec::with_capacity(name.len());
    for &byte in name {
        if byte == 0xfe {
            result.extend_from_slice(b"__");
        } else {
            result.push(byte);
        }
    }
    Cow::Owned(result)
}
// port: tsc/internal/ast/symbol.go:EscapeInternalSymbolName
pub fn escape_internal_symbol_name(name: &[u8]) -> Cow<'_, [u8]> {
    name.strip_prefix(INTERNAL_SYMBOL_NAME_PREFIX)
        .map_or(Cow::Borrowed(name), |rest| {
            Cow::Owned([b"__".as_slice(), rest].concat())
        })
}
// port: tsc/internal/ast/symbol.go:EscapeSymbolName
pub fn escape_symbol_name(name: &[u8]) -> Cow<'_, [u8]> {
    if name.starts_with(INTERNAL_SYMBOL_NAME_PREFIX) {
        return escape_internal_symbol_name(name);
    }
    if name.starts_with(b"__") {
        return Cow::Owned([b"_".as_slice(), name].concat());
    }
    Cow::Borrowed(name)
}
// port: tsc/internal/ast/utilities.go:TryGetAmbientModuleNameFromSymbolName
pub fn try_get_ambient_module_name_from_symbol_name(name: &[u8]) -> Option<&[u8]> {
    if name.starts_with(b"\"") && name.ends_with(b"\"") {
        // The one-byte quote input preserves Go's source bounds panic.
        return Some(&name[1..name.len() - 1]);
    }
    let rest = name.strip_prefix(b"\xfe\"")?;
    let index = rest
        .windows(b"\"pattern@".len())
        .rposition(|part| part == b"\"pattern@")?;
    (index >= 1).then_some(&rest[..index])
}
// port: tsc/internal/ast/utilities.go:IsAmbientModuleSymbolName
pub fn is_ambient_module_symbol_name(name: &[u8]) -> bool {
    try_get_ambient_module_name_from_symbol_name(name).is_some()
}

pub use crate::symbol_tables::{
    SymbolTable, SymbolTableId, SymbolTableMut, SymbolTableRead, SymbolTables,
};
// port: tsc/internal/ast/utilities.go:GetSymbolTable
pub fn get_symbol_table<'a>(
    tables: &'a mut SymbolTables,
    id: &mut Option<SymbolTableId>,
) -> Result<SymbolTableMut<'a>, Error> {
    tables.get_or_create(id)
}
// port: tsc/internal/ast/utilities.go:GetMembers
pub fn get_members<'a>(
    symbol: &mut Symbol,
    tables: &'a mut SymbolTables,
) -> Result<SymbolTableMut<'a>, Error> {
    get_symbol_table(tables, &mut symbol.members)
}
// port: tsc/internal/ast/utilities.go:GetExports
pub fn get_exports<'a>(
    symbol: &mut Symbol,
    tables: &'a mut SymbolTables,
) -> Result<SymbolTableMut<'a>, Error> {
    get_symbol_table(tables, &mut symbol.exports)
}

#[path = "declaration_lists.rs"]
mod declaration_lists;
pub use declaration_lists::{DeclarationLists, DeclarationRead, DeclarationSlice};

macro_rules! locals_shape {
    ($node:expr; $($variant:ident),*) => {
        matches!($node.data(), $(NodeDataRead::$variant(_))|*)
    };
}

/// The source tests the payload's promoted LocalsContainerData method, even for
/// an open kind/payload mismatch. Token(SourceFile) therefore has no locals.
// port: tsc/internal/ast/ast.go:IsLocalsContainer
pub fn is_locals_container(node: &(impl NodeAccess + ?Sized)) -> bool {
    crate::node_semantics::locals_container_shapes!(locals_shape, node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AstBuilder, Factory, FactoryMethods, Node, RuntimeFactory, SyntaxKind};
    use ts_arena::SymbolArena;
    use ts_jsstring::SourceText;

    #[test]
    fn alias_queries_keep_nil_symbols_local_merges_and_declaration_expression_rules() {
        use symbol_flags as f;
        assert!(!is_non_local_alias(None, f::ALL));
        for (flags, excludes, expected) in [
            (f::ALIAS, f::VALUE, true),
            (f::ALIAS | f::FUNCTION_SCOPED_VARIABLE, f::VALUE, false),
            (
                f::ALIAS | f::FUNCTION_SCOPED_VARIABLE | f::ASSIGNMENT,
                f::VALUE,
                true,
            ),
            (f::ASSIGNMENT, f::VALUE, false),
        ] {
            assert_eq!(
                is_non_local_alias(Some(&Symbol::new(flags, JsString::default())), excludes),
                expected
            );
        }
        let mut builder = AstBuilder::new(
            SourceText::from_loaded_bytes(b"".as_slice()),
            &Counters::new(),
        );
        let name = builder.new_identifier(JsString::from_bytes(b"\xffalias".as_slice()));
        let named = builder.new_import_clause(SyntaxKind::Unknown.into(), Some(name), None);
        let unnamed = builder.new_import_clause(SyntaxKind::Unknown.into(), None, None);
        let alias = builder.new_export_assignment(None, false, None, Some(name));
        let literal = builder.new_token(SyntaxKind::TrueKeyword.into());
        let value = builder.new_export_assignment(None, false, None, Some(literal));
        let class = builder.new_class_expression(None, None, None, None, None);
        let class_alias = builder.new_export_assignment(None, false, None, Some(class));
        for (node, expected) in [
            (named, true),
            (unnamed, false),
            (alias, true),
            (value, false),
            (class_alias, true),
        ] {
            assert_eq!(
                is_alias_symbol_declaration(builder.view(), node).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn symbol_name_escaping_matches_pinned_go_bytes() {
        // Observed through the pinned ast functions with Go 1.27.1, including
        // invalid UTF-8 adjacent to the internal FE sentinel.
        type EscapeCase<'a> = (&'a [u8], &'a [u8], &'a [u8], &'a [u8]);
        let cases: &[EscapeCase<'_>] = &[
            (b"", b"", b"", b""),
            (b"plain", b"plain", b"plain", b"plain"),
            (b"__", b"__", b"__", b"___"),
            (b"___user", b"___user", b"___user", b"____user"),
            (
                b"\xff\xfe\xfe",
                b"\xff____",
                b"\xff\xfe\xfe",
                b"\xff\xfe\xfe",
            ),
            (b"\xfetype", b"__type", b"__type", b"__type"),
            (b"x\xfe\xff", b"x__\xff", b"x\xfe\xff", b"x\xfe\xff"),
        ];
        for &(name, all, internal, escaped) in cases {
            assert_eq!(&*escape_all_internal_symbol_names(name), all);
            assert_eq!(&*escape_internal_symbol_name(name), internal);
            assert_eq!(&*escape_symbol_name(name), escaped);
        }
        assert!(matches!(escape_symbol_name(b"ordinary"), Cow::Borrowed(_)));
        assert!(matches!(
            escape_internal_symbol_name(b"x\xfe"),
            Cow::Borrowed(_)
        ));
        assert_eq!(internal_symbol_names::JSX_ATTRIBUTES, b"\xfejsxAttributes");
    }

    #[test]
    fn ambient_symbol_names_preserve_empty_and_pattern_markers_and_panic_order() {
        let cases: &[(&[u8], Option<&[u8]>)] = &[
            (b"", None),
            (b"\"\"", Some(b"")),
            (b"\"x\"", Some(b"x")),
            (b"\xfe\"x\"pattern@1", Some(b"x")),
            (b"\xfe\"\"pattern@1", None),
            (b"\xfe\"a\"pattern@b\"pattern@2", Some(b"a\"pattern@b")),
            (b"\"\xff\"", Some(b"\xff")),
        ];
        for &(name, expected) in cases {
            assert_eq!(try_get_ambient_module_name_from_symbol_name(name), expected);
            let symbol = Symbol::new(symbol_flags::VALUE_MODULE, JsString::from_bytes(name));
            assert_eq!(symbol.is_external_module(), expected.is_some());
        }
        let mut quote = Symbol::new(symbol_flags::NONE, JsString::from_bytes(b"\"".as_slice()));
        assert!(!quote.is_external_module());
        quote.flags = symbol_flags::VALUE_MODULE;
        assert!(std::panic::catch_unwind(|| quote.is_external_module()).is_err());
    }

    #[test]
    fn symbol_queries_follow_private_declarations_and_table_links() {
        let counters = Counters::new();
        let mut ast = AstBuilder::new(SourceText::default(), &counters);
        let name = ast.new_private_identifier(JsString::from_bytes(b"#\xff".as_slice()));
        let modifier = ast.new_modifier(SyntaxKind::StaticKeyword.into());
        let modifiers = ast.node_slice(vec![Some(modifier)]).unwrap();
        let modifiers = ast.new_modifier_list(modifiers);
        let property = ast.new_property_declaration(Some(modifiers), Some(name), None, None, None);
        let mut symbol = Symbol::new(
            symbol_flags::PROPERTY,
            JsString::from_bytes(b"internal".as_slice()),
        );
        assert_eq!(
            symbol_name(&symbol, ast.view()).unwrap().as_bytes(),
            b"internal"
        );
        assert!(!symbol.is_static(ast.view()).unwrap());
        symbol.value_declaration = Some(property);
        assert_eq!(
            symbol_name(&symbol, ast.view()).unwrap().as_bytes(),
            b"#\xff"
        );
        assert!(symbol.is_static(ast.view()).unwrap());
        assert_eq!(
            symbol
                .combined_local_and_export_symbol_flags(|_| panic!("no export"))
                .unwrap(),
            symbol_flags::PROPERTY
        );
        let mut symbols = SymbolArena::new(&counters);
        let exported = symbols.push(Symbol::new(symbol_flags::FUNCTION, JsString::default()));
        symbol.export_symbol = Some(exported);
        assert_eq!(
            symbol
                .combined_local_and_export_symbol_flags(|id| Ok(symbols.get(id)?.flags))
                .unwrap(),
            symbol_flags::PROPERTY | symbol_flags::FUNCTION
        );
        let mut tables = SymbolTables::new(&counters);
        assert!(symbol.members.is_none() && symbol.exports.is_none());
        get_members(&mut symbol, &mut tables)
            .unwrap()
            .insert(JsString::from_bytes(b"\xff".as_slice()), None);
        let members = symbol.members.unwrap();
        assert_eq!(
            get_members(&mut symbol, &mut tables).unwrap().get(b"\xff"),
            Some(None)
        );
        assert_eq!(symbol.members, Some(members));
        assert!(get_exports(&mut symbol, &mut tables).unwrap().is_empty());
        assert_ne!(symbol.members, symbol.exports);
        let foreign = SymbolTables::new(&counters);
        assert!(matches!(foreign.get(members), Err(Error::WrongOwner)));
        assert_eq!(symbol_flags::ALL, 1_073_741_823);
        assert_eq!(symbol_flags::VALUE, 111_551);
        assert_eq!(symbol_flags::TYPE, 788_968);
        assert_eq!(symbol_flags::CLASS_EXCLUDES, 899_503);
        assert_eq!(
            symbol_flags::EXPORT_DOES_NOT_SUPPORT_DEFAULT_MODIFIER,
            4_294_967_183
        );
    }

    #[test]
    fn symbol_runtime_ids_are_shared_under_contention_and_not_storage_capabilities() {
        let symbol = Symbol::default();
        let ids = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| runtime_symbol_id(&symbol)))
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert!(ids[0] != 0 && ids.iter().all(|&id| id == ids[0]));
        assert_ne!(runtime_symbol_id(&Symbol::default()), ids[0]);
    }

    #[test]
    fn declaration_headers_preserve_nil_aliasing_append_and_unique_order() {
        let counters = Counters::new();
        let mut ast = AstBuilder::new(SourceText::default(), &counters);
        let first = ast.new_token(SyntaxKind::ThisKeyword.into());
        let second = ast.new_token(SyntaxKind::TrueKeyword.into());
        let third = ast.new_token(SyntaxKind::FalseKeyword.into());
        let mut lists = DeclarationLists::new(&counters);
        let nil = DeclarationSlice::empty();
        let empty = lists.alloc(Vec::new()).unwrap();
        assert!(nil.is_nil() && !empty.is_nil() && nil.same(empty));
        let original = lists
            .alloc_with_capacity(vec![Some(first), None], 3)
            .unwrap();
        let copied = original;
        lists.set(original, 0, Some(second)).unwrap();
        assert_eq!(lists.get(copied).unwrap().to_vec(), &[Some(second), None]);
        let extended = lists.append(original, Some(third)).unwrap();
        assert_eq!(
            lists.get(copied.slice(0..3).unwrap()).unwrap().to_vec(),
            &[Some(second), None, Some(third)]
        );
        assert_eq!(copied.len(), 2);
        let detached = lists.append(extended, Some(first)).unwrap();
        lists.set(detached, 0, Some(first)).unwrap();
        assert_eq!(lists.get(copied).unwrap().at(0), Some(second));
        assert_eq!(lists.append_if_unique(detached, None).unwrap(), detached);
        let clamped = detached.slice_with_capacity(0..1, 1).unwrap();
        let after = lists.append(clamped, Some(second)).unwrap();
        assert!(!after.slice(0..1).unwrap().same(clamped));
        let foreign = DeclarationLists::new(&counters);
        assert!(matches!(foreign.get(original), Err(Error::WrongOwner)));
        assert!(original.slice(0..4).is_err());
    }

    #[test]
    fn declaration_heap_growth_matches_pinned_go_pointer_slice_observations() {
        // Go 1.27.1, append([]*ast.Node, nil) through a non-inlined function whose
        // result escapes. Stack-only compiler capacity optimizations are separate.
        let expected = [
            (1, 1),
            (2, 2),
            (3, 4),
            (5, 8),
            (9, 16),
            (17, 32),
            (33, 64),
            (65, 143),
            (144, 287),
            (288, 607),
            (608, 1023),
            (1024, 1535),
            (1536, 2303),
        ];
        let mut lists = DeclarationLists::new(&Counters::new());
        let mut slice = DeclarationSlice::empty();
        let mut observed = Vec::new();
        for _ in 0..2000 {
            let old = slice.capacity();
            slice = lists.append(slice, None).unwrap();
            if slice.capacity() != old {
                observed.push((slice.len(), slice.capacity()));
            }
        }
        assert_eq!(observed, expected);
    }

    #[test]
    fn locals_container_support_follows_payload_instead_of_open_kind() {
        let token =
            Node::from_factory_parts(SyntaxKind::SourceFile.into(), crate::TokenData {}.into());
        assert!(!is_locals_container(&token));
        let block = Node::from_factory_parts(
            SyntaxKind::Unknown.into(),
            crate::BlockData {
                statements: None,
                multi_line: false,
            }
            .into(),
        );
        assert!(is_locals_container(&block));
        let mut ast = AstBuilder::new(SourceText::default(), &Counters::new());
        let function_type = ast.new_function_type_node(None, None, None);
        assert!(is_locals_container(
            &ast.view().node(function_type).unwrap()
        ));
        let property = ast.new_property_declaration(None, None, None, None, None);
        assert!(!is_locals_container(&ast.view().node(property).unwrap()));
    }
}
