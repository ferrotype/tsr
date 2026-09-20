//! Replay the census's backing-length distribution, not parser construction.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, hint::black_box, num::NonZeroU64, time::Instant};
use tsr_s07_storage_pilot::chunks;
use tsr_s07_storage_pilot::lists::{Builder, NodeRef, Published};

#[cfg(feature = "allocation")]
#[global_allocator]
static ALLOCATOR: cap::Cap<mimalloc::MiMalloc> = cap::Cap::new(mimalloc::MiMalloc, usize::MAX);
#[cfg(not(feature = "allocation"))]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

type Legacy = Vec<Vec<Box<[Option<NonZeroU64>]>>>;
type BorrowedViews<'a> = Vec<Vec<&'a [u32]>>;
enum Roots {
    Legacy(Legacy),
    Page64(Vec<Published<64>>),
    Page256(Vec<Published<256>>),
    Page1024(Vec<Published<1024>>),
    Chunk256(Vec<chunks::Published<256>>),
    Chunk1024(Vec<chunks::Published<1024>>),
    Chunk256Owned(Vec<chunks::Published<256>>),
    Chunk256Borrowed(Vec<chunks::Published<256>>),
}

#[cfg(feature = "allocation")]
#[inline(never)]
fn allocation_preflight() {
    let before_live = ALLOCATOR.allocated();
    let before_requests = ALLOCATOR.total_allocated();
    let zeroed = black_box(vec![0u8; black_box(100_003)]);
    let filled = black_box(vec![7u8; black_box(100_007)]);
    let mut growing = Vec::with_capacity(black_box(31));
    growing.extend_from_slice(&[9u8; 31]);
    growing.reserve_exact(black_box(1_000_009 - growing.len()));
    assert_eq!(growing.capacity(), 1_000_009);
    assert!(zeroed.iter().all(|&byte| byte == 0));
    black_box((&zeroed, &filled, &growing));
    drop((zeroed, filled, growing));
    let requested_bytes = ALLOCATOR.total_allocated() - before_requests;
    let live_after = ALLOCATOR.allocated();
    let expected_bytes = 100_003 + 100_007 + 31 + 1_000_009;
    assert_eq!(requested_bytes, expected_bytes);
    assert_eq!(live_after, before_live);
    println!(
        "{}",
        json!({"version": 1, "preflight": true, "expected_bytes": expected_bytes,
        "requested_bytes": requested_bytes, "live_before": before_live, "live_after": live_after})
    );
}

// The same deterministic values exercise nils and nonnil edges in all variants.
fn word(index: usize) -> u32 {
    u32::try_from(index % 17).expect("small remainder")
}

fn paged<const N: usize>(files: &[Vec<usize>]) -> Vec<Published<N>> {
    files
        .iter()
        .map(|lengths| {
            let mut builder = Builder::<N>::new(1, 16);
            for &length in lengths {
                let frame = builder.begin().expect("frame identity");
                for index in 0..length {
                    let slot = word(index);
                    builder
                        .push(&frame, (slot != 0).then_some(NodeRef { owner: 1, slot }))
                        .expect("replay edge is in its node domain");
                }
                black_box(builder.finish(&frame).expect("completed frame"));
            }
            builder.publish().expect("all replay frames completed")
        })
        .collect()
}

fn chunked<const N: usize>(files: &[Vec<usize>]) -> Vec<chunks::Published<N>> {
    files
        .iter()
        .map(|lengths| {
            let mut builder = chunks::Builder::<N>::new(1, 16);
            for &length in lengths {
                let frame = builder.begin().expect("frame identity");
                for index in 0..length {
                    let slot = word(index);
                    builder
                        .push(
                            &frame,
                            (slot != 0).then_some(chunks::NodeRef { owner: 1, slot }),
                        )
                        .expect("replay edge is in its node domain");
                }
                black_box(builder.finish(&frame).expect("completed frame"));
            }
            builder.publish().expect("all replay frames completed")
        })
        .collect()
}

impl Roots {
    #[inline(never)]
    fn build(mode: &str, files: &[Vec<usize>]) -> Result<Self, &'static str> {
        Ok(match mode {
            "legacy" => Self::Legacy(
                files
                    .iter()
                    .map(|lengths| {
                        let mut backings = Vec::new();
                        for &length in lengths {
                            let mut values = Vec::new();
                            for index in 0..length {
                                values.push(NonZeroU64::new(u64::from(word(index))));
                            }
                            backings.push(values.into_boxed_slice());
                        }
                        backings
                    })
                    .collect(),
            ),
            "page64" => Self::Page64(paged(files)),
            "page256" => Self::Page256(paged(files)),
            "page1024" => Self::Page1024(paged(files)),
            "chunk256" => Self::Chunk256(chunked(files)),
            "chunk1024" => Self::Chunk1024(chunked(files)),
            "chunk256-owned" => Self::Chunk256Owned(chunked(files)),
            "chunk256-borrowed" => Self::Chunk256Borrowed(chunked(files)),
            _ => return Err("unknown list construction/access mode"),
        })
    }
    /// Resolve through the checked public-ID route once. All descriptor vectors
    /// are allocated before construction's timer/counter endpoint and retained
    /// through every sweep. These borrows prevent disposing the underlying roots.
    fn prepare_borrowed_views(&self) -> Option<BorrowedViews<'_>> {
        let Self::Chunk256Borrowed(files) = self else {
            return None;
        };
        let mut owners = Vec::with_capacity(files.len());
        for file in files {
            let mut views = Vec::with_capacity(file.stats().backings);
            file.for_each_backing(|words| views.push(words))
                .expect("published backing range");
            owners.push(views);
        }
        Some(owners)
    }
    #[inline(never)]
    fn checksum(&self) -> u64 {
        let mut checksum = 0u64;
        let mut visit = |word: u32| {
            checksum = checksum.wrapping_mul(31).wrapping_add(u64::from(word));
        };
        match self {
            Self::Legacy(files) => {
                for file in files {
                    for backing in file {
                        for value in backing {
                            visit(value.map_or(0, |value| {
                                u32::try_from(value.get()).expect("replay word")
                            }));
                        }
                    }
                }
            }
            Self::Page64(files) => {
                for file in files {
                    file.for_each_edge(&mut visit);
                }
            }
            Self::Page256(files) => {
                for file in files {
                    file.for_each_edge(&mut visit);
                }
            }
            Self::Page1024(files) => {
                for file in files {
                    file.for_each_edge(&mut visit);
                }
            }
            Self::Chunk256(files) => {
                for file in files {
                    file.for_each_backing(|words| {
                        for &word in words {
                            visit(word);
                        }
                    })
                    .expect("published backing range");
                }
            }
            Self::Chunk1024(files) => {
                for file in files {
                    file.for_each_backing(|words| {
                        for &word in words {
                            visit(word);
                        }
                    })
                    .expect("published backing range");
                }
            }
            Self::Chunk256Owned(files) => {
                for file in files {
                    for words in file.owned_backings() {
                        for &word in words.expect("published backing range") {
                            visit(word);
                        }
                    }
                }
            }
            Self::Chunk256Borrowed(_) => {
                unreachable!("borrowed route uses its charged descriptor cache")
            }
        }
        checksum
    }
    fn storage_stats(&self, borrowed: Option<&BorrowedViews<'_>>) -> Value {
        fn stats<const N: usize>(files: &[Published<N>]) -> Value {
            let mut pages = 0;
            let mut spare_words = 0;
            let mut scratch_words = 0;
            let mut wide = 0;
            for file in files {
                let s = file.stats();
                pages += s.edge_pages;
                spare_words += s.edge_capacity - s.edge_words;
                scratch_words += s.scratch_capacity;
                wide += s.wide_backings;
            }
            json!({"edge_pages": pages, "spare_edge_words": spare_words, "retained_scratch_words": scratch_words, "wide_backings": wide})
        }
        fn chunk_stats<const N: usize>(files: &[chunks::Published<N>]) -> Value {
            let mut chunks = 0;
            let mut spare_words = 0;
            let mut scratch_words = 0;
            for file in files {
                let stats = file.stats();
                chunks += stats.edge_chunks;
                spare_words += stats.edge_capacity - stats.edge_words;
                scratch_words += stats.scratch_capacity;
            }
            json!({"edge_chunks": chunks, "spare_edge_words": spare_words, "retained_scratch_words": scratch_words})
        }
        match self {
            Self::Legacy(_) => json!(null),
            Self::Page64(files) => stats(files),
            Self::Page256(files) => stats(files),
            Self::Page1024(files) => stats(files),
            Self::Chunk256(files) => chunk_stats(files),
            Self::Chunk1024(files) => chunk_stats(files),
            Self::Chunk256Owned(files) | Self::Chunk256Borrowed(files) => {
                let mut stats = chunk_stats(files);
                let count = borrowed.map_or(0, |owners| owners.iter().map(Vec::len).sum::<usize>());
                let bytes = borrowed.map_or(0, |owners| {
                    owners.capacity() * size_of::<Vec<&[u32]>>()
                        + owners
                            .iter()
                            .map(|views| views.capacity() * size_of::<&[u32]>())
                            .sum::<usize>()
                });
                stats["borrowed_view_count"] = json!(count);
                stats["borrowed_view_capacity_bytes"] = json!(bytes);
                stats
            }
        }
    }
}

#[inline(never)]
fn checksum_borrowed(files: &BorrowedViews<'_>) -> u64 {
    let mut checksum = 0u64;
    for file in files {
        for backing in file {
            for &word in *backing {
                checksum = checksum.wrapping_mul(31).wrapping_add(u64::from(word));
            }
        }
    }
    checksum
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    #[cfg(feature = "allocation")]
    if args.len() == 2 && args[1] == "--allocation-preflight" {
        allocation_preflight();
        return Ok(());
    }
    if args.len() != 4 {
        return Err("usage: list-pilot CENSUS.ndjson MODE SWEEPS".into());
    }
    let sweeps: usize = args[3].parse()?;
    if !(1..=100).contains(&sweeps) {
        return Err("sweep count must be 1..100".into());
    }
    let raw = fs::read(&args[1])?;
    let input_hash = format!("{:x}", Sha256::digest(&raw));
    let mut files = Vec::new();
    for (index, line) in raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .enumerate()
    {
        let row: Value = serde_json::from_slice(line)?;
        if row["index"].as_u64() != Some(u64::try_from(index)?) {
            return Err("census file order changed".into());
        }
        let lengths: Vec<usize> =
            serde_json::from_value(row["node_backings"]["physical_lengths_in_aux_order"].clone())?;
        if lengths.iter().any(|len| u32::try_from(*len).is_err()) {
            return Err("list length outside current public domain".into());
        }
        files.push(lengths);
    }
    if files.is_empty() {
        return Err("empty replay".into());
    }
    let backing_count: usize = files.iter().map(Vec::len).sum();
    let edge_count: usize = files.iter().flatten().sum();
    let expected_checksum = files.iter().flatten().fold(0u64, |sum, &len| {
        (0..len).fold(sum, |sum, index| {
            sum.wrapping_mul(31).wrapping_add(u64::from(word(index)))
        })
    });

    #[cfg(feature = "allocation")]
    let (before_requests, before_live) = (ALLOCATOR.total_allocated(), ALLOCATOR.allocated());
    let start = Instant::now();
    let roots = black_box(Roots::build(&args[2], black_box(&files))?);
    let borrowed = black_box(roots.prepare_borrowed_views());
    let construction_ns = start.elapsed().as_nanos();
    #[cfg(feature = "allocation")]
    let (requests, live) = (
        ALLOCATOR.total_allocated() - before_requests,
        ALLOCATOR.allocated() - before_live,
    );
    let start = Instant::now();
    for _ in 0..sweeps {
        let observed = match &borrowed {
            Some(views) => checksum_borrowed(black_box(views)),
            None => black_box(&roots).checksum(),
        };
        if observed != expected_checksum {
            return Err("backing edge/order mismatch".into());
        }
    }
    let traversal_ns = start.elapsed().as_nanos();
    #[cfg(feature = "allocation")]
    let live_after_reads = ALLOCATOR.allocated();
    #[cfg(feature = "allocation")]
    if live_after_reads != before_live + live
        || ALLOCATOR.total_allocated() != before_requests + requests
    {
        return Err("read-only backing traversal allocated".into());
    }
    let stats = roots.storage_stats(borrowed.as_ref());
    // Stats JSON owns diagnostic allocations; record its cost before checking
    // whether dropping the actual storage returns to the starting endpoint.
    #[cfg(feature = "allocation")]
    let stats_live = ALLOCATOR.allocated() - live_after_reads;
    drop(borrowed);
    drop(roots);
    #[cfg(feature = "allocation")]
    if ALLOCATOR.allocated() - stats_live != before_live {
        return Err("replay storage remains allocated after drop".into());
    }
    #[cfg(feature = "allocation")]
    let allocation = json!({"requested_bytes": requests, "retained_requested_bytes": live, "freed_or_superseded_requests": requests - live, "drop_returns_to_start": true});
    #[cfg(not(feature = "allocation"))]
    let allocation = Value::Null;
    let timing = if cfg!(feature = "allocation") {
        Value::Null
    } else {
        json!({"construction_ns": construction_ns, "traversal_ns": traversal_ns, "sweeps": sweeps})
    };
    println!(
        "{}",
        json!({"version": 1, "diagnostic_only": true, "mode": args[2],
        "scope": "physical backing-length/order replay with synthetic edge values; excludes parser, AST nodes, list headers, imports and binding",
        "input_sha256": input_hash, "files": files.len(), "backings": backing_count, "edges": edge_count,
        "checksum": expected_checksum, "allocation": allocation, "timing": timing, "storage": stats})
    );
    Ok(())
}
