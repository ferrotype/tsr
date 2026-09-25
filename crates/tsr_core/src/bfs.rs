//! `core.BreadthFirstSearchParallel` and its level type.
//!
//! Ports of `tsc/internal/core/bfs.go`, witnessed by the `concurrency` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).
use crate::collections::OrderedMap;
use std::collections::HashSet;
use std::hash::Hash;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

/// Go's `breadthFirstSearchJob`: a node and the job that queued it.
#[derive(Debug)]
pub struct Job<N> {
    node: N,
    parent: Option<Arc<Job<N>>>,
}

/// Go's `BreadthFirstSearchLevel`: the jobs of one level, which
/// `PreprocessLevel` may inspect and prune before they run.
pub struct BreadthFirstSearchLevel<'a, K, N> {
    jobs: &'a mut OrderedMap<K, Arc<Job<N>>>,
}

impl<K: Eq + Hash + Clone, N> BreadthFirstSearchLevel<'_, K, N> {
    /// port: tsc/internal/core/bfs.go:BreadthFirstSearchLevel.Has
    pub fn has(&self, key: &K) -> bool {
        self.jobs.contains_key(key)
    }

    /// port: tsc/internal/core/bfs.go:BreadthFirstSearchLevel.Delete
    pub fn delete(&mut self, key: &K) {
        self.jobs.remove(key);
    }

    /// Calls `f` with each job's node in level order until it returns false.
    /// port: tsc/internal/core/bfs.go:BreadthFirstSearchLevel.Range
    pub fn range(&self, f: &mut dyn FnMut(&N) -> bool) {
        for job in self.jobs.values() {
            if !f(&job.node) {
                return;
            }
        }
    }
}

/// Go's `collections.SyncSet[K]`, the visited set a search may share.
#[derive(Debug)]
pub struct VisitedSet<K> {
    keys: Mutex<HashSet<K>>,
}

impl<K> Default for VisitedSet<K> {
    fn default() -> Self {
        Self {
            keys: Mutex::new(HashSet::new()),
        }
    }
}

impl<K: Eq + Hash> VisitedSet<K> {
    pub fn add_if_absent(&self, key: K) -> bool {
        self.keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key)
    }
}

/// Go's `PreprocessLevel` callback.
pub type PreprocessLevel<'a, K, N> = &'a dyn Fn(&mut BreadthFirstSearchLevel<'_, K, N>);

/// Go's `BreadthFirstSearchOptions`.
pub struct BreadthFirstSearchOptions<'a, K, N> {
    pub visited: Option<Arc<VisitedSet<K>>>,
    pub preprocess_level: Option<PreprocessLevel<'a, K, N>>,
}

impl<K, N> Default for BreadthFirstSearchOptions<'_, K, N> {
    fn default() -> Self {
        Self {
            visited: None,
            preprocess_level: None,
        }
    }
}

/// Go's `updateMin`: lowers `a` to `candidate`, reporting whether it did.
/// port: tsc/internal/core/bfs.go:updateMin
pub fn update_min(a: &AtomicI64, candidate: i64) -> bool {
    loop {
        let current = a.load(Ordering::SeqCst);
        if current < candidate {
            return false;
        }
        if a.compare_exchange(current, candidate, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return true;
        }
    }
}

/// Go's `BreadthFirstSearchResult` as `(stopped, path)`, the path from the
/// found node back to the start.
pub type BreadthFirstSearchResult<N> = (bool, Vec<N>);

/// port: tsc/internal/core/bfs.go:BreadthFirstSearchParallel
pub fn breadth_first_search_parallel<N: Clone + Eq + Hash + Send + Sync>(
    start: N,
    neighbors: &(dyn Fn(&N) -> Vec<N> + Sync),
    visit: &(dyn Fn(&N) -> (bool, bool) + Sync),
) -> (bool, Vec<N>) {
    breadth_first_search_parallel_ex(
        start,
        neighbors,
        visit,
        &BreadthFirstSearchOptions::default(),
        &|node: &N| node.clone(),
    )
}

fn create_path<N: Clone>(job: Option<&Job<N>>) -> Vec<N> {
    let mut path = Vec::new();
    let mut job = job;
    while let Some(current) = job {
        path.push(current.node.clone());
        job = current.parent.as_deref();
    }
    path
}

/// The outcome of one level: a job to stop at, or the next level.
struct LevelResult<K, N> {
    stop: bool,
    job: Option<Arc<Job<N>>>,
    next: OrderedMap<K, Arc<Job<N>>>,
}

/// Go's `BreadthFirstSearchParallelEx`: each level's jobs run on their own
/// threads, joined before the level's result is chosen. The port marker is
/// on the stop test, a site the mutation splicer can negate.
pub fn breadth_first_search_parallel_ex<
    K: Clone + Eq + Hash + Send + Sync,
    N: Clone + Send + Sync,
>(
    start: N,
    neighbors: &(dyn Fn(&N) -> Vec<N> + Sync),
    visit: &(dyn Fn(&N) -> (bool, bool) + Sync),
    options: &BreadthFirstSearchOptions<'_, K, N>,
    get_key: &(dyn Fn(&N) -> K + Sync),
) -> (bool, Vec<N>) {
    let visited = options.visited.clone().unwrap_or_default();
    let mut fallback: Option<Arc<Job<N>>> = None;
    let process_level = |jobs: &mut OrderedMap<K, Arc<Job<N>>>,
                         fallback: &Option<Arc<Job<N>>>|
     -> LevelResult<K, N> {
        let lowest_fallback = AtomicI64::new(i64::MAX);
        let lowest_goal = AtomicI64::new(i64::MAX);
        let next_job_count = AtomicI64::new(0);
        if let Some(preprocess) = options.preprocess_level {
            preprocess(&mut BreadthFirstSearchLevel { jobs });
        }
        let has_fallback = fallback.is_some();
        let next: Vec<Mutex<Vec<Arc<Job<N>>>>> =
            (0..jobs.len()).map(|_| Mutex::new(Vec::new())).collect();
        std::thread::scope(|scope| {
            for (i, job) in jobs.values().enumerate() {
                let (lowest_goal, lowest_fallback, next_job_count, next, visited) = (
                    &lowest_goal,
                    &lowest_fallback,
                    &next_job_count,
                    &next,
                    &visited,
                );
                let i = i64::try_from(i).expect("level index");
                scope.spawn(move || {
                    if i >= lowest_goal.load(Ordering::SeqCst) {
                        return;
                    }
                    if !visited.add_if_absent(get_key(&job.node)) {
                        return;
                    }
                    let (is_result, stop) = visit(&job.node);
                    if is_result {
                        if stop {
                            update_min(lowest_goal, i);
                            return;
                        }
                        if !has_fallback {
                            update_min(lowest_fallback, i);
                        }
                    }
                    if i >= lowest_goal.load(Ordering::SeqCst) {
                        return;
                    }
                    let neighbor_nodes = neighbors(&job.node);
                    if !neighbor_nodes.is_empty() {
                        next_job_count.fetch_add(
                            i64::try_from(neighbor_nodes.len()).expect("count"),
                            Ordering::SeqCst,
                        );
                        *next[usize::try_from(i).expect("index")]
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = neighbor_nodes
                            .into_iter()
                            .map(|child| {
                                Arc::new(Job {
                                    node: child,
                                    parent: Some(job.clone()),
                                })
                            })
                            .collect();
                    }
                });
            }
        });
        let goal = lowest_goal.load(Ordering::SeqCst);
        if goal != i64::MAX {
            let job = jobs
                .entry_at(isize::try_from(goal).expect("index"))
                .map(|(_, job)| job.clone());
            return LevelResult {
                stop: true,
                job,
                next: OrderedMap::default(),
            };
        }
        let mut found = None;
        if !has_fallback {
            let index = lowest_fallback.load(Ordering::SeqCst);
            if index != i64::MAX {
                found = jobs
                    .entry_at(isize::try_from(index).expect("index"))
                    .map(|(_, job)| job.clone());
            }
        }
        let mut next_jobs = OrderedMap::with_capacity(
            usize::try_from(next_job_count.load(Ordering::SeqCst)).unwrap_or(0),
        );
        for level in next {
            for job in level
                .into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
            {
                let key = get_key(&job.node);
                if !next_jobs.contains_key(&key) {
                    next_jobs.insert(key, job);
                }
            }
        }
        LevelResult {
            stop: false,
            job: found,
            next: next_jobs,
        }
    };
    let mut level = OrderedMap::default();
    level.insert(
        get_key(&start),
        Arc::new(Job {
            node: start,
            parent: None,
        }),
    );
    while !level.is_empty() {
        let result = process_level(&mut level, &fallback);
        // port: tsc/internal/core/bfs.go:BreadthFirstSearchParallelEx
        if result.stop {
            return (true, create_path(result.job.as_deref()));
        }
        if result.job.is_some() && fallback.is_none() {
            fallback = result.job;
        }
        level = result.next;
    }
    (false, create_path(fallback.as_deref()))
}
