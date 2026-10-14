"""Exact, unapplied Rust source overlay for untimed matched-work diagnostics.

The counter inventory belongs to work_counts_go. These edits add aggregate
atomics only; they must never be used in a timed performance executable.
"""
from __future__ import annotations

from pathlib import Path

from tools.phase7.bench.work_counts_go import COUNTERS


def _bump(name: str, indent: int = 8) -> str:
    return ' ' * indent + f'crate::work_counts::bump({COUNTERS.index(name)});\n'


def _module(relation_origins: bool = False) -> str:
    names = ',\n'.join(f'    "{name}"' for name in COUNTERS)
    expression_arms = []
    for kinds, name in (
        ('ObjectLiteralExpression', 'expression_object'),
        ('FunctionExpression | K::ArrowFunction', 'expression_function'),
        ('Identifier', 'expression_identifier'),
        ('PropertyAccessExpression | K::QualifiedName', 'expression_property'),
        ('CallExpression | K::NewExpression', 'expression_call'),
        ('ArrayLiteralExpression', 'expression_array'),
    ):
        expression_arms.append(f'        Some(K::{kinds}) => bump({COUNTERS.index(name)}),')
    type_bits = '\n'.join(
        f'    if flags & crate::type_flags::{flag} != 0 {{\n'
        f'        bump({COUNTERS.index(name)});\n    }}'
        for flag, name in (
            ('OBJECT', 'type_created_object'),
            ('UNION', 'type_created_union'),
            ('INTERSECTION', 'type_created_intersection'),
            ('CONDITIONAL', 'type_created_conditional'),
        )
    )
    source = '''//! Diagnostic source overlay only; do not time this executable.
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTS: [AtomicU64; %d] = [const { AtomicU64::new(0) }; %d];
const NAMES: [&str; %d] = [
%s
];

pub(crate) fn bump(index: usize) {
    COUNTS[index].fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn expression(kind: Option<tsr_ast::SyntaxKind>) {
    use tsr_ast::SyntaxKind as K;
    match kind {
%s
        _ => {}
    }
}

pub(crate) fn type_created(flags: u32) {
    bump(%d);
%s
}

/// Called after the compiler thread joins, before the CLI exits. Failure to
/// create a fresh capture must fail the diagnostic run, never overwrite one.
pub fn dump_work_counts() -> std::io::Result<()> {
    let Some(path) = std::env::var_os("TSR_WORK_COUNTS") else {
        return Ok(());
    };
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut output = String::from("{\\\"format\\\":1,\\\"counters\\\":{");
    for (index, name) in NAMES.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        output.push('"');
        output.push_str(name);
        output.push_str("\\\":");
        output.push_str(&COUNTS[index].load(Ordering::Relaxed).to_string());
    }
    output.push_str("}}\\n");
    file.write_all(output.as_bytes())
}
''' % (len(COUNTERS), len(COUNTERS), len(COUNTERS), names,
       '\n'.join(expression_arms), COUNTERS.index('type_created'), type_bits)
    if relation_origins:
        anchor = '    output.push_str("}}\\n");\n'
        assert source.count(anchor) == 1
        source = source.replace(anchor, '''    output.push('}');
    append_relation_origins(&mut output);
    output.push_str("}\\n");
''', 1) + _relation_origins_module()
    return source


def _relation_origins_module() -> str:
    source = r'''
// Aggregate static sites only: no per-request allocation or stack capture.
type RelationOrigins = std::collections::BTreeMap<(&'static str, u32), u64>;
static RELATION_ORIGINS: std::sync::OnceLock<std::sync::Mutex<RelationOrigins>> =
    std::sync::OnceLock::new();

pub(crate) fn relation_origin(location: &'static std::panic::Location<'static>) {
    let mut origins = RELATION_ORIGINS
        .get_or_init(|| std::sync::Mutex::new(RelationOrigins::new()))
        .lock()
        .expect("relation origin mutex");
    *origins.entry((location.file(), location.line())).or_default() += 1;
}

fn append_relation_origins(output: &mut String) {
    use std::fmt::Write as _;
    let mut total = 0_u64;
    output.push_str(",\"relation_origins\":{");
    if let Some(origins) = RELATION_ORIGINS.get() {
        let origins = origins.lock().expect("relation origin mutex");
        for (index, ((file, line), count)) in origins.iter().enumerate() {
            total += *count;
            if index != 0 {
                output.push(',');
            }
            output.push('"');
            for ch in file.chars() {
                match ch {
                    '"' => output.push_str("\\\""),
                    '\\' => output.push_str("\\\\"),
                    ch if ch <= '\u{1f}' => {
                        write!(output, "\\u{:04x}", u32::from(ch)).expect("write to String");
                    }
                    ch => output.push(ch),
                }
            }
            write!(output, ":{line}\":{count}").expect("write to String");
        }
    }
    assert_eq!(total, COUNTS[_RELATION_REQUEST_].load(Ordering::Relaxed),
        "relation origin sum must equal relation requests");
    output.push('}');
}
'''
    assert source.count('_RELATION_REQUEST_') == 1
    return source.replace('_RELATION_REQUEST_', str(COUNTERS.index('relation_request')), 1)


def edits(root: Path, *, relation_origins: bool = False) -> dict[str, str]:
    """Return complete modified files; never write or apply the Rust overlay.

    Every source replacement requires exactly one literal match. A changed
    production source therefore rejects the overlay instead of silently moving
    or omitting counters. The new helper must not already exist.
    """
    helper = 'crates/tsr_checker/src/work_counts.rs'
    if (root / helper).exists():
        raise ValueError(f'{helper}: diagnostic module already exists')
    changed: dict[str, str] = {}

    def replace(path: str, old: str, new: str) -> None:
        source = changed.get(path)
        if source is None:
            source = (root / path).read_text()
        # All anchors start on a complete source line, not at file byte zero.
        # Include the newline so an eight-space anchor cannot match a deeper
        # nested line merely because its suffix has the same indentation.
        old, new = '\n' + old, '\n' + new
        matches = source.count(old)
        if matches != 1:
            raise ValueError(f'{path}: expected one exact anchor, found {matches}: {old!r}')
        changed[path] = source.replace(old, new, 1)

    def checker(file: str, old: str, new: str) -> None:
        replace(f'crates/tsr_checker/src/{file}.rs', old, new)

    def before(file: str, anchor: str, name: str, indent: int = 8) -> None:
        checker(file, anchor, _bump(name, indent) + anchor)

    def after(file: str, anchor: str, name: str, indent: int = 8) -> None:
        checker(file, anchor, anchor + _bump(name, indent))

    # chooseOverload: each relation pass, each examined candidate (including
    # the single nongeneric fast path), and the full contextual retry.
    before('calls', '        let node = state.node;\n', 'choose_overload')
    after('calls', '        if state.single_non_generic {\n', 'overload_candidate', 12)
    after('calls', '        for candidate in &mut state.candidates {\n', 'overload_candidate', 12)
    after('calls', '            if state.argument_mode != 0 {\n', 'overload_retry', 16)

    # The collector is the shared implementation of isSignatureApplicable;
    # counting its wrapper would miss diagnostics that call it directly.
    after('call_arguments', '''        output: &mut Vec<Diagnostic>,
    ) -> Result<bool, Error> {
''', 'signature_applicable')
    after('call_arguments', '''        for (index, &argument) in args[..arg_count].iter().enumerate() {
            if self.node(argument)?.kind() == K::OmittedExpression {
                continue;
            }
''', 'applicability_argument', 12)
    after('call_arguments', '''    pub(crate) fn infer_call_type_arguments_ex(
        &mut self,
        node: NodeId,
        signature: SignatureId,
        args: &[NodeId],
        mode: u32,
        context: InferenceId,
    ) -> Result<Vec<TypeId>, Error> {
''', 'infer_type_arguments')
    before('infer_types', '        let mut run = InferenceRun {\n', 'infer_types')
    after('infer_types', '''    fn infer_from_types_worker(
        &mut self,
        run: &mut InferenceRun,
        mut source: TypeId,
        mut target: TypeId,
    ) -> Result<(), Error> {
''', 'infer_from_types')

    after('query', '''    pub(crate) fn check_expression_worker(&mut self, node: NodeId) -> Result<TypeId, Error> {
''', 'expression_worker')
    checker('query', '''        let read = self.node(node)?;
        let ty = match read.kind().known() {
''', '''        let read = self.node(node)?;
        crate::work_counts::expression(read.kind().known());
        let ty = match read.kind().known() {
''')
    after('query', '''    pub(crate) fn check_expression_cached(&mut self, node: NodeId) -> Result<TypeId, Error> {
''', 'expression_cached_request')
    before('query', '        let saved_loops = std::mem::take(&mut self.flow.loop_stack);\n', 'expression_cached_miss')
    # Go checkExpressionCachedEx includes nonnormal requests. Rust inlines
    # those bypasses in these two callers; normal requests delegate above.
    after('return_inference', '''        if mode == 0 {
            self.check_expression_cached(expression)
        } else {
''', 'expression_cached_request', 12)
    after('variables', '''        } else if mode == 0 {
            self.check_expression_cached(initializer)?
        } else {
''', 'expression_cached_request', 12)
    after('call_arguments', '''    pub(crate) fn check_call_argument_ex(
        &mut self,
        node: NodeId,
        contextual: TypeId,
        inference: Option<InferenceId>,
        mode: u32,
    ) -> Result<TypeId, Error> {
''', 'expression_contextual')
    # checkDeclarationInitializer inlines this contextual operation in Rust.
    after('variables', '''        let ty = if let Some(ty) = self.quick_type_of_expression(initializer)? {
            ty
        } else if let Some(ty) = contextual {
''', 'expression_contextual', 12)
    after('expressions', '''        if let Some(&ty) = self.query.context_free_types.get(&node) {
''', 'expression_context_free_hit', 12)

    after('instantiate', '''    pub(crate) fn instantiate_type_with_alias(
        &mut self,
        ty: TypeId,
        mapper: Option<MapperId>,
        alias: Option<AliasId>,
    ) -> Result<TypeId, Error> {
''', 'instantiate_type_request')
    checker('instantiate', '        let Some(mapper) = mapper else { return Ok(ty) };\n',
            '        let Some(mapper) = mapper else {\n'
            + _bump('instantiate_type_early_return', 12)
            + '            return Ok(ty);\n        };\n')
    checker('instantiate', '''        if !contains {
            return Ok(ty);
        }
''', '        if !contains {\n' + _bump('instantiate_type_early_return', 12)
            + '            return Ok(ty);\n        }\n')
    after('instantiate', '''        if let Some(&cached) = self.instantiation.active[index].1.get(&key) {
''', 'instantiate_type_cache_hit', 12)
    after('instantiate', '''    fn instantiate_type_worker(
        &mut self,
        ty: TypeId,
        mapper: MapperId,
        alias: Option<AliasId>,
    ) -> Result<TypeId, Error> {
''', 'instantiate_type_worker')

    # getSignatureInstantiationWithoutFillingInTypeArguments is folded into
    # this Rust helper, including the direct higher-order/inference callers.
    after('infer_signatures', '''    pub(crate) fn signature_instantiation(
        &mut self,
        signature: SignatureId,
        arguments: &[TypeId],
        javascript: bool,
    ) -> Result<SignatureId, Error> {
''', 'signature_instantiation_request')
    after('infer_signatures', '''        if let Some(&cached) = self.signatures.instantiations.get(&key) {
''', 'signature_instantiation_cache_hit', 12)
    after('source_signatures', '''    pub(crate) fn instantiate_signature_ex(
        &mut self,
        signature: SignatureId,
        mapper: MapperId,
        erase: bool,
    ) -> Result<SignatureId, Error> {
''', 'instantiate_signature_worker')
    before('signatures', '        let id = SignatureId::next(0, self.signatures.len())?;\n', 'signature_created')
    checker('types', '        let id = TypeId::next(self.base, self.records.len())?;\n',
            '        crate::work_counts::type_created(flags);\n'
            + '        let id = TypeId::next(self.base, self.records.len())?;\n')

    after('relater', '''    pub(crate) fn is_type_related_to(
        &mut self,
        mut source: TypeId,
        mut target: TypeId,
        kind: RelationKind,
    ) -> Result<bool, Error> {
''', 'relation_request')
    if relation_origins:
        # Propagate through the thin assignable adapter and public handle/mode
        # adapters; retain semantic helpers as their own attribution sites.
        for file, header in (
            ('relater', '    pub(crate) fn is_type_related_to(\n'),
            ('check', '    pub(crate) fn source_type_assignable(\n'),
            ('handles', '    pub fn is_type_related_to(\n'),
            ('exports', '    pub fn is_type_assignable_to(\n'),
        ):
            checker(file, header, '    #[track_caller]\n' + header)
        checker('relater', _bump('relation_request'), _bump('relation_request')
                + '        crate::work_counts::relation_origin(std::panic::Location::caller());\n')
    after('relater', '''            if let Some(&result) = self.relations.caches[kind.index()].get(&key) {
''', 'relation_cache_hit', 16)
    # Rust's worker, unlike its public related wrapper, includes calls carrying
    # a diagnostic head; this is Go isRelatedToEx's common semantic body.
    before('relater', '        let original_source = source;\n', 'relation_worker')
    after('relater', '''    fn recursive_related(
        &mut self,
        source: TypeId,
        target: TypeId,
        recursion: u32,
        intersection: u32,
    ) -> Result<Ternary, Error> {
''', 'relation_recursive')
    after('relater', '''            if cached & FAILED == 0 || !self.report_errors || cached & COMPLEXITY_OVERFLOW != 0 {
''', 'relation_recursive_cache_hit', 16)
    after('relater_properties', '''    pub(crate) fn properties_related_except(
        &mut self,
        source: TypeId,
        target: TypeId,
        optional_only: bool,
        intersection: u32,
        excluded: &[tsr_ast::JsString],
    ) -> Result<Ternary, Error> {
''', 'relation_properties')

    after('flow', '''    pub(crate) fn flow_type_of_reference_at_flow(
        &mut self,
        reference: NodeId,
        declared: TypeId,
        initial: TypeId,
        container: Option<NodeId>,
        flow_owner: NodeId,
        flow: Option<FlowId>,
    ) -> Result<TypeId, Error> {
''', 'flow_reference')
    # Go builds synthetic true/false condition flow nodes for this query;
    # Rust applies narrowing directly after walking its existing antecedents.
    # Count the query, but do not invent visits to nonexistent Rust nodes.
    after('flow', '''    pub(crate) fn infer_predicate_flow_type(
        &mut self,
        reference: NodeId,
        declared: TypeId,
        initial: TypeId,
        function: NodeId,
        expression: NodeId,
        assume_true: bool,
    ) -> Result<TypeId, Error> {
''', 'flow_reference')
    after('flow', '''            let mut shared = None;
            loop {
''', 'flow_node_visit', 16)
    after('flow', '''                    if let Some((_, ty)) = query.shared.iter().find(|(id, _)| *id == flow) {
''', 'flow_shared_hit', 24)

    after('flow', '''    pub(crate) fn reachable_flow(&mut self, owner: NodeId, flow: FlowId) -> Result<bool, Error> {
''', 'reachable_request')
    # Two query paths reuse the retained graph and bypass reachable_flow.
    # Increment their requests at the call sites, not in the shared callee.
    for prefix in (
        '                    if self.matching_query_reference(query, target)? {\n',
        '                    } else if self.query_reference_contained(query, target)? {\n',
    ):
        anchor = prefix + '                        if !self.reachable_flow_in(&query.graph, flow)? {\n'
        checker('flow', anchor, prefix + _bump('reachable_request', 24)
                + '                        if !self.reachable_flow_in(&query.graph, flow)? {\n')
    # Rust additionally memoizes unshared pass-through nodes. These counters
    # observe actual cache reuse; they do not assume the two cache policies match.
    checker('flow', '''            if last == flow {
                return Ok(result);
            }
''', '''            if last == flow {
''' + _bump('reachable_node_visit', 16) + _bump('reachable_cache_hit', 16)
            + '                return Ok(result);\n            }\n')
    after('flow', '''        if let Some(&result) = self.flow.reachable.get(&flow) {
''', 'reachable_node_visit', 12)
    after('flow', '''        if let Some(&result) = self.flow.reachable.get(&flow) {
''', 'reachable_cache_hit', 12)
    after('flow', '''            let result = loop {
''', 'reachable_node_visit', 16)
    after('flow', '''                    if last == flow {
''', 'reachable_cache_hit', 24)
    after('flow', '''                    if let Some(&result) = self.flow.reachable.get(&flow) {
''', 'reachable_cache_hit', 24)

    changed[helper] = _module(relation_origins)
    checker('lib', 'mod access_expressions;\n',
            'mod work_counts;\npub use work_counts::dump_work_counts;\n\nmod access_expressions;\n')
    replace('crates/tsr_project/src/lib.rs', 'pub mod api;\n',
            'pub use tsr_checker::dump_work_counts;\n\npub mod api;\n')
    replace('crates/tsrust/src/main.rs', '        .join();\n    match result {\n',
            '        .join();\n    tsr_project::dump_work_counts().expect("write checker work counts");\n'
            + '    match result {\n')
    return changed
