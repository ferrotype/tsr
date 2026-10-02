//! The patience diff the pinned `baseline.DiffText` composes for the `noCheck`
//! repeat's `differs from original emit` block: `github.com/peter-evans/patience`
//! v0.3.0 (the pin's `go.mod`), restricted to `Diff` and
//! `UnifiedDiffTextWithOptions`. Lines are bytes; nothing is decoded.

/// Go's `patience.DiffType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffType {
    Delete,
    Insert,
    Equal,
}

/// Go's `patience.DiffLine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffLine<'a> {
    pub text: &'a [u8],
    pub kind: DiffType,
}

// source: github.com/peter-evans/patience@v0.3.0/patience.go:toDiffLines
fn to_diff_lines<'a>(lines: &[&'a [u8]], kind: DiffType) -> Vec<DiffLine<'a>> {
    lines.iter().map(|&text| DiffLine { text, kind }).collect()
}

// source: github.com/peter-evans/patience@v0.3.0/patience.go:uniqueElements
fn unique_elements<'a>(lines: &[&'a [u8]]) -> (Vec<&'a [u8]>, Vec<usize>) {
    let mut counts: std::collections::HashMap<&[u8], usize> = std::collections::HashMap::new();
    for &line in lines {
        *counts.entry(line).or_default() += 1;
    }
    let mut elements = Vec::new();
    let mut indices = Vec::new();
    for (index, &line) in lines.iter().enumerate() {
        if counts[line] == 1 {
            elements.push(line);
            indices.push(index);
        }
    }
    (elements, indices)
}

// source: github.com/peter-evans/patience@v0.3.0/lcs.go:LCS
fn lcs(a: &[&[u8]], b: &[&[u8]]) -> Vec<[usize; 2]> {
    let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in 1..table.len() {
        for j in 1..table[i].len() {
            table[i][j] = if a[i - 1] == b[j - 1] {
                table[i - 1][j - 1] + 1
            } else {
                table[i - 1][j].max(table[i][j - 1])
            };
        }
    }
    let (mut i, mut j) = (a.len(), b.len());
    let mut pairs = Vec::with_capacity(table[i][j]);
    while i > 0 && j > 0 {
        if a[i - 1] == b[j - 1] {
            pairs.push([i - 1, j - 1]);
            i -= 1;
            j -= 1;
        } else if table[i - 1][j] > table[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    pairs.reverse();
    pairs
}

/// The patience diff of two line lists.
// source: github.com/peter-evans/patience@v0.3.0/patience.go:Diff
pub fn diff<'a>(a: &[&'a [u8]], b: &[&'a [u8]]) -> Vec<DiffLine<'a>> {
    if a.is_empty() && b.is_empty() {
        return Vec::new();
    }
    if a.is_empty() {
        return to_diff_lines(b, DiffType::Insert);
    }
    if b.is_empty() {
        return to_diff_lines(a, DiffType::Delete);
    }

    // Find equal elements at the head of slices a and b.
    let mut i = 0;
    while i < a.len() && i < b.len() && a[i] == b[i] {
        i += 1;
    }
    if i > 0 {
        let mut head = to_diff_lines(&a[..i], DiffType::Equal);
        head.extend(diff(&a[i..], &b[i..]));
        return head;
    }

    // Find equal elements at the tail of slices a and b.
    let mut j = 0;
    while j < a.len() && j < b.len() && a[a.len() - 1 - j] == b[b.len() - 1 - j] {
        j += 1;
    }
    if j > 0 {
        let mut body = diff(&a[..a.len() - j], &b[..b.len() - j]);
        body.extend(to_diff_lines(&a[a.len() - j..], DiffType::Equal));
        return body;
    }

    // Find the longest common subsequence of unique elements in a and b.
    let (unique_a, indices_a) = unique_elements(a);
    let (unique_b, indices_b) = unique_elements(b);
    let mut pairs = lcs(&unique_a, &unique_b);

    // If the LCS is empty, the diff is all deletions and insertions.
    if pairs.is_empty() {
        let mut all = to_diff_lines(a, DiffType::Delete);
        all.extend(to_diff_lines(b, DiffType::Insert));
        return all;
    }

    // Lookup the original indices of slices a and b.
    for pair in &mut pairs {
        *pair = [indices_a[pair[0]], indices_b[pair[1]]];
    }

    let mut diffs = Vec::new();
    let (mut gap_a, mut gap_b) = (0, 0);
    for [index_a, index_b] in pairs {
        // Diff the gaps between the lcs elements.
        diffs.extend(diff(&a[gap_a..index_a], &b[gap_b..index_b]));
        // Append the LCS elements to the diff.
        diffs.push(DiffLine {
            text: a[index_a],
            kind: DiffType::Equal,
        });
        gap_a = index_a + 1;
        gap_b = index_b + 1;
    }
    // Diff the remaining elements of a and b after the final LCS element.
    diffs.extend(diff(&a[gap_a..], &b[gap_b..]));
    diffs
}

/// Go's `patience.Hunk`. Line numbers are Go `int`s.
#[derive(Clone, Debug, Default)]
struct Hunk<'a> {
    diffs: Vec<DiffLine<'a>>,
    src_start: usize,
    src_lines: usize,
    dst_start: usize,
    dst_lines: usize,
}

// source: github.com/peter-evans/patience@v0.3.0/unified.go:makeHunks
fn make_hunks<'a>(diffs: &[DiffLine<'a>], precontext: usize, postcontext: usize) -> Vec<Hunk<'a>> {
    if diffs.is_empty() {
        return Vec::new();
    }
    let mut hunks: Vec<Hunk<'a>> = Vec::new();

    // Update hunks with a diff block.
    let update_hunks = |hunks: &mut Vec<Hunk<'a>>, block: &Hunk<'a>, last_block: bool| {
        if block.diffs[0].kind == DiffType::Equal {
            // Unmodified block.
            if hunks.is_empty() {
                // Start a new hunk with the tail of the block.
                let context = precontext.min(block.diffs.len());
                hunks.push(Hunk {
                    diffs: block.diffs[block.diffs.len() - context..].to_vec(),
                    src_start: block.diffs.len() - context + block.src_start,
                    src_lines: context,
                    dst_start: block.diffs.len() - context + block.dst_start,
                    dst_lines: context,
                });
            } else {
                let current = hunks.len() - 1;
                // Update the current hunk.
                let max_non_context = if last_block {
                    postcontext
                } else {
                    precontext + postcontext
                };
                if block.diffs.len() <= max_non_context {
                    // Block is small enough to be appended to the current hunk.
                    hunks[current].diffs.extend_from_slice(&block.diffs);
                    hunks[current].src_lines += block.diffs.len();
                    hunks[current].dst_lines += block.diffs.len();
                } else {
                    // Append the head of the block to the current hunk.
                    hunks[current]
                        .diffs
                        .extend_from_slice(&block.diffs[..postcontext]);
                    hunks[current].src_lines += postcontext;
                    hunks[current].dst_lines += postcontext;
                    if !last_block {
                        // Start a new hunk with the tail of the block.
                        hunks.push(Hunk {
                            diffs: block.diffs[block.diffs.len() - precontext..].to_vec(),
                            src_start: block.diffs.len() - precontext + block.src_start,
                            src_lines: precontext,
                            dst_start: block.diffs.len() - precontext + block.dst_start,
                            dst_lines: precontext,
                        });
                    }
                }
                // Update starting line numbers if the current hunk had no source or destination diff.
                if hunks[current].src_start == 0 {
                    hunks[current].src_start = block.src_start;
                }
                if hunks[current].dst_start == 0 {
                    hunks[current].dst_start = block.dst_start;
                }
            }
        } else if let Some(current) = hunks.last_mut() {
            // Modified block.
            current.diffs.extend_from_slice(&block.diffs);
            current.src_lines += block.src_lines;
            current.dst_lines += block.dst_lines;
        } else {
            hunks.push(block.clone());
        }
    };

    // Aggregate blocks of modified and unmodified diff lines, creating
    // or updating hunks after each block.
    let mut block = Hunk::default();
    let mut modified_lines = 0;
    let (mut src_line_number, mut dst_line_number) = (0, 0);
    for &line in diffs {
        if block.diffs.is_empty()
            || block.diffs[0].kind == line.kind
            || (block.diffs[0].kind != DiffType::Equal && line.kind != DiffType::Equal)
        {
            block.diffs.push(line);
        } else {
            update_hunks(&mut hunks, &block, false);
            block = Hunk {
                diffs: vec![line],
                ..Hunk::default()
            };
        }

        match line.kind {
            DiffType::Delete => {
                src_line_number += 1;
                block.src_lines += 1;
                modified_lines += 1;
            }
            DiffType::Insert => {
                dst_line_number += 1;
                block.dst_lines += 1;
                modified_lines += 1;
            }
            DiffType::Equal => {
                src_line_number += 1;
                dst_line_number += 1;
                block.src_lines += 1;
                block.dst_lines += 1;
            }
        }

        if block.src_start == 0 && matches!(line.kind, DiffType::Equal | DiffType::Delete) {
            block.src_start = src_line_number;
        }
        if block.dst_start == 0 && matches!(line.kind, DiffType::Equal | DiffType::Insert) {
            block.dst_start = dst_line_number;
        }
    }
    update_hunks(&mut hunks, &block, true);

    // Return no hunks if the diffs contain only equal lines.
    if modified_lines == 0 {
        return Vec::new();
    }
    hunks
}

// source: github.com/peter-evans/patience@v0.3.0/format.go:typeSymbol
fn type_symbol(kind: DiffType) -> &'static [u8] {
    match kind {
        DiffType::Equal => b" ",
        DiffType::Insert => b"+",
        DiffType::Delete => b"-",
    }
}

/// Go's `patience.UnifiedDiffOptions`.
pub struct UnifiedDiffOptions<'a> {
    pub precontext: usize,
    pub postcontext: usize,
    pub src_header: &'a [u8],
    pub dst_header: &'a [u8],
}

// source: github.com/peter-evans/patience@v0.3.0/format.go:UnifiedDiffTextWithOptions
pub fn unified_diff_text_with_options(
    diffs: &[DiffLine<'_>],
    opts: &UnifiedDiffOptions<'_>,
) -> Vec<u8> {
    let hunks = make_hunks(diffs, opts.precontext, opts.postcontext);
    let mut lines: Vec<Vec<u8>> = Vec::new();
    if !opts.src_header.is_empty() {
        lines.push([b"--- ".as_slice(), opts.src_header].concat());
    }
    if !opts.dst_header.is_empty() {
        lines.push([b"+++ ".as_slice(), opts.dst_header].concat());
    }
    for hunk in &hunks {
        lines.push(
            format!(
                "@@ -{},{} +{},{} @@",
                hunk.src_start, hunk.src_lines, hunk.dst_start, hunk.dst_lines
            )
            .into_bytes(),
        );
        for line in &hunk.diffs {
            if line.kind == DiffType::Equal && line.text.is_empty() {
                lines.push(Vec::new());
            } else {
                lines.push([type_symbol(line.kind), line.text].concat());
            }
        }
    }
    lines.join(b"\n".as_slice())
}

/// The pinned `baseline.DiffText`: a three-line-context unified patience diff
/// of the two texts split by `stringutil.SplitLines`.
// source: tsc/internal/testutil/baseline/baseline.go:DiffText
pub fn diff_text(old_name: &[u8], new_name: &[u8], expected: &[u8], actual: &[u8]) -> Vec<u8> {
    let expected = tsr_jsstring::text::split_lines(expected);
    let actual = tsr_jsstring::text::split_lines(actual);
    let lines = diff(&expected, &actual);
    unified_diff_text_with_options(
        &lines,
        &UnifiedDiffOptions {
            precontext: 3,
            postcontext: 3,
            src_header: old_name,
            dst_header: new_name,
        },
    )
}
