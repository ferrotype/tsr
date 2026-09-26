//! The type and symbol comparators (`CompareTypes` and its helpers in
//! `tsc/internal/checker/utilities.go`, `compareSymbolsWorker` and
//! `compareNodes` in `checker.go`). Union constituent order, and therefore
//! every `.types` baseline that prints a union, depends on them (ADR 0010).
//!
//! Comparisons that need declaration positions (`compareNodes`), type mappers or
//! type kinds not stored yet are named failures, never a guessed order.

use crate::{object_flags, type_flags, CheckerState, Error, LiteralValue, TypeId, TypeKind};
use std::cmp::Ordering;
use tsr_arena::{NodeId, SymbolId};

fn ordering(value: i64) -> Ordering {
    value.cmp(&0)
}

impl CheckerState {
    /// Sorts types with the ported comparator; the first comparator error wins.
    pub(crate) fn sort_types(&self, types: &mut [TypeId]) -> Result<(), Error> {
        #[cfg(feature = "creation-trace")]
        self.trace_type_sort("sort_begin", types);
        let mut failure = None;
        types.sort_by(|a, b| match self.compare_types(*a, *b) {
            Ok(order) => order,
            Err(error) => {
                if failure.is_none() {
                    failure = Some(error);
                }
                Ordering::Equal
            }
        });
        #[cfg(feature = "creation-trace")]
        self.trace_type_sort("sort_end", types);
        failure.map_or(Ok(()), Err)
    }

    // port: tsc/internal/checker/utilities.go:Checker.sortSymbols
    pub(crate) fn sort_symbols(&self, symbols: &mut [SymbolId]) -> Result<(), Error> {
        #[cfg(feature = "creation-trace")]
        self.trace_symbol_sort("sort_begin", symbols);
        // `compareSymbols` orders by the first declaration's file and position
        // before names and ids. Each symbol's key is computed once; only equal
        // keys fall back to the full comparison, which decides by name and id.
        let mut keyed = Vec::with_capacity(symbols.len());
        for &symbol in symbols.iter() {
            let read = self.symbol(symbol)?;
            let key = if read.declarations().is_empty() {
                (1u8, 0, 0)
            } else {
                let first = self.declaration_slice(read.declarations())?.at(0);
                match first {
                    Some(first) => (0u8, self.node_file_index(first)?, self.node(first)?.pos()),
                    None => (1u8, 0, 0),
                }
            };
            keyed.push((key, symbol));
        }
        let mut failure = None;
        keyed.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| match self.compare_symbols(Some(a.1), Some(b.1)) {
                    Ok(order) => order,
                    Err(error) => {
                        if failure.is_none() {
                            failure = Some(error);
                        }
                        Ordering::Equal
                    }
                })
        });
        for (slot, (_, symbol)) in symbols.iter_mut().zip(keyed) {
            *slot = symbol;
        }
        #[cfg(feature = "creation-trace")]
        self.trace_symbol_sort("sort_end", symbols);
        failure.map_or(Ok(()), Err)
    }

    // port: tsc/internal/checker/utilities.go:CompareTypes
    pub(crate) fn compare_types(&self, t1: TypeId, t2: TypeId) -> Result<Ordering, Error> {
        if t1 == t2 {
            return Ok(Ordering::Equal);
        }
        let r1 = *self.types.get(t1)?;
        let r2 = *self.types.get(t2)?;
        // First sort in order of increasing type flags values.
        let c = self.sort_order_flags(t1)? - self.sort_order_flags(t2)?;
        if c != 0 {
            return Ok(ordering(c));
        }
        // Order named types by name and, in the case of aliased types, by alias type arguments.
        let c = self.compare_type_names(t1, t2)?;
        if c != Ordering::Equal {
            return Ok(c);
        }
        // We have unnamed types or types with identical names. Now sort by data specific to the type.
        let flags = r1.flags;
        if flags
            & (type_flags::ANY
                | type_flags::UNKNOWN
                | type_flags::STRING
                | type_flags::NUMBER
                | type_flags::BOOLEAN
                | type_flags::BIG_INT
                | type_flags::ES_SYMBOL
                | type_flags::VOID
                | type_flags::UNDEFINED
                | type_flags::NULL
                | type_flags::NEVER
                | type_flags::NON_PRIMITIVE)
            != 0
        {
            // Only distinguished by type IDs, handled below.
        } else if flags & type_flags::OBJECT != 0 {
            if r1.object_flags & object_flags::INSTANTIATION_EXPRESSION_TYPE != 0
                && r2.object_flags & object_flags::INSTANTIATION_EXPRESSION_TYPE != 0
            {
                let first_declaration = |symbol| -> Result<Option<NodeId>, Error> {
                    match symbol {
                        Some(symbol) => {
                            Ok(self.symbol_declarations(symbol)?.iter().next().flatten())
                        }
                        None => Ok(None),
                    }
                };
                let order = self
                    .compare_nodes(first_declaration(r1.symbol)?, first_declaration(r2.symbol)?)?;
                if order != Ordering::Equal {
                    return Ok(order);
                }
                let order = self.compare_nodes(
                    self.types.instantiation_expression(t1)?.node,
                    self.types.instantiation_expression(t2)?.node,
                )?;
                if order != Ordering::Equal {
                    return Ok(order);
                }
            } else {
                let order = self.compare_symbols(r1.symbol, r2.symbol)?;
                if order != Ordering::Equal {
                    return Ok(order);
                }
            }
            // When object types have the same or no symbol, order by kind. We order type references before other kinds.
            let ref1 = r1.object_flags & object_flags::REFERENCE != 0;
            let ref2 = r2.object_flags & object_flags::REFERENCE != 0;
            if ref1 && ref2 {
                let target1 = self.types.target(t1)?;
                let target2 = self.types.target(t2)?;
                if self.types.object_flags(target1)? & object_flags::TUPLE != 0
                    && self.types.object_flags(target2)? & object_flags::TUPLE != 0
                {
                    // Tuple types have no associated symbol, instead we order by tuple element information.
                    let c = self.compare_tuple_types(target1, target2)?;
                    if c != Ordering::Equal {
                        return Ok(c);
                    }
                }
                // Here we know we have references to instantiations of the same type because we have matching targets.
                let (node1, node2) = (
                    self.types.type_reference(t1)?.node,
                    self.types.type_reference(t2)?.node,
                );
                if node1.is_none() && node2.is_none() {
                    // Non-deferred type references with the same target are sorted by their type argument lists.
                    let args1 = self
                        .types
                        .type_reference(t1)?
                        .resolved_type_arguments
                        .clone();
                    let args2 = self
                        .types
                        .type_reference(t2)?
                        .resolved_type_arguments
                        .clone();
                    let c = self.compare_type_lists(
                        args1.as_deref().unwrap_or(&[]),
                        args2.as_deref().unwrap_or(&[]),
                    )?;
                    if c != Ordering::Equal {
                        return Ok(c);
                    }
                } else {
                    // Deferred type references are ordered by source location, then by type mapper.
                    let c = self.compare_nodes(node1, node2)?;
                    if c != Ordering::Equal {
                        return Ok(c);
                    }
                    let c = self.compare_type_mappers(
                        self.types.object(t1)?.mapper,
                        self.types.object(t2)?.mapper,
                    )?;
                    if c != Ordering::Equal {
                        return Ok(c);
                    }
                }
            } else if ref1 {
                return Ok(Ordering::Less);
            } else if ref2 {
                return Ok(Ordering::Greater);
            } else {
                // Order unnamed non-reference object types by kind and associated type mappers.
                let c = i64::from(r1.object_flags & object_flags::OBJECT_TYPE_KIND_MASK)
                    - i64::from(r2.object_flags & object_flags::OBJECT_TYPE_KIND_MASK);
                if c != 0 {
                    return Ok(ordering(c));
                }
                let c = self.compare_type_mappers(
                    self.types.object(t1)?.mapper,
                    self.types.object(t2)?.mapper,
                )?;
                if c != Ordering::Equal {
                    return Ok(c);
                }
            }
        } else if flags & type_flags::UNION != 0 {
            // Unions are ordered by origin and then constituent type lists.
            let o1 = self.types.union(t1)?.origin;
            let o2 = self.types.union(t2)?.origin;
            match (o1, o2) {
                (None, None) => {
                    let c = self
                        .compare_type_lists(self.types.types_of(t1)?, self.types.types_of(t2)?)?;
                    if c != Ordering::Equal {
                        return Ok(c);
                    }
                }
                (None, Some(_)) => return Ok(Ordering::Greater),
                (Some(_), None) => return Ok(Ordering::Less),
                (Some(o1), Some(o2)) => {
                    let c = self.compare_types(o1, o2)?;
                    if c != Ordering::Equal {
                        return Ok(c);
                    }
                }
            }
        } else if flags & type_flags::INTERSECTION != 0 {
            // Intersections are ordered by their constituent type lists.
            let c = self.compare_type_lists(self.types.types_of(t1)?, self.types.types_of(t2)?)?;
            if c != Ordering::Equal {
                return Ok(c);
            }
        } else if flags
            & (type_flags::ENUM | type_flags::ENUM_LITERAL | type_flags::UNIQUE_ES_SYMBOL)
            != 0
        {
            // Enum members are ordered by their symbol (and thus their declaration order).
            let c = self.compare_symbols(r1.symbol, r2.symbol)?;
            if c != Ordering::Equal {
                return Ok(c);
            }
        } else if flags & type_flags::STRING_LITERAL != 0 {
            // String literal types are ordered by their values.
            let c = self.literal_string(t1)?.cmp(self.literal_string(t2)?);
            if c != Ordering::Equal {
                return Ok(c);
            }
        } else if flags & type_flags::NUMBER_LITERAL != 0 {
            // Numeric literal types are ordered by their values.
            let c = compare_numbers(self.literal_number(t1)?, self.literal_number(t2)?);
            if c != Ordering::Equal {
                return Ok(c);
            }
        } else if flags & type_flags::BOOLEAN_LITERAL != 0 {
            let (b1, b2) = (self.literal_boolean(t1)?, self.literal_boolean(t2)?);
            if b1 != b2 {
                return Ok(if b1 {
                    Ordering::Greater
                } else {
                    Ordering::Less
                });
            }
        } else if flags & type_flags::TYPE_PARAMETER != 0 {
            let c = self.compare_symbols(r1.symbol, r2.symbol)?;
            if c != Ordering::Equal {
                return Ok(c);
            }
        } else if flags & type_flags::INDEX != 0 {
            let a = self.types.index_type(t1)?;
            let b = self.types.index_type(t2)?;
            let ordering = self.compare_types(a.target, b.target)?;
            if ordering != Ordering::Equal {
                return Ok(ordering);
            }
            let ordering = a.index_flags.cmp(&b.index_flags);
            if ordering != Ordering::Equal {
                return Ok(ordering);
            }
        } else if flags & type_flags::INDEXED_ACCESS != 0 {
            let a = self.types.indexed_access(t1)?;
            let b = self.types.indexed_access(t2)?;
            let ordering = self.compare_types(a.object_type, b.object_type)?;
            if ordering != Ordering::Equal {
                return Ok(ordering);
            }
            let ordering = self.compare_types(a.index_type, b.index_type)?;
            if ordering != Ordering::Equal {
                return Ok(ordering);
            }
        } else if flags & type_flags::TEMPLATE_LITERAL != 0 {
            let (texts1, types1) = {
                let d = self.types.template_literal(t1)?;
                (d.texts.clone(), d.types.clone())
            };
            let (texts2, types2) = {
                let d = self.types.template_literal(t2)?;
                (d.texts.clone(), d.types.clone())
            };
            let c = texts1
                .iter()
                .map(tsr_ast::JsString::as_bytes)
                .cmp(texts2.iter().map(tsr_ast::JsString::as_bytes));
            if c != Ordering::Equal {
                return Ok(c);
            }
            let c = self.compare_type_lists(&types1, &types2)?;
            if c != Ordering::Equal {
                return Ok(c);
            }
        } else if flags & type_flags::STRING_MAPPING != 0 {
            let ordering = self.compare_types(self.types.target(t1)?, self.types.target(t2)?)?;
            if ordering != Ordering::Equal {
                return Ok(ordering);
            }
        } else if flags & type_flags::CONDITIONAL != 0 {
            let a = self.types.conditional(t1)?;
            let b = self.types.conditional(t2)?;
            let order = self.compare_nodes(
                Some(self.conditional_root(a.root)?.node),
                Some(self.conditional_root(b.root)?.node),
            )?;
            if order != Ordering::Equal {
                return Ok(order);
            }
            let order = self.compare_type_mappers(a.mapper, b.mapper)?;
            if order != Ordering::Equal {
                return Ok(order);
            }
        } else if flags & type_flags::SUBSTITUTION != 0 {
            let a = self.types.substitution(t1)?;
            let b = self.types.substitution(t2)?;
            let order = self.compare_types(a.base, b.base)?;
            if order != Ordering::Equal {
                return Ok(order);
            }
            let order = self.compare_types(a.constraint, b.constraint)?;
            if order != Ordering::Equal {
                return Ok(order);
            }
        }
        // Fall back to type IDs. This results in type creation order for built-in types.
        #[cfg(feature = "creation-trace")]
        if tsr_ast::creation_trace::active() {
            tsr_ast::creation_trace::record(
                serde_json::json!({"event":"fallback", "kind":"type", "branch":"final_type_id",
                "left":self.trace_type(t1), "right":self.trace_type(t2), "sign":t1.get().cmp(&t2.get()) as i8}),
            );
        }
        Ok(t1.get().cmp(&t2.get()))
    }

    /// All enum-like unit types sort as `TypeFlagsEnum`; they are then ordered by symbol.
    // port: tsc/internal/checker/utilities.go:getSortOrderFlags
    fn sort_order_flags(&self, t: TypeId) -> Result<i64, Error> {
        let flags = self.types.flags(t)?;
        if flags & (type_flags::ENUM_LITERAL | type_flags::ENUM) != 0
            && flags & type_flags::UNION == 0
        {
            return Ok(i64::from(type_flags::ENUM));
        }
        Ok(i64::from(flags))
    }

    // port: tsc/internal/checker/utilities.go:compareTypeNames
    fn compare_type_names(&self, t1: TypeId, t2: TypeId) -> Result<Ordering, Error> {
        let s1 = self.type_name_symbol(t1)?;
        let s2 = self.type_name_symbol(t2)?;
        if s1 == s2 {
            if let Some(alias) = self.types.alias_of(t1)? {
                let args1 = alias.type_arguments.clone();
                let args2 = self
                    .types
                    .alias_of(t2)?
                    .map(|alias| alias.type_arguments.clone());
                return self.compare_type_lists(&args1, args2.as_deref().unwrap_or(&[]));
            }
            return Ok(Ordering::Equal);
        }
        let (Some(s1), Some(s2)) = (s1, s2) else {
            return Ok(if s1.is_none() {
                Ordering::Greater
            } else {
                Ordering::Less
            });
        };
        Ok(self
            .symbol(s1)?
            .name_bytes()
            .cmp(self.symbol(s2)?.name_bytes()))
    }

    // port: tsc/internal/checker/utilities.go:getTypeNameSymbol
    fn type_name_symbol(&self, t: TypeId) -> Result<Option<SymbolId>, Error> {
        if let Some(alias) = self.types.alias_of(t)? {
            return Ok(Some(alias.symbol));
        }
        let record = self.types.get(t)?;
        if record.flags & (type_flags::TYPE_PARAMETER | type_flags::STRING_MAPPING) != 0
            || record.object_flags & (object_flags::CLASS_OR_INTERFACE | object_flags::REFERENCE)
                != 0
        {
            return Ok(record.symbol);
        }
        Ok(None)
    }

    // port: tsc/internal/checker/utilities.go:compareTypeLists
    pub(crate) fn compare_type_lists(
        &self,
        s1: &[TypeId],
        s2: &[TypeId],
    ) -> Result<Ordering, Error> {
        if s1.len() != s2.len() {
            return Ok(s1.len().cmp(&s2.len()));
        }
        for (t1, t2) in s1.iter().zip(s2) {
            let c = self.compare_types(*t1, *t2)?;
            if c != Ordering::Equal {
                return Ok(c);
            }
        }
        Ok(Ordering::Equal)
    }

    // port: tsc/internal/checker/utilities.go:compareTupleTypes
    fn compare_tuple_types(&self, t1: TypeId, t2: TypeId) -> Result<Ordering, Error> {
        if t1 == t2 {
            return Ok(Ordering::Equal);
        }
        let d1 = self.types.tuple(t1)?;
        let d2 = self.types.tuple(t2)?;
        if d1.readonly != d2.readonly {
            return Ok(if d1.readonly {
                Ordering::Greater
            } else {
                Ordering::Less
            });
        }
        if d1.element_infos.len() != d2.element_infos.len() {
            return Ok(d1.element_infos.len().cmp(&d2.element_infos.len()));
        }
        for (e1, e2) in d1.element_infos.iter().zip(d2.element_infos.iter()) {
            let c = i64::from(e1.flags) - i64::from(e2.flags);
            if c != 0 {
                return Ok(ordering(c));
            }
        }
        for (e1, e2) in d1.element_infos.iter().zip(d2.element_infos.iter()) {
            let c = self.compare_element_labels(e1.labeled_declaration, e2.labeled_declaration)?;
            if c != Ordering::Equal {
                return Ok(c);
            }
        }
        Ok(Ordering::Equal)
    }

    // port: tsc/internal/checker/utilities.go:compareElementLabels
    fn compare_element_labels(
        &self,
        n1: Option<NodeId>,
        n2: Option<NodeId>,
    ) -> Result<Ordering, Error> {
        match (n1, n2) {
            (None, None) => Ok(Ordering::Equal),
            (None, Some(_)) => Ok(Ordering::Less),
            (Some(_), None) => Ok(Ordering::Greater),
            (Some(a), Some(b)) if a == b => Ok(Ordering::Equal),
            (Some(a), Some(b)) => {
                let a = self
                    .ast(a)?
                    .node(a)?
                    .name()
                    .ok_or(Error::MissingLink("tuple label name"))?;
                let b = self
                    .ast(b)?
                    .node(b)?
                    .name()
                    .ok_or(Error::MissingLink("tuple label name"))?;
                Ok(self
                    .ast(a)?
                    .node_text(a)?
                    .as_bytes()
                    .cmp(self.node_text(b)?.as_bytes()))
            }
        }
    }

    /// Symbols with declarations order by their first declaration, then by
    /// name, then by runtime id.
    // port: tsc/internal/checker/utilities.go:Checker.compareSymbolsWorker
    pub(crate) fn compare_symbols(
        &self,
        s1: Option<SymbolId>,
        s2: Option<SymbolId>,
    ) -> Result<Ordering, Error> {
        if s1 == s2 {
            return Ok(Ordering::Equal);
        }
        let (Some(s1), Some(s2)) = (s1, s2) else {
            return Ok(if s1.is_none() {
                Ordering::Greater
            } else {
                Ordering::Less
            });
        };
        let sym1 = self.symbol(s1)?;
        let sym2 = self.symbol(s2)?;
        let (has1, has2) = (
            !sym1.declarations().is_empty(),
            !sym2.declarations().is_empty(),
        );
        match (has1, has2) {
            (true, true) => {
                let first1 = self.declaration_slice(sym1.declarations())?.at(0);
                let first2 = self.declaration_slice(sym2.declarations())?.at(0);
                let order = self.compare_nodes(first1, first2)?;
                if order != Ordering::Equal {
                    return Ok(order);
                }
            }
            (true, false) => return Ok(Ordering::Less),
            (false, true) => return Ok(Ordering::Greater),
            (false, false) => {}
        }
        let c = sym1.name_bytes().cmp(sym2.name_bytes());
        if c != Ordering::Equal {
            return Ok(c);
        }
        // Fall back to symbol IDs. This is a last resort that should happen only when symbols have
        // no declaration and duplicate names.
        let left = tsr_ast::runtime_symbol_id(&sym1);
        let right = tsr_ast::runtime_symbol_id(&sym2);
        #[cfg(feature = "creation-trace")]
        if tsr_ast::creation_trace::active() {
            tsr_ast::creation_trace::record(
                serde_json::json!({"event":"fallback", "kind":"symbol",
                "branch":if has1 {"equal_first_declaration"} else {"declarationless"},
                "left":self.trace_symbol(s1), "right":self.trace_symbol(s2), "sign":left.cmp(&right) as i8}),
            );
        }
        Ok(left.cmp(&right))
    }

    // port: tsc/internal/checker/utilities.go:Checker.compareNodes
    pub(crate) fn compare_nodes(
        &self,
        n1: Option<NodeId>,
        n2: Option<NodeId>,
    ) -> Result<Ordering, Error> {
        match (n1, n2) {
            (None, None) => Ok(Ordering::Equal),
            (None, Some(_)) => Ok(Ordering::Greater),
            (Some(_), None) => Ok(Ordering::Less),
            (Some(a), Some(b)) if a == b => Ok(Ordering::Equal),
            (Some(a), Some(b)) => {
                let (f1, f2) = (self.node_file_index(a)?, self.node_file_index(b)?);
                if f1 != f2 {
                    return Ok(f1.cmp(&f2));
                }
                Ok(self.ast(a)?.node(a)?.pos().cmp(&self.node(b)?.pos()))
            }
        }
    }

    /// `fileIndexMap[GetSourceFileOfNode(node)]`: a core node's file comes from
    /// the arena index; other nodes walk their ancestors as Go does.
    fn node_file_index(&self, node: NodeId) -> Result<usize, Error> {
        let program = self.program()?;
        if let Some(index) = program.core_file_index(node) {
            return Ok(index);
        }
        let view = self.ast(node)?;
        let source = tsr_ast::utilities::get_source_file_of_node(view, Some(node))?;
        Ok(program.file_index(source))
    }

    fn literal_string(&self, t: TypeId) -> Result<&[u8], Error> {
        match &self.types.literal(t)?.value {
            LiteralValue::String(text) => Ok(text.as_bytes()),
            _ => Err(Error::UnexpectedType {
                context: "string literal value",
                kind: TypeKind::Literal,
            }),
        }
    }

    fn literal_number(&self, t: TypeId) -> Result<f64, Error> {
        match &self.types.literal(t)?.value {
            LiteralValue::Number(number) => Ok(number.value()),
            _ => Err(Error::UnexpectedType {
                context: "number literal value",
                kind: TypeKind::Literal,
            }),
        }
    }

    fn literal_boolean(&self, t: TypeId) -> Result<bool, Error> {
        match &self.types.literal(t)?.value {
            LiteralValue::Boolean(value) => Ok(*value),
            _ => Err(Error::UnexpectedType {
                context: "boolean literal value",
                kind: TypeKind::Literal,
            }),
        }
    }
}

/// Go's `cmp.Compare` on `float64`: NaN sorts before every number and equals
/// itself; `-0` and `+0` are equal.
fn compare_numbers(a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => a.partial_cmp(&b).expect("non-NaN floats are ordered"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_comparison_matches_go_cmp_compare() {
        assert_eq!(compare_numbers(f64::NAN, f64::NAN), Ordering::Equal);
        assert_eq!(compare_numbers(f64::NAN, -1.0), Ordering::Less);
        assert_eq!(compare_numbers(0.0, -0.0), Ordering::Equal);
        assert_eq!(compare_numbers(2.0, 1.0), Ordering::Greater);
    }
}
