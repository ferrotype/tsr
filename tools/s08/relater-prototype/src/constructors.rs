//! Literal and union construction shared by source resolution and template
//! inference. These caches own keys; the graph remains the sole owner of cells.

use super::{flags, object_flags, unsupported, Error, Graph, LiteralValue, Rc, TypeCell, Weak};
use std::cmp::Ordering;

impl Graph {
    // port: tsc/internal/checker/checker.go:Checker.getFreshTypeOfLiteralType
    pub fn fresh_literal(&self, ty: &Rc<TypeCell>) -> Result<Rc<TypeCell>, Error> {
        if !self.owns(ty) {
            return unsupported("fresh literal owner mismatch");
        }
        if ty.flags & (flags::ENUM | flags::LITERAL) == 0 || ty.fresh {
            return Ok(ty.clone());
        }
        if let Some(fresh) = ty.alternate() {
            return Ok(fresh);
        }
        let fresh = self.allocate_full(
            ty.flags,
            0,
            ty.name.clone(),
            ty.symbol,
            None,
            ty.literal.clone(),
            true,
            Vec::new(),
            false,
            None,
        );
        *ty.alternate.borrow_mut() = Rc::downgrade(&fresh);
        *fresh.alternate.borrow_mut() = Rc::downgrade(ty);
        Ok(fresh)
    }

    /// Intern a regular literal. Fresh forms are created separately on demand.
    pub fn intern_literal(&self, flags: u32, mut value: LiteralValue, name: &str) -> Rc<TypeCell> {
        if let LiteralValue::Number(bits) = &mut value {
            if f64::from_bits(*bits) == 0.0 {
                *bits = 0;
            }
        }
        let key = (flags, value.clone());
        if let Some(existing) = self
            .literal_cache
            .borrow()
            .get(&key)
            .and_then(Weak::upgrade)
        {
            return existing;
        }
        let cell = self.allocate_full(
            flags,
            0,
            Rc::from(name),
            None,
            None,
            Some(value),
            false,
            Vec::new(),
            false,
            None,
        );
        self.literal_cache
            .borrow_mut()
            .insert(key, Rc::downgrade(&cell));
        cell
    }

    pub fn string_literal(&self, bytes: &[u8]) -> Rc<TypeCell> {
        let bytes = ts_jsstring::wtf8::combine_surrogate_pairs(bytes).into_owned();
        let escaped = ts_jsstring::escape::escape_string(&bytes, ts_jsstring::QuoteChar::Double);
        let mut name = String::from("\"");
        name.push_str(
            &String::from_utf8(escaped)
                .expect("EscapeString escapes invalid UTF-8 and lone surrogates"),
        );
        name.push('"');
        self.intern_literal(flags::STRING_LITERAL, LiteralValue::String(bytes), &name)
    }

    pub(super) fn intrinsic_cell(&self, flag: u32) -> Result<Rc<TypeCell>, Error> {
        self.types
            .borrow()
            .iter()
            .find(|cell| cell.flags == flag && cell.literal.is_none())
            .cloned()
            .ok_or_else(|| Error::Unsupported(Rc::from("intrinsic type not constructed")))
    }

    /// `getUnionType` with the pin's default literal reduction. Types supplied
    /// by the frontend already carry symbol/declaration ordering identities.
    pub fn union(&self, input: &[Rc<TypeCell>]) -> Result<Rc<TypeCell>, Error> {
        self.union_with_alias(input, None, None)
    }

    pub fn union_named(
        &self,
        input: &[Rc<TypeCell>],
        alias: u64,
        name: &str,
    ) -> Result<Rc<TypeCell>, Error> {
        self.union_named_arguments(input, alias, name, &[])
    }

    pub fn union_named_arguments(
        &self,
        input: &[Rc<TypeCell>],
        alias: u64,
        name: &str,
        arguments: &[u32],
    ) -> Result<Rc<TypeCell>, Error> {
        self.union_with_alias(input, Some((alias, name, arguments)), None)
    }

    pub(super) fn union_with_origin(
        &self,
        input: &[Rc<TypeCell>],
        origin: Option<&Rc<TypeCell>>,
    ) -> Result<Rc<TypeCell>, Error> {
        self.union_with_alias(input, None, origin)
    }

    pub(crate) fn union_with_alias(
        &self,
        input: &[Rc<TypeCell>],
        alias: Option<(u64, &str, &[u32])>,
        origin: Option<&Rc<TypeCell>>,
    ) -> Result<Rc<TypeCell>, Error> {
        if input.len() == 1 {
            return Ok(input[0].clone());
        }
        let mut members = Vec::new();
        for ty in input {
            if ty.flags & flags::UNION != 0 {
                members.extend(ty.types()?);
            } else if ty.flags & flags::NEVER == 0 {
                members.push(ty.clone());
            }
        }
        if members.iter().any(|t| t.flags & flags::ANY != 0) {
            return self.intrinsic_cell(flags::ANY);
        }
        if members.iter().any(|t| t.flags & flags::UNKNOWN != 0) {
            return self.intrinsic_cell(flags::UNKNOWN);
        }
        let includes = members.iter().fold(0, |acc, t| acc | t.flags);
        members.retain(|t| {
            !(t.flags & (flags::STRING_LITERAL | flags::TEMPLATE_LITERAL) != 0
                && includes & flags::STRING != 0
                || t.flags & flags::NUMBER_LITERAL != 0 && includes & flags::NUMBER != 0
                || t.flags & flags::BIG_INT_LITERAL != 0 && includes & flags::BIG_INT != 0
                || t.flags & flags::UNIQUE_ES_SYMBOL != 0 && includes & flags::ES_SYMBOL != 0)
        });
        members.sort_by(compare_cells);
        members.dedup_by(|left, right| Rc::ptr_eq(left, right));
        match members.as_slice() {
            [] => return self.intrinsic_cell(flags::NEVER),
            [only] => return Ok(only.clone()),
            _ => {}
        }
        // Pattern reduction itself can relate types and is not a byte-pattern
        // simplification. Fail visibly until the frontend routes it through the
        // reference checker rather than silently retaining redundant literals.
        if members.iter().any(|t| t.flags & flags::STRING_LITERAL != 0)
            && members
                .iter()
                .any(|t| t.flags & flags::TEMPLATE_LITERAL != 0)
        {
            return unsupported("union literal reduction against template patterns");
        }
        let key = (
            members.iter().map(|t| t.id).collect::<Vec<_>>(),
            alias.map(|(id, _, arguments)| (id, arguments.to_vec())),
            origin.map(|origin| origin.id()),
        );
        if let Some(existing) = self.union_cache.borrow().get(&key).and_then(Weak::upgrade) {
            return Ok(existing);
        }
        let boolean = members.len() == 2
            && members
                .iter()
                .all(|t| t.flags & flags::BOOLEAN_LITERAL != 0);
        let name = alias.map_or_else(
            || {
                members
                    .iter()
                    .map(|t| t.name.as_ref())
                    .collect::<Vec<_>>()
                    .join(" | ")
            },
            |(_, name, _)| name.to_owned(),
        );
        let cell = self.allocate_full(
            flags::UNION | if boolean { flags::BOOLEAN } else { 0 },
            if members
                .iter()
                .all(|t| t.flags & (flags::PRIMITIVE | flags::NEVER) != 0)
            {
                object_flags::PRIMITIVE_UNION
            } else {
                0
            },
            Rc::from(name),
            None,
            alias.map(|(id, _, _)| id),
            None,
            false,
            members.iter().map(Rc::downgrade).collect(),
            false,
            None,
        );
        if let Some(origin) = origin {
            cell.origin
                .set(Rc::downgrade(origin))
                .expect("new union origin");
        }
        self.union_cache
            .borrow_mut()
            .insert(key, Rc::downgrade(&cell));
        Ok(cell)
    }
}

/// `CompareTypes`: primitive and declared-object ordering used by this source
/// slice. Numeric IDs are only the final tie breaker, as in the pin.
fn compare_cells(left: &Rc<TypeCell>, right: &Rc<TypeCell>) -> Ordering {
    let flags_order = left.flags.cmp(&right.flags);
    if flags_order != Ordering::Equal {
        return flags_order;
    }
    let named = |t: &TypeCell| t.alias.is_some() || t.symbol.is_some();
    match (named(left), named(right)) {
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        (true, true) => {
            let order = left.name.cmp(&right.name);
            if order != Ordering::Equal {
                return order;
            }
        }
        _ => {}
    }
    let specific = match (&left.literal, &right.literal) {
        (Some(LiteralValue::String(l)), Some(LiteralValue::String(r))) => l.cmp(r),
        (Some(LiteralValue::Number(l)), Some(LiteralValue::Number(r))) => {
            let (l, r) = (f64::from_bits(*l), f64::from_bits(*r));
            l.partial_cmp(&r)
                .unwrap_or_else(|| l.is_nan().cmp(&r.is_nan()).reverse())
        }
        (Some(LiteralValue::Boolean(l)), Some(LiteralValue::Boolean(r))) => l.cmp(r),
        _ if left.flags & flags::OBJECT != 0 => left
            .symbol
            .cmp(&right.symbol)
            .then_with(|| left.object_flags.cmp(&right.object_flags)),
        _ => Ordering::Equal,
    };
    specific.then_with(|| left.id.cmp(&right.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_literal_cache_unifies_negative_zero() {
        let graph = Graph::new();
        let positive = graph.intern_literal(
            flags::NUMBER_LITERAL,
            LiteralValue::Number(0.0f64.to_bits()),
            "0",
        );
        let negative = graph.intern_literal(
            flags::NUMBER_LITERAL,
            LiteralValue::Number((-0.0f64).to_bits()),
            "0",
        );
        assert!(Rc::ptr_eq(&positive, &negative));
    }

    #[test]
    fn literal_freshness_allocates_a_real_pair_once() {
        let graph = Graph::new();
        let regular = graph.string_literal(b"{0}\xed\xa0\x80");
        let before = graph.len();
        let fresh = graph.fresh_literal(&regular).unwrap();
        assert_eq!(graph.len(), before + 1);
        assert!(fresh.fresh);
        assert!(!regular.fresh);
        assert_eq!(fresh.literal, regular.literal);
        assert_eq!(fresh.name, regular.name);
        assert!(Rc::ptr_eq(&fresh.alternate().unwrap(), &regular));
        assert!(Rc::ptr_eq(&graph.fresh_literal(&regular).unwrap(), &fresh));
        assert!(Rc::ptr_eq(&graph.fresh_literal(&fresh).unwrap(), &fresh));
        assert_eq!(graph.len(), before + 1);
        let intrinsic = graph.primitive(flags::ANY, "any");
        assert!(Rc::ptr_eq(
            &graph.fresh_literal(&intrinsic).unwrap(),
            &intrinsic
        ));
    }

    #[test]
    fn unions_sort_primitive_values_and_intern_after_literal_reduction() {
        let graph = Graph::new();
        let string = graph.primitive(flags::STRING, "string");
        let number = graph.primitive(flags::NUMBER, "number");
        let z = graph.string_literal(b"z");
        let a = graph.string_literal(b"a");
        let union = graph.union(&[z.clone(), a.clone()]).unwrap();
        let repeated = graph.union(&[a.clone(), z]).unwrap();
        assert!(Rc::ptr_eq(&union, &repeated));
        assert_eq!(union.types().unwrap()[0].literal, a.literal);
        let reduced = graph
            .union(&[union, number.clone(), string.clone()])
            .unwrap();
        assert_eq!(
            reduced
                .types()
                .unwrap()
                .iter()
                .map(|t| t.id())
                .collect::<Vec<_>>(),
            [string.id(), number.id()]
        );
    }
}
