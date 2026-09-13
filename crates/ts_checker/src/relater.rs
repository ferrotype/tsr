//! Production type relations. Mode caches belong to the checker; the assumed
//! relation stack belongs to one comparison. A successful outer comparison
//! commits its assumptions, while failure removes dependent assumptions.

use crate::types::Set;
use crate::{
    object_flags as of, ternary as tr, type_flags as tf, CacheKey, CheckerState, Error, Ternary,
    TypeId,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationKind {
    Identity,
    Assignable,
    Subtype,
    StrictSubtype,
    Comparable,
}

impl RelationKind {
    fn index(self) -> usize {
        match self {
            Self::Identity => 0,
            Self::Assignable => 1,
            Self::Subtype => 2,
            Self::StrictSubtype => 3,
            Self::Comparable => 4,
        }
    }
}

pub(crate) const SUCCEEDED: u32 = 1;
pub(crate) const FAILED: u32 = 2;
pub(crate) const COMPLEXITY_OVERFLOW: u32 = 1 << 5;
pub(crate) const SOURCE: u32 = 1;
pub(crate) const TARGET: u32 = 2;
pub(crate) const BOTH: u32 = SOURCE | TARGET;

#[derive(Default)]
pub(crate) struct Relations {
    pub caches: [crate::types::Map<CacheKey, u32>; 5],
    frames: Vec<Option<RelationFrame>>,
    free_frames: Vec<crate::RelationFrameId>,
    #[cfg(feature = "relation-probe")]
    pub observer: Option<Vec<Ternary>>,
}

pub(crate) struct Relater<'a> {
    pub checker: &'a mut CheckerState,
    pub kind: RelationKind,
    pub(crate) frame: crate::RelationFrameId,
    pub(crate) report_errors: bool,
    pub(crate) errors: crate::relation_errors::RelationErrors,
}

pub(crate) struct RelationFrame {
    kind: RelationKind,
    retained: bool,
    maybe_keys: Vec<CacheKey>,
    maybe_set: Set<CacheKey>,
    pub(crate) source_stack: Vec<TypeId>,
    pub(crate) target_stack: Vec<TypeId>,
    expanding: u32,
    remaining: i64,
    overflow: bool,
}

impl CheckerState {
    // port: tsc/internal/checker/relater.go:Checker.isSignatureAssignableTo
    pub(crate) fn signature_is_assignable(
        &mut self,
        source: crate::SignatureId,
        target: crate::SignatureId,
        ignore_returns: bool,
    ) -> Result<bool, Error> {
        let kind = RelationKind::Assignable;
        let frame = self.new_relation_frame(kind)?;
        let mut relater = Relater {
            checker: self,
            kind,
            frame,
            report_errors: false,
            errors: crate::relation_errors::RelationErrors::default(),
        };
        let mode = if ignore_returns {
            crate::relater_signatures::IGNORE_RETURN_TYPES
        } else {
            0
        };
        let result = relater.compare_signatures(source, target, mode, 0);
        if !relater.frame().retained {
            relater.checker.relations.frames[frame.index(0).expect("allocated relation frame")] =
                None;
            relater.checker.relations.free_frames.push(frame);
        }
        result.map(|result| result != tr::FALSE)
    }

    fn new_relation_frame(&mut self, kind: RelationKind) -> Result<crate::RelationFrameId, Error> {
        let frame = RelationFrame {
            kind,
            retained: false,
            maybe_keys: Vec::new(),
            maybe_set: Set::default(),
            source_stack: Vec::new(),
            target_stack: Vec::new(),
            expanding: 0,
            remaining: (16_000_000 - self.relations.caches[kind.index()].len() as i64) / 8,
            overflow: false,
        };
        if let Some(id) = self.relations.free_frames.pop() {
            self.relations.frames[id.index(0).expect("allocated relation frame")] = Some(frame);
            return Ok(id);
        }
        let id = crate::RelationFrameId::next(0, self.relations.frames.len())?;
        self.relations.frames.push(Some(frame));
        Ok(id)
    }

    pub(crate) fn retain_relation_frame(
        &mut self,
        id: crate::RelationFrameId,
    ) -> Result<(), Error> {
        let frame = id
            .index(0)
            .and_then(|i| self.relations.frames.get_mut(i))
            .and_then(Option::as_mut)
            .ok_or(Error::MissingLink("inference relation continuation"))?;
        frame.retained = true;
        Ok(())
    }

    /// The callback resumes the same assumptions, mode and budget. A captured
    /// frame is never recycled while a lazy inference mapper can refer to it.
    pub(crate) fn compare_in_relation_frame(
        &mut self,
        frame: crate::RelationFrameId,
        source: TypeId,
        target: TypeId,
    ) -> Result<Ternary, Error> {
        let kind = frame
            .index(0)
            .and_then(|i| self.relations.frames.get(i))
            .and_then(Option::as_ref)
            .ok_or(Error::MissingLink("inference relation continuation"))?
            .kind;
        Relater {
            checker: self,
            frame,
            kind,
            report_errors: false,
            errors: crate::relation_errors::RelationErrors::default(),
        }
        .related(source, target, BOTH, 0)
    }

    // port: tsc/internal/checker/relater.go:Checker.isTypeRelatedTo
    pub(crate) fn is_type_related_to(
        &mut self,
        mut source: TypeId,
        mut target: TypeId,
        kind: RelationKind,
    ) -> Result<bool, Error> {
        if self.is_fresh_literal_type(source)? {
            source = self.types.literal(source)?.regular;
        }
        if self.is_fresh_literal_type(target)? {
            target = self.types.literal(target)?.regular;
        }
        if source == target {
            return Ok(true);
        }
        let s = self.types.flags(source)?;
        let t = self.types.flags(target)?;
        if kind != RelationKind::Identity {
            if kind == RelationKind::Comparable
                && t & tf::NEVER == 0
                && self.simple_type_related(target, source, kind)?
                || self.simple_type_related(source, target, kind)?
            {
                return Ok(true);
            }
        } else if (s | t)
            & (tf::UNION_OR_INTERSECTION | tf::INDEXED_ACCESS | tf::CONDITIONAL | tf::SUBSTITUTION)
            == 0
        {
            if s != t {
                return Ok(false);
            }
            if s & tf::SINGLETON != 0 {
                return Ok(true);
            }
        }
        if s & t & tf::OBJECT != 0 {
            let (key, _) =
                self.relation_key(source, target, 0, kind == RelationKind::Identity, false)?;
            if let Some(&result) = self.relations.caches[kind.index()].get(&key) {
                self.variance.reliability |= result & crate::variance::RELIABILITY;
                return Ok(result & SUCCEEDED != 0);
            }
        }
        if (s | t) & tf::STRUCTURED_OR_INSTANTIABLE == 0 {
            return Ok(false);
        }
        self.check_type_related(source, target, kind)
    }

    // port: tsc/internal/checker/relater.go:Checker.checkTypeRelatedToEx
    pub(crate) fn check_type_related(
        &mut self,
        source: TypeId,
        target: TypeId,
        kind: RelationKind,
    ) -> Result<bool, Error> {
        let (result, diagnostic) = self.check_type_related_ex(source, target, kind, None, None)?;
        if let Some(diagnostic) = diagnostic {
            self.add_diagnostic(diagnostic)?;
        }
        Ok(result)
    }

    pub(crate) fn best_assignable_error_target(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        let kind = RelationKind::Assignable;
        let frame = self.new_relation_frame(kind)?;
        let mut relater = Relater {
            checker: self,
            frame,
            kind,
            report_errors: false,
            errors: crate::relation_errors::RelationErrors::default(),
        };
        let result = relater.best_error_target(source, target);
        if !relater.frame().retained {
            relater.checker.relations.frames[frame.index(0).expect("allocated relation frame")] =
                None;
            relater.checker.relations.free_frames.push(frame);
        }
        result
    }

    pub(crate) fn check_type_related_ex(
        &mut self,
        source: TypeId,
        target: TypeId,
        kind: RelationKind,
        error_node: Option<ts_arena::NodeId>,
        head: Option<&'static ts_diagnostics::Message>,
    ) -> Result<(bool, Option<ts_ast::Diagnostic>), Error> {
        let frame = self.new_relation_frame(kind)?;
        let mut relater = Relater {
            checker: self,
            frame,
            kind,
            report_errors: false,
            errors: crate::relation_errors::RelationErrors {
                error_node,
                ..Default::default()
            },
        };
        relater.report_errors = error_node.is_some() && kind != RelationKind::Identity;
        let result = relater.related_with_head(source, target, BOTH, 0, head);
        #[cfg(feature = "relation-probe")]
        if let (Some(observer), Ok(result)) = (&mut relater.checker.relations.observer, &result) {
            observer.push(*result);
        }
        let overflow = relater.frame().overflow;
        let retained = relater.frame().retained;
        if !retained {
            relater.checker.relations.frames[frame.index(0).expect("allocated relation frame")] =
                None;
            relater.checker.relations.free_frames.push(frame);
        }
        let result = result?;
        if overflow {
            let (key, _) = relater.checker.relation_key(
                source,
                target,
                0,
                kind == RelationKind::Identity,
                false,
            )?;
            relater.checker.relations.caches[kind.index()]
                .insert(key, FAILED | COMPLEXITY_OVERFLOW);
        }
        let diagnostic = if overflow {
            let source = relater
                .checker
                .type_to_string(source, crate::type_display::DEFAULT_FLAGS)?;
            let target = relater
                .checker
                .type_to_string(target, crate::type_display::DEFAULT_FLAGS)?;
            Some(relater.checker.diagnostic_for_node(
                error_node.or(relater.checker.current_node),
                ts_diagnostics::Excessive_complexity_comparing_types_0_and_1,
                vec![source, target],
            )?)
        } else {
            let error_node = relater.errors.error_node;
            relater.error_diagnostic(error_node)?
        };
        Ok((result != tr::FALSE, diagnostic))
    }

    // port: tsc/internal/checker/relater.go:Checker.isSimpleTypeRelatedTo
    pub(crate) fn simple_type_related(
        &mut self,
        source: TypeId,
        target: TypeId,
        kind: RelationKind,
    ) -> Result<bool, Error> {
        let s = self.types.flags(source)?;
        let t = self.types.flags(target)?;
        if t & tf::ANY != 0 || s & tf::NEVER != 0 || source == self.builtins.wildcard_type {
            return Ok(true);
        }
        if t & tf::UNKNOWN != 0 && !(kind == RelationKind::StrictSubtype && s & tf::ANY != 0) {
            return Ok(true);
        }
        if t & tf::NEVER != 0 {
            return Ok(false);
        }
        for (source_flags, target_flags) in [
            (tf::STRING_LIKE, tf::STRING),
            (tf::NUMBER_LIKE, tf::NUMBER),
            (tf::BIG_INT_LIKE, tf::BIG_INT),
            (tf::BOOLEAN_LIKE, tf::BOOLEAN),
            (tf::ES_SYMBOL_LIKE, tf::ES_SYMBOL),
        ] {
            if s & source_flags != 0 && t & target_flags != 0 {
                return Ok(true);
            }
        }
        if s & tf::ENUM_LITERAL != 0
            && t & tf::ENUM_LITERAL == 0
            && ((s & tf::STRING_LITERAL != 0 && t & tf::STRING_LITERAL != 0)
                || (s & tf::NUMBER_LITERAL != 0 && t & tf::NUMBER_LITERAL != 0))
            && self.types.literal(source)?.value == self.types.literal(target)?.value
        {
            return Ok(true);
        }
        if let Some((source, target)) = self.simple_enum_relation_pair(source, target)? {
            if self.enum_types_related(source, target)? {
                return Ok(true);
            }
        }
        if s & tf::UNDEFINED != 0
            && (!self.options.strict_null_checks && t & tf::UNION_OR_INTERSECTION == 0
                || t & (tf::UNDEFINED | tf::VOID) != 0)
        {
            return Ok(true);
        }
        if s & tf::NULL != 0
            && (!self.options.strict_null_checks && t & tf::UNION_OR_INTERSECTION == 0
                || t & tf::NULL != 0)
        {
            return Ok(true);
        }
        if s & tf::OBJECT != 0
            && t & tf::NON_PRIMITIVE != 0
            && !(kind == RelationKind::StrictSubtype
                && self.is_empty_anonymous_object_type(source)?
                && self.types.get(source)?.object_flags & of::FRESH_LITERAL == 0)
        {
            return Ok(true);
        }
        if matches!(kind, RelationKind::Assignable | RelationKind::Comparable) {
            if s & tf::ANY != 0 {
                return Ok(true);
            }
            if s & tf::NUMBER != 0
                && (t & tf::ENUM != 0 || t & tf::NUMBER_LITERAL != 0 && t & tf::ENUM_LITERAL != 0)
            {
                return Ok(true);
            }
            if s & tf::NUMBER_LITERAL != 0
                && s & tf::ENUM_LITERAL == 0
                && (t & tf::ENUM != 0
                    || t & tf::NUMBER_LITERAL != 0
                        && t & tf::ENUM_LITERAL != 0
                        && self.types.literal(source)?.value == self.types.literal(target)?.value)
            {
                return Ok(true);
            }
            if self.is_unknown_like_union(target)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn simple_enum_relation_pair(
        &self,
        source: TypeId,
        target: TypeId,
    ) -> Result<Option<(ts_arena::SymbolId, ts_arena::SymbolId)>, Error> {
        let s = self.types.get(source)?;
        let t = self.types.get(target)?;
        let eligible = s.flags & tf::ENUM != 0 && t.flags & tf::ENUM != 0
            || s.flags & tf::ENUM_LITERAL != 0
                && t.flags & tf::ENUM_LITERAL != 0
                && (s.flags & tf::UNION != 0 && t.flags & tf::UNION != 0
                    || s.flags & tf::LITERAL != 0
                        && t.flags & tf::LITERAL != 0
                        && self.types.literal(source)?.value == self.types.literal(target)?.value);
        if !eligible {
            return Ok(None);
        }
        let source = s.symbol.ok_or(Error::MissingLink("source enum symbol"))?;
        let target = t.symbol.ok_or(Error::MissingLink("target enum symbol"))?;
        if s.flags & tf::ENUM != 0
            && self.symbol(source)?.name_bytes() != self.symbol(target)?.name_bytes()
        {
            return Ok(None);
        }
        Ok(Some((source, target)))
    }

    // port: tsc/internal/checker/checker.go:Checker.isUnknownLikeUnionType
    fn is_unknown_like_union(&mut self, ty: TypeId) -> Result<bool, Error> {
        if !self.options.strict_null_checks || self.types.flags(ty)? & tf::UNION == 0 {
            return Ok(false);
        }
        if self.types.object_flags(ty)? & of::IS_UNKNOWN_LIKE_UNION_COMPUTED != 0 {
            return Ok(self.types.object_flags(ty)? & of::IS_UNKNOWN_LIKE_UNION != 0);
        }
        let types = self.types.types_of(ty)?;
        let mut result = false;
        if types.len() >= 3
            && self.types.flags(types[0])? & tf::UNDEFINED != 0
            && self.types.flags(types[1])? & tf::NULL != 0
        {
            for &part in &types[2..] {
                if self.is_empty_anonymous_object_type(part)? {
                    result = true;
                    break;
                }
            }
        }
        self.types.get_mut(ty)?.object_flags |=
            of::IS_UNKNOWN_LIKE_UNION_COMPUTED | if result { of::IS_UNKNOWN_LIKE_UNION } else { 0 };
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:getRelationKey
    pub(crate) fn relation_key(
        &mut self,
        mut source: TypeId,
        mut target: TypeId,
        intersection: u32,
        identity: bool,
        ignore_constraints: bool,
    ) -> Result<(CacheKey, bool), Error> {
        if identity && source.get() > target.get() {
            std::mem::swap(&mut source, &mut target);
        }
        let mut key = crate::key::KeyBuilder::new();
        let constrained = if self.reference_has_generic_arguments(source)?
            && self.reference_has_generic_arguments(target)?
        {
            key.write_byte(b'g');
            let mut parameters = Vec::new();
            let a = self.write_generic_reference_key(
                &mut key,
                source,
                0,
                ignore_constraints,
                &mut parameters,
            )?;
            key.write_byte(b',');
            let b = self.write_generic_reference_key(
                &mut key,
                target,
                0,
                ignore_constraints,
                &mut parameters,
            )?;
            a || b
        } else {
            key.write_byte(b's');
            key.write_type(source);
            key.write_type(target);
            false
        };
        key.write_u32(intersection);
        Ok((key.finish(), constrained))
    }

    fn reference_has_generic_arguments(&mut self, ty: TypeId) -> Result<bool, Error> {
        if self.types.get(ty)?.object_flags & of::REFERENCE == 0
            || self.types.type_reference(ty)?.node.is_some()
        {
            return Ok(false);
        }
        for &argument in self.get_type_arguments(ty)?.iter() {
            if self.types.flags(argument)? & tf::TYPE_PARAMETER != 0
                || self.reference_has_generic_arguments(argument)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn write_generic_reference_key(
        &mut self,
        key: &mut crate::key::KeyBuilder,
        ty: TypeId,
        depth: u32,
        ignore_constraints: bool,
        parameters: &mut Vec<TypeId>,
    ) -> Result<bool, Error> {
        key.write_type(self.types.target(ty)?);
        let mut constrained = false;
        let arguments = self
            .types
            .type_reference(ty)?
            .resolved_type_arguments
            .clone()
            .unwrap_or_else(|| [].into());
        for &argument in arguments.iter() {
            if self.types.flags(argument)? & tf::TYPE_PARAMETER != 0 {
                if ignore_constraints || self.constraint_of_type_parameter(argument)?.is_none() {
                    let index = if let Some(index) = parameters.iter().position(|&p| p == argument)
                    {
                        index
                    } else {
                        let index = parameters.len();
                        parameters.push(argument);
                        index
                    };
                    key.write_byte(b'=');
                    key.write_int(index);
                    continue;
                }
                constrained = true;
            } else if depth < 4 && self.reference_has_generic_arguments(argument)? {
                key.write_byte(b'<');
                constrained |= self.write_generic_reference_key(
                    key,
                    argument,
                    depth + 1,
                    ignore_constraints,
                    parameters,
                )?;
                key.write_byte(b'>');
                continue;
            }
            key.write_byte(b'-');
            key.write_type(argument);
        }
        Ok(constrained)
    }

    // port: tsc/internal/checker/relater.go:Checker.isDeeplyNestedType
    pub(crate) fn deeply_nested_type(
        &self,
        ty: TypeId,
        stack: &[TypeId],
        max: usize,
    ) -> Result<bool, Error> {
        if stack.len() < max {
            return Ok(false);
        }
        if self.types.flags(ty)? & tf::INTERSECTION != 0 {
            for &part in self.types.types_of(ty)? {
                if self.deeply_nested_type(part, stack, max)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        let identity = self.recursion_identity(ty)?;
        let mut last = 0;
        let mut count = 0;
        for &ty in stack {
            if self.has_recursion_identity(ty, identity)? {
                if ty.get() >= last {
                    count += 1;
                    if count >= max {
                        return Ok(true);
                    }
                }
                last = ty.get();
            }
        }
        Ok(false)
    }

    fn has_recursion_identity(
        &self,
        ty: TypeId,
        identity: crate::constraints::RecursionIdentity,
    ) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::INTERSECTION != 0 {
            for &part in self.types.types_of(ty)? {
                if self.has_recursion_identity(part, identity)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        Ok(self.recursion_identity(ty)? == identity)
    }
}

impl Relater<'_> {
    pub(crate) fn frame(&self) -> &RelationFrame {
        self.checker.relations.frames[self.frame.index(0).expect("allocated relation frame")]
            .as_ref()
            .expect("active or inference-retained frame")
    }
    fn frame_mut(&mut self) -> &mut RelationFrame {
        self.checker.relations.frames[self.frame.index(0).expect("allocated relation frame")]
            .as_mut()
            .expect("active or inference-retained frame")
    }
    // port: tsc/internal/checker/relater.go:Relater.isRelatedToEx
    pub(crate) fn related(
        &mut self,
        source: TypeId,
        target: TypeId,
        recursion: u32,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        self.related_with_head(source, target, recursion, intersection, None)
    }

    fn related_with_head(
        &mut self,
        source: TypeId,
        target: TypeId,
        recursion: u32,
        intersection: u32,
        head: Option<&'static ts_diagnostics::Message>,
    ) -> Result<Ternary, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.related_worker(source, target, recursion, intersection, head)
        })
    }

    fn related_worker(
        &mut self,
        mut source: TypeId,
        mut target: TypeId,
        recursion: u32,
        intersection: u32,
        head: Option<&'static ts_diagnostics::Message>,
    ) -> Result<Ternary, Error> {
        let original_source = source;
        let original_target = target;
        let mut report_results = true;
        let result = (|| {
            if source == target {
                return Ok(tr::TRUE);
            }
            if self.checker.types.flags(source)? & tf::OBJECT != 0
                && self.checker.types.flags(target)? & tf::PRIMITIVE != 0
            {
                return self.simple_related(source, target);
            }
            source = self.checker.normalized_type(source, false)?;
            target = self.checker.normalized_type(target, true)?;
            if source == target {
                return Ok(tr::TRUE);
            }
            let s = self.checker.types.flags(source)?;
            let t = self.checker.types.flags(target)?;
            if self.kind == RelationKind::Identity {
                if s != t {
                    return Ok(tr::FALSE);
                }
                if s & tf::SINGLETON != 0 {
                    return Ok(tr::TRUE);
                }
                return self.recursive_related(source, target, recursion, 0);
            }
            if s & tf::TYPE_PARAMETER != 0
                && self.checker.constraint_of_type_parameter(source)? == Some(target)
            {
                return Ok(tr::TRUE);
            }
            if s & tf::DEFINITELY_NON_NULLABLE != 0 && t & tf::UNION != 0 {
                let parts = self.checker.types.types_of(target)?;
                let candidate = if parts.len() == 2
                    && self.checker.types.flags(parts[0])? & tf::NULLABLE != 0
                {
                    Some(parts[1])
                } else if parts.len() == 3
                    && self.checker.types.flags(parts[0])? & tf::NULLABLE != 0
                    && self.checker.types.flags(parts[1])? & tf::NULLABLE != 0
                {
                    Some(parts[2])
                } else {
                    None
                };
                if let Some(candidate) = candidate {
                    if self.checker.types.flags(candidate)? & tf::NULLABLE == 0 {
                        target = self.checker.normalized_type(candidate, true)?;
                        if source == target {
                            return Ok(tr::TRUE);
                        }
                    }
                }
            }
            if self.simple_related(source, target)? != tr::FALSE {
                return Ok(tr::TRUE);
            }
            let t = self.checker.types.flags(target)?;
            if (s | t) & tf::STRUCTURED_OR_INSTANTIABLE == 0 {
                return Ok(tr::FALSE);
            }
            if intersection & TARGET == 0 {
                if self.excess_properties(source, target, self.report_errors)? {
                    return Ok(tr::FALSE);
                }
                if self.no_common_properties(source, target)? {
                    if self.report_errors {
                        self.report_no_common(source, target, original_source, original_target)?;
                    }
                    report_results = false;
                    return Ok(tr::FALSE);
                }
            }
            let skip = s & tf::UNION != 0
                && self.checker.types.types_of(source)?.len() < 4
                && t & tf::UNION == 0
                || t & tf::UNION != 0
                    && self.checker.types.types_of(target)?.len() < 4
                    && s & tf::STRUCTURED_OR_INSTANTIABLE == 0;
            if skip {
                self.union_intersection_related(source, target, intersection)
            } else {
                self.recursive_related(source, target, recursion, intersection)
            }
        })()?;
        if result == tr::FALSE && self.report_errors && report_results {
            self.report_error_results(original_source, original_target, source, target, head)?;
        }
        Ok(result)
    }

    fn simple_related(&mut self, source: TypeId, target: TypeId) -> Result<Ternary, Error> {
        let reverse = self.kind == RelationKind::Comparable
            && self.checker.types.flags(target)? & tf::NEVER == 0
            && self
                .checker
                .simple_type_related(target, source, self.kind)?;
        if reverse
            || self
                .checker
                .simple_type_related(source, target, self.kind)?
        {
            return Ok(tr::TRUE);
        }
        if self.report_errors {
            if let Some((source, target)) =
                self.checker.simple_enum_relation_pair(source, target)?
            {
                let source = self.checker.enum_owner_symbol(source)?;
                let target = self.checker.enum_owner_symbol(target)?;
                if self.checker.symbol(source)?.name_bytes()
                    == self.checker.symbol(target)?.name_bytes()
                    && self.checker.symbol(source)?.flags() & ts_ast::symbol_flags::REGULAR_ENUM
                        != 0
                    && self.checker.symbol(target)?.flags() & ts_ast::symbol_flags::REGULAR_ENUM
                        != 0
                {
                    if let Some(mismatch) = self.checker.enum_relation_mismatch(source, target)? {
                        self.report_enum_mismatch(mismatch)?;
                    }
                }
            }
        }
        Ok(tr::FALSE)
    }

    fn report_enum_mismatch(
        &mut self,
        mismatch: crate::enums::EnumRelationMismatch,
    ) -> Result<(), Error> {
        use crate::enums::EnumRelationMismatch as M;
        use ts_diagnostics as d;
        let (message, args) = match mismatch {
            M::Missing { property, target } => {
                let name = self.checker.symbol_to_string(property)?;
                let ty = self.checker.declared_enum_type(target)?;
                let target = self.checker.type_to_string(ty, crate::type_format_flags::USE_FULLY_QUALIFIED_TYPE)?;
                (d::Property_0_is_missing_in_type_1, vec![name, target])
            }
            M::Different { target, property, expected, actual } => (
                d::Each_declaration_of_0_1_differs_in_its_value_where_2_was_expected_but_3_was_given,
                vec![self.checker.symbol_to_string(target)?, self.checker.symbol_to_string(property)?, expected.diagnostic_text(), actual.diagnostic_text()]),
            M::StringAndUnknown { target, property, value } => (
                d::One_value_of_0_1_is_the_string_2_and_the_other_is_assumed_to_be_an_unknown_numeric_value,
                vec![self.checker.symbol_to_string(target)?, self.checker.symbol_to_string(property)?, crate::enums::EnumValue::String(value).diagnostic_text()]),
        };
        self.report_error(message, args);
        Ok(())
    }

    // port: tsc/internal/checker/relater.go:Relater.recursiveTypeRelatedTo
    fn recursive_related(
        &mut self,
        source: TypeId,
        target: TypeId,
        recursion: u32,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        if self.frame().overflow {
            return Ok(tr::FALSE);
        }
        let (key, constrained) = self.checker.relation_key(
            source,
            target,
            intersection,
            self.kind == RelationKind::Identity,
            false,
        )?;
        if let Some(&cached) = self.checker.relations.caches[self.kind.index()].get(&key) {
            if cached & FAILED == 0 || !self.report_errors || cached & COMPLEXITY_OVERFLOW != 0 {
                self.checker.variance.reliability |= cached & crate::variance::RELIABILITY;
                if self.report_errors && cached & COMPLEXITY_OVERFLOW != 0 {
                    let source = self
                        .checker
                        .type_to_string(source, crate::type_display::DEFAULT_FLAGS)?;
                    let target = self
                        .checker
                        .type_to_string(target, crate::type_display::DEFAULT_FLAGS)?;
                    self.report_error(
                        ts_diagnostics::Excessive_complexity_comparing_types_0_and_1,
                        vec![source, target],
                    );
                }
                return Ok(if cached & SUCCEEDED != 0 {
                    tr::TRUE
                } else {
                    tr::FALSE
                });
            }
        }
        if self.frame().remaining <= 0 {
            self.frame_mut().overflow = true;
            return Ok(tr::FALSE);
        }
        if self.frame().maybe_set.contains(&key) {
            return Ok(tr::MAYBE);
        }
        if constrained {
            let (broad, _) = self.checker.relation_key(
                source,
                target,
                intersection,
                self.kind == RelationKind::Identity,
                true,
            )?;
            if self.frame().maybe_set.contains(&broad) {
                return Ok(tr::MAYBE);
            }
        }
        if self.frame().source_stack.len() == 100 || self.frame().target_stack.len() == 100 {
            return Ok(tr::MAYBE);
        }
        let start = self.frame().maybe_keys.len();
        self.frame_mut().maybe_keys.push(key.clone());
        self.frame_mut().maybe_set.insert(key.clone());
        let previous = self.frame().expanding;
        if recursion & SOURCE != 0 {
            self.frame_mut().source_stack.push(source);
        }
        if recursion & TARGET != 0 {
            self.frame_mut().target_stack.push(target);
        }
        let saved_reliability = std::mem::take(&mut self.checker.variance.reliability);
        let result = (|| {
            if recursion & SOURCE != 0
                && self.frame().expanding & SOURCE == 0
                && self
                    .checker
                    .deeply_nested_type(source, &self.frame().source_stack, 3)?
            {
                self.frame_mut().expanding |= SOURCE;
            }
            if recursion & TARGET != 0
                && self.frame().expanding & TARGET == 0
                && self
                    .checker
                    .deeply_nested_type(target, &self.frame().target_stack, 3)?
            {
                self.frame_mut().expanding |= TARGET;
            }
            if self.frame().expanding == BOTH {
                Ok(tr::MAYBE)
            } else {
                self.structured_related(source, target, intersection)
            }
        })();
        let propagating = self.checker.variance.reliability;
        self.checker.variance.reliability |= saved_reliability;
        if recursion & SOURCE != 0 {
            self.frame_mut().source_stack.pop();
        }
        if recursion & TARGET != 0 {
            self.frame_mut().target_stack.pop();
        }
        self.frame_mut().expanding = previous;
        match result {
            Ok(tr::FALSE) => {
                self.checker.relations.caches[self.kind.index()].insert(key, FAILED | propagating);
                self.frame_mut().remaining -= 1;
                self.reset_maybe(start, false, propagating);
            }
            Ok(result)
                if result == tr::TRUE
                    || self.frame().source_stack.is_empty()
                        && self.frame().target_stack.is_empty() =>
            {
                self.reset_maybe(
                    start,
                    result == tr::TRUE || result == tr::MAYBE,
                    propagating,
                );
            }
            Err(_) => self.reset_maybe(start, false, propagating),
            _ => {}
        }
        result
    }

    fn reset_maybe(&mut self, start: usize, succeeded: bool, propagating: u32) {
        let frame_index = self.frame.index(0).expect("allocated relation frame");
        let relations = &mut self.checker.relations;
        let frame = relations.frames[frame_index]
            .as_mut()
            .expect("active relation frame");
        for key in frame.maybe_keys.drain(start..) {
            frame.maybe_set.remove(&key);
            if succeeded {
                relations.caches[self.kind.index()].insert(key, SUCCEEDED | propagating);
                frame.remaining -= 1;
            }
        }
    }
}

#[cfg(any(test, feature = "storage-pilot"))]
impl Relations {
    pub(crate) fn census(&self, census: &mut crate::census::Census) {
        for cache in &self.caches {
            census.key_map("relations", cache);
        }
        census.vec_capacity("relations", &self.frames, self.frames.capacity());
        census.vec_capacity("relations", &self.free_frames, self.free_frames.capacity());
        for frame in self.frames.iter().flatten() {
            census.vec_capacity("relations", &frame.maybe_keys, frame.maybe_keys.capacity());
            census.add(
                "relations",
                0,
                frame.maybe_keys.iter().map(|key| key.len()).sum(),
            );
            census.set("relations", &frame.maybe_set);
            census.add(
                "relations",
                0,
                frame.maybe_set.iter().map(|key| key.len()).sum(),
            );
            census.vec_capacity(
                "relations",
                &frame.source_stack,
                frame.source_stack.capacity(),
            );
            census.vec_capacity(
                "relations",
                &frame.target_stack,
                frame.target_stack.capacity(),
            );
        }
        #[cfg(feature = "relation-probe")]
        if let Some(observer) = &self.observer {
            census.vec_capacity("relations", observer, observer.capacity());
        }
    }
}

#[cfg(any(test, feature = "storage-pilot"))]
impl Relations {
    pub(crate) fn census_type_roots(&self, roots: &mut Vec<TypeId>) {
        for frame in self.frames.iter().flatten() {
            roots.extend_from_slice(&frame.source_stack);
            roots.extend_from_slice(&frame.target_stack);
        }
    }
}
