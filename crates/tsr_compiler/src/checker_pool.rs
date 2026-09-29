//! The compiler's checker pool (docs/PHASE2-C6-plan.md, C6.3): the pin's
//! `compiler/checkerpool.go`, a different type from the project system's pool
//! (`tsr_project::CheckerPool`); both implement [`tsr_checker::CheckerPool`].
//!
//! The pool owns a fixed number of checkers over one program, created once,
//! in a work group, on first use, under one generation, and associates every
//! program file with one of them by the pin's weighted FENNEL partition of the
//! import graph. The threading model: a [`CheckerOwner`] is used by whichever
//! thread holds its [`Operation`] (a group task, a per-file exclusive
//! acquisition, a resolver call), never confined to a thread with requests
//! forwarded to it. The pin's non-exclusive acquisitions hand out a checker
//! without its lock and every resolver method then takes the checker's lock
//! for that call; here they hand out the owner, and each call acquires the
//! owner's operation, waiting for the current holder like any other.
use crate::{Program, ProgramCheckerHost};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use tsr_arena::{CheckerIdentity, Counters, Generation, NodeId};
use tsr_ast::Diagnostic;
use tsr_checker::{
    CheckerHost, CheckerLifetime, CheckerOwner, CheckerPool, Error, Operation, TraceSink, Tracer,
};
use tsr_core::workgroup::WorkGroup;

// The pin's calibrated constants (checkerpool.go; ADR 0009 keeps them).
const CHECKER_ASSOCIATION_TEXT_WEIGHT_DIVISOR: i64 = 100;
const CHECKER_ASSOCIATION_SOURCE_FILE_WEIGHT_MULTIPLIER: i64 = 4;
const CHECKER_ASSOCIATION_BALANCE_PENALTY_MULTIPLIER: i64 = 16;
const CHECKER_ASSOCIATION_PRIORITIZED_SOURCE_PENALTY: i64 = 12;
const CHECKER_ASSOCIATION_STRONG_BALANCE_MIN_CHECKER_COUNT: i64 = 4;

/// One of the pin's three calibrated association regimes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckerAssociationPolicy {
    pub prioritize_source_files: bool,
    pub source_file_weight_multiplier: i64,
    pub balance_penalty_multiplier: i64,
}

// port: tsc/internal/compiler/checkerpool.go:getCheckerAssociationPolicy
pub fn checker_association_policy(
    total_weight: i64,
    declaration_weight: i64,
    checker_count: i64,
) -> CheckerAssociationPolicy {
    if should_prioritize_source_files(total_weight, declaration_weight, checker_count) {
        return CheckerAssociationPolicy {
            prioritize_source_files: true,
            source_file_weight_multiplier: 1,
            balance_penalty_multiplier: CHECKER_ASSOCIATION_PRIORITIZED_SOURCE_PENALTY,
        };
    }
    if checker_count >= CHECKER_ASSOCIATION_STRONG_BALANCE_MIN_CHECKER_COUNT {
        return CheckerAssociationPolicy {
            prioritize_source_files: false,
            source_file_weight_multiplier: CHECKER_ASSOCIATION_SOURCE_FILE_WEIGHT_MULTIPLIER,
            balance_penalty_multiplier: CHECKER_ASSOCIATION_BALANCE_PENALTY_MULTIPLIER,
        };
    }
    CheckerAssociationPolicy {
        prioritize_source_files: false,
        source_file_weight_multiplier: 1,
        balance_penalty_multiplier: 1,
    }
}

/// The penalty and score of checkerpool.go:210-211 with the rounding Go's
/// compiler gives them on this architecture. On arm64, go1.27.1 fuses the
/// `oldWeight*math.Sqrt(oldWeight)` product into the subtraction and the
/// `alpha*(…)` product into the score, two `FMSUBD`s
/// (`data/phase2/c6-fusion-arm64-go1.27.1.txt`); on amd64 every step is
/// rounded, `GOAMD64=v3` included. Assignments therefore equal Go's on each
/// host (decision 4); other architectures take the unfused arithmetic.
#[cfg(target_arch = "aarch64")]
fn association_score(neighbors: f64, alpha: f64, old_weight: f64, new_weight: f64) -> f64 {
    let difference = (-old_weight).mul_add(old_weight.sqrt(), new_weight * new_weight.sqrt());
    (-alpha).mul_add(difference, neighbors)
}

#[cfg(not(target_arch = "aarch64"))]
fn association_score(neighbors: f64, alpha: f64, old_weight: f64, new_weight: f64) -> f64 {
    let penalty = alpha * (new_weight * new_weight.sqrt() - old_weight * old_weight.sqrt());
    neighbors - penalty
}

/// The weighted FENNEL assignment with gamma 3/2: each file, in `file_order`
/// (program order when `None`), goes to the checker where it has the most
/// already-placed neighbors minus the convex load increment, under a maximum
/// checker weight of the larger of the largest file and the average plus one
/// percent; ties go to the lower load, then the lower index; a file no
/// checker can take goes to the least-loaded one.
// port: tsc/internal/compiler/checkerpool.go:getCheckerAssociationsInOrder
#[allow(clippy::cast_precision_loss)]
pub fn checker_associations_in_order(
    file_weights: &[i64],
    adjacent_files: &[Vec<usize>],
    file_order: Option<&[usize]>,
    checker_count: usize,
    penalty_multiplier: i64,
) -> Vec<usize> {
    if file_weights.is_empty() {
        return Vec::new();
    }
    let mut total_weight: i64 = 0;
    let mut max_file_weight: i64 = 0;
    let mut edge_count: i64 = 0;
    for (index, &weight) in file_weights.iter().enumerate() {
        total_weight += weight;
        max_file_weight = max_file_weight.max(weight);
        edge_count += adjacent_files[index].len() as i64;
    }
    let mut associations: Vec<Option<usize>> = vec![None; file_weights.len()];
    let mut checker_weights = vec![0_i64; checker_count];
    let count = checker_count as i64;
    let average_checker_weight = (total_weight + count - 1) / count;
    let max_checker_weight =
        max_file_weight.max(average_checker_weight + average_checker_weight / 100);
    let total_weight_float = total_weight as f64;
    let alpha = penalty_multiplier as f64 * (edge_count / 2) as f64 * (count as f64).sqrt()
        / (total_weight_float * total_weight_float.sqrt());
    let mut neighbor_counts = vec![0_i64; checker_count];
    for position in 0..file_weights.len() {
        let file_index = file_order.map_or(position, |order| order[position]);
        neighbor_counts.fill(0);
        for &adjacent_file in &adjacent_files[file_index] {
            if let Some(checker_index) = associations[adjacent_file] {
                neighbor_counts[checker_index] += 1;
            }
        }
        let weight = file_weights[file_index];
        let mut best_checker: Option<usize> = None;
        let mut best_score = f64::NEG_INFINITY;
        for (checker_index, &checker_weight) in checker_weights.iter().enumerate() {
            if checker_weight + weight > max_checker_weight {
                continue;
            }
            let score = association_score(
                neighbor_counts[checker_index] as f64,
                alpha,
                checker_weight as f64,
                (checker_weight + weight) as f64,
            );
            #[allow(clippy::float_cmp)]
            if score > best_score
                || score == best_score
                    && best_checker.is_none_or(|best| checker_weight < checker_weights[best])
            {
                best_checker = Some(checker_index);
                best_score = score;
            }
        }
        let best_checker = best_checker.unwrap_or_else(|| {
            let mut best = 0;
            for (checker_index, &checker_weight) in checker_weights.iter().enumerate().skip(1) {
                if checker_weight < checker_weights[best] {
                    best = checker_index;
                }
            }
            best
        });
        associations[file_index] = Some(best_checker);
        checker_weights[best_checker] += weight;
    }
    associations
        .into_iter()
        .map(|association| association.expect("every file is placed"))
        .collect()
}

/// Source files before declarations, each group by descending weight, ties by
/// program order; `None` keeps program order.
// port: tsc/internal/compiler/checkerpool.go:getCheckerAssociationOrder
pub fn checker_association_order(
    file_weights: &[i64],
    is_declaration_file: &[bool],
    prioritize_source_files: bool,
) -> Option<Vec<usize>> {
    if !prioritize_source_files {
        return None;
    }
    let mut file_order: Vec<usize> = (0..file_weights.len()).collect();
    file_order.sort_by(|&left, &right| {
        is_declaration_file[left]
            .cmp(&is_declaration_file[right])
            .then(file_weights[right].cmp(&file_weights[left]))
            .then(left.cmp(&right))
    });
    Some(file_order)
}

// port: tsc/internal/compiler/checkerpool.go:getCheckerAssociationBaseWeight
pub fn checker_association_base_weight(node_count: i64, text_length: i64) -> i64 {
    (node_count + text_length / CHECKER_ASSOCIATION_TEXT_WEIGHT_DIVISOR).max(1)
}

// port: tsc/internal/compiler/checkerpool.go:shouldPrioritizeSourceFiles
pub fn should_prioritize_source_files(
    total_weight: i64,
    declaration_weight: i64,
    checker_count: i64,
) -> bool {
    declaration_weight * checker_count * 2 <= total_weight
}

/// Base weights plus the syntactic import count in normalized import units.
// port: tsc/internal/compiler/checkerpool.go:getCheckerAssociationWeights
pub fn checker_association_weights(base_weights: &[i64], import_counts: &[i64]) -> Vec<i64> {
    let total_base_weight: i64 = base_weights.iter().sum();
    let total_imports: i64 = import_counts.iter().sum();
    let import_weight = if total_imports > 0 {
        (total_base_weight / total_imports).max(1)
    } else {
        0
    };
    base_weights
        .iter()
        .zip(import_counts)
        .map(|(base_weight, imports)| base_weight + imports * import_weight)
        .collect()
}

/// What `createCheckers` computes to associate a program's files with its
/// checkers, every vector in program order: the per-file inputs, and with
/// more than one checker the regime, the final weights, the stream order and
/// the associations. With one checker the pin computes nothing and every
/// file goes to checker 0. This is the record the C6.7 assignment witnesses
/// compare with the pin's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckerAssociationPlan {
    pub checker_count: usize,
    pub node_counts: Vec<i64>,
    pub text_lengths: Vec<i64>,
    pub import_counts: Vec<i64>,
    pub is_declaration_file: Vec<bool>,
    pub adjacency: Vec<Vec<usize>>,
    pub policy: Option<CheckerAssociationPolicy>,
    pub file_weights: Vec<i64>,
    pub order: Option<Vec<usize>>,
    pub associations: Vec<usize>,
}

impl CheckerAssociationPlan {
    /// The association step of `createCheckers` over per-file inputs.
    pub fn compute(
        checker_count: usize,
        node_counts: Vec<i64>,
        text_lengths: Vec<i64>,
        import_counts: Vec<i64>,
        is_declaration_file: Vec<bool>,
        adjacency: Vec<Vec<usize>>,
    ) -> Self {
        let files = node_counts.len();
        let mut plan = Self {
            checker_count,
            node_counts,
            text_lengths,
            import_counts,
            is_declaration_file,
            adjacency,
            policy: None,
            file_weights: Vec::new(),
            order: None,
            associations: vec![0; files],
        };
        if checker_count <= 1 {
            return plan;
        }
        let mut base_weights = Vec::with_capacity(files);
        let mut total_base_weight = 0;
        let mut declaration_base_weight = 0;
        for index in 0..files {
            let base_weight =
                checker_association_base_weight(plan.node_counts[index], plan.text_lengths[index]);
            total_base_weight += base_weight;
            if plan.is_declaration_file[index] {
                declaration_base_weight += base_weight;
            }
            base_weights.push(base_weight);
        }
        let policy = checker_association_policy(
            total_base_weight,
            declaration_base_weight,
            checker_count as i64,
        );
        if policy.source_file_weight_multiplier != 1 {
            // Before import normalization, as the pin: the policy raises both
            // source-file work and the normalized import unit.
            for (base_weight, &declaration) in
                base_weights.iter_mut().zip(&plan.is_declaration_file)
            {
                if !declaration {
                    *base_weight *= policy.source_file_weight_multiplier;
                }
            }
        }
        plan.file_weights = checker_association_weights(&base_weights, &plan.import_counts);
        plan.order = checker_association_order(
            &plan.file_weights,
            &plan.is_declaration_file,
            policy.prioritize_source_files,
        );
        plan.associations = checker_associations_in_order(
            &plan.file_weights,
            &plan.adjacency,
            plan.order.as_deref(),
            checker_count,
            policy.balance_penalty_multiplier,
        );
        plan.policy = Some(policy);
        plan
    }

    /// The plan `createCheckers` computes for `program` with `checker_count`
    /// checkers: node counts, text lengths in bytes, syntactic import counts
    /// and declaration-file flags read from the parsed files, and the import
    /// adjacency from the program's resolutions.
    pub fn for_program(program: &Arc<Program>, checker_count: usize) -> Result<Self, Error> {
        let mut node_counts = Vec::new();
        let mut text_lengths = Vec::new();
        let mut import_counts = Vec::new();
        let mut is_declaration_file = Vec::new();
        for file in program.files() {
            let source = file.bound().view().source_file()?;
            node_counts.push(source.node_count);
            text_lengths.push(i64::try_from(source.text().len()).expect("text length fits i64"));
            import_counts
                .push(i64::try_from(source.imports()?.len()).expect("import count fits i64"));
            is_declaration_file.push(source.is_declaration_file);
        }
        let adjacency = if checker_count > 1 {
            import_adjacency(program)?
        } else {
            vec![Vec::new(); node_counts.len()]
        };
        Ok(Self::compute(
            checker_count,
            node_counts,
            text_lengths,
            import_counts,
            is_declaration_file,
            adjacency,
        ))
    }
}

/// The undirected import graph by file index: an edge for every resolved
/// in-program module entry of a file (one per distinct name and mode, so
/// several may connect one pair), without self edges or targets outside the
/// program. Adjacency order is not observable; only counts are read.
// port: tsc/internal/compiler/checkerpool.go:checkerPool.getImportAdjacency
fn import_adjacency(program: &Arc<Program>) -> Result<Vec<Vec<usize>>, Error> {
    let host = ProgramCheckerHost::new(program.clone());
    let mut file_indices = HashMap::with_capacity(program.files().len());
    let mut paths = HashMap::with_capacity(program.files().len());
    for (index, file) in program.files().iter().enumerate() {
        file_indices.insert(file.source(), index);
        let source = file.bound().view().source_file()?;
        paths.insert(source.parse_options().path.as_bytes().to_vec(), index);
    }
    let mut adjacent_files = vec![Vec::new(); program.files().len()];
    for resolution in program.resolutions() {
        let Some(&file_index) = paths.get(resolution.file.as_bytes()) else {
            continue;
        };
        if !resolution.result.is_resolved() {
            continue;
        }
        let Some(imported) = host
            .get_source_file_for_resolved_module(resolution.result.resolved_file_name.as_bytes())
        else {
            continue;
        };
        let Some(&imported_index) = file_indices.get(&imported.source()) else {
            continue;
        };
        if imported_index == file_index {
            continue;
        }
        adjacent_files[file_index].push(imported_index);
        adjacent_files[imported_index].push(file_index);
    }
    Ok(adjacent_files)
}

struct Checkers {
    owners: Vec<Arc<CheckerOwner>>,
    plan: CheckerAssociationPlan,
    /// Program file index by source file.
    file_indices: HashMap<NodeId, usize>,
}

/// The compiler's checker pool over one program (the pin's `checkerPool`).
pub struct CompilerCheckerPool {
    program: Arc<Program>,
    host: Arc<dyn CheckerHost>,
    counters: Counters,
    generation: Generation,
    tracing: Option<Arc<dyn TraceSink>>,
    checker_count: usize,
    checkers: OnceLock<Result<Checkers, Error>>,
}

impl CompilerCheckerPool {
    // port: tsc/internal/compiler/checkerpool.go:newCheckerPool
    pub fn new(program: Arc<Program>, counters: &Counters) -> Self {
        Self::with_tracing(program, counters, None)
    }

    /// Four checkers by default, one when the program is single-threaded,
    /// otherwise the `checkers` option; at least one, and at most the smaller
    /// of the file count and 256. A trace session gives every checker a
    /// tracer with its index.
    // port: tsc/internal/compiler/checkerpool.go:newCheckerPoolWithTracing
    pub fn with_tracing(
        program: Arc<Program>,
        counters: &Counters,
        tracing: Option<Arc<dyn TraceSink>>,
    ) -> Self {
        let mut checker_count: isize = 4;
        if program.single_threaded() {
            checker_count = 1;
        } else if let Some(checkers) = program.options().checkers {
            checker_count = checkers;
        }
        let files = isize::try_from(program.files().len()).unwrap_or(isize::MAX);
        let checker_count = checker_count.min(files).clamp(1, 256);
        Self {
            host: Arc::new(ProgramCheckerHost::new(program.clone())),
            program,
            counters: counters.clone(),
            generation: Generation::new(counters),
            tracing,
            checker_count: usize::try_from(checker_count).expect("clamped to 1..=256"),
            checkers: OnceLock::new(),
        }
    }

    pub fn program(&self) -> &Arc<Program> {
        &self.program
    }

    /// The generation every checker of the pool shares: a panic in any of
    /// them retires it, and every other thread stops at its next gate.
    pub fn generation(&self) -> &Generation {
        &self.generation
    }

    pub fn checker_count(&self) -> usize {
        self.checker_count
    }

    /// Creates the checkers once, in a work group (in parallel unless the
    /// program is single-threaded, where the group runs the last queued
    /// creation first), then associates every file with one of them. A
    /// failure is recorded and returned to every later caller; a panic
    /// retires the pool's generation.
    // port: tsc/internal/compiler/checkerpool.go:checkerPool.createCheckers
    fn create_checkers(&self) -> Result<&Checkers, Error> {
        self.checkers
            .get_or_init(|| {
                let _unwind = RetireOnUnwind(&self.generation);
                self.generation.validate()?;
                let created: Vec<Mutex<Option<Result<CheckerOwner, Error>>>> =
                    (0..self.checker_count).map(|_| Mutex::new(None)).collect();
                let group = WorkGroup::new(self.program.single_threaded());
                for (index, slot) in created.iter().enumerate() {
                    group.queue(move || {
                        let tracer = self
                            .tracing
                            .as_ref()
                            .map(|session| Tracer::new(session.clone(), index));
                        let owner = CheckerOwner::for_program_with_tracer(
                            CheckerIdentity::new(self.generation.clone(), &self.counters),
                            &self.counters,
                            self.host.clone(),
                            tracer,
                        );
                        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(owner);
                    });
                }
                group.run_and_wait();
                drop(group);
                let owners = created
                    .into_iter()
                    .map(|slot| {
                        slot.into_inner()
                            .unwrap_or_else(PoisonError::into_inner)
                            .expect("every queued creation ran")
                            .map(Arc::new)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let plan = CheckerAssociationPlan::for_program(&self.program, self.checker_count)?;
                let file_indices = self
                    .program
                    .files()
                    .iter()
                    .enumerate()
                    .map(|(index, file)| (file.source(), index))
                    .collect();
                Ok(Checkers {
                    owners,
                    plan,
                    file_indices,
                })
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// The pool's checkers in index order, creating them.
    pub fn checkers(&self) -> Result<&[Arc<CheckerOwner>], Error> {
        Ok(&self.create_checkers()?.owners)
    }

    /// The association plan of this pool's program, creating the checkers.
    pub fn association_plan(&self) -> Result<&CheckerAssociationPlan, Error> {
        Ok(&self.create_checkers()?.plan)
    }

    fn checker_index_for_file(checkers: &Checkers, file: NodeId) -> Result<usize, Error> {
        let index = checkers
            .file_indices
            .get(&file)
            .ok_or(tsr_arena::Error::WrongOwner)?;
        Ok(checkers.plan.associations[*index])
    }

    /// The checker associated with `file`, without holding it: each call on
    /// it acquires its operation (the pin's lock-free hand-out to the emit
    /// resolver, whose methods lock per call). It has no release, where the
    /// pin returns `noop`.
    // port: tsc/internal/compiler/checkerpool.go:checkerPool.getCheckerForFileNonExclusive
    pub fn checker_for_file_non_exclusive(
        &self,
        file: NodeId,
    ) -> Result<&Arc<CheckerOwner>, Error> {
        let checkers = self.create_checkers()?;
        Ok(&checkers.owners[Self::checker_index_for_file(checkers, file)?])
    }

    /// The checker associated with `file`, held exclusively until the
    /// operation is dropped (the pin's lock and its release).
    // port: tsc/internal/compiler/checkerpool.go:checkerPool.getCheckerForFileExclusive
    pub fn checker_for_file_exclusive(&self, file: NodeId) -> Result<Operation<'_>, Error> {
        self.checker_for_file_non_exclusive(file)?.operation()
    }

    /// The first checker, without holding it.
    // port: tsc/internal/compiler/checkerpool.go:checkerPool.getCheckerNonExclusive
    pub fn checker_non_exclusive(&self) -> Result<&Arc<CheckerOwner>, Error> {
        Ok(&self.create_checkers()?.owners[0])
    }

    /// Runs `task` for every checker at once, each holding its checker; safe
    /// to call from many threads, each waiting for the checkers it needs.
    // port: tsc/internal/compiler/checkerpool.go:checkerPool.forEachCheckerParallel
    pub fn for_each_checker_parallel(
        &self,
        task: &(dyn Fn(usize, &mut Operation<'_>) + Sync),
    ) -> Result<(), Error> {
        let checkers = self.create_checkers()?;
        let failure = Mutex::new(None);
        let group = WorkGroup::new(self.program.single_threaded());
        for (index, owner) in checkers.owners.iter().enumerate() {
            let failure = &failure;
            group.queue(move || match owner.operation() {
                Ok(mut operation) => task(index, &mut operation),
                Err(error) => {
                    failure
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .get_or_insert(error);
                }
            });
        }
        group.run_and_wait();
        drop(group);
        failure
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
            .map_or(Ok(()), Err)
    }

    /// Every checker's global diagnostics, concatenated in checker order,
    /// then sorted and deduplicated.
    // port: tsc/internal/compiler/checkerpool.go:checkerPool.GetGlobalDiagnostics
    pub fn global_diagnostics(&self) -> Result<Vec<Diagnostic>, crate::Error> {
        let checkers = self.create_checkers()?;
        let global: Vec<Mutex<Result<Vec<Diagnostic>, Error>>> = checkers
            .owners
            .iter()
            .map(|_| Mutex::new(Ok(Vec::new())))
            .collect();
        self.for_each_checker_parallel(&|index, operation| {
            *global[index].lock().unwrap_or_else(PoisonError::into_inner) =
                operation.global_diagnostics();
        })?;
        let mut concatenated = Vec::new();
        for diagnostics in global {
            concatenated.extend(
                diagnostics
                    .into_inner()
                    .unwrap_or_else(PoisonError::into_inner)?,
            );
        }
        self.program.sort_and_deduplicate_diagnostics(&concatenated)
    }

    /// One task per checker, at once unless `single_threaded`: each holds its
    /// checker and visits, in the order given, the files associated with it,
    /// passing each file's position in `files`.
    // port: tsc/internal/compiler/checkerpool.go:checkerPool.forEachCheckerGroupDo
    pub fn for_each_checker_group_do(
        &self,
        files: &[NodeId],
        single_threaded: bool,
        task: &(dyn Fn(&mut Operation<'_>, usize, NodeId) + Sync),
    ) -> Result<(), Error> {
        let checkers = self.create_checkers()?;
        let mut associated = Vec::with_capacity(files.len());
        for &file in files {
            associated.push(Self::checker_index_for_file(checkers, file)?);
        }
        let failure = Mutex::new(None);
        let group = WorkGroup::new(single_threaded);
        for (checker_index, owner) in checkers.owners.iter().enumerate() {
            let (failure, associated) = (&failure, &associated);
            group.queue(move || match owner.operation() {
                Ok(mut operation) => {
                    for (position, &file) in files.iter().enumerate() {
                        if associated[position] == checker_index {
                            task(&mut operation, position, file);
                        }
                    }
                }
                Err(error) => {
                    failure
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .get_or_insert(error);
                }
            });
        }
        group.run_and_wait();
        drop(group);
        failure
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
            .map_or(Ok(()), Err)
    }
}

impl CheckerPool for CompilerCheckerPool {
    /// With a file, its associated checker; otherwise the first. The pool
    /// ignores the lifetime.
    // port: tsc/internal/compiler/checkerpool.go:checkerPool.GetChecker
    fn with_checker(
        &self,
        _lifetime: CheckerLifetime,
        file: Option<NodeId>,
        task: &mut dyn FnMut(&mut Operation<'_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut operation = match file {
            Some(file) => self.checker_for_file_exclusive(file)?,
            None => self.checker_non_exclusive()?.operation()?,
        };
        task(&mut operation)
    }
}

struct RetireOnUnwind<'a>(&'a Generation);
impl Drop for RetireOnUnwind<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.retire();
        }
    }
}
