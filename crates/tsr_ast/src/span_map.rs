//! Bidirectional span mapping between a content mapper's virtual text and its
//! original source: explicit segments for the virtual parts that correspond to
//! the original, every other virtual position synthesized. A missing map
//! (`None`, Go's nil `*SpanMap`) maps identically; an empty map marks every
//! position as synthesized.
use crate::SpanSegment;
use std::cmp::Ordering;
use std::sync::OnceLock;
use tsr_core::TextRange;

/// Length-preserving: interior positions map 1:1.
pub const KIND_VERBATIM: i32 = 0;
/// Maps a virtual span to an original span as a whole.
pub const KIND_ATOM: i32 = 1;
/// Atom geometry naming the same entity; diagnostics may show the original.
pub const KIND_ALIAS: i32 = 2;

pub const FEATURE_HOVER: i32 = 1;
pub const FEATURE_SIGNATURE_HELP: i32 = 1 << 1;
pub const FEATURE_COMPLETION: i32 = 1 << 2;
pub const FEATURE_DEFINITION: i32 = 1 << 3;
pub const FEATURE_TYPE_DEFINITION: i32 = 1 << 4;
pub const FEATURE_IMPLEMENTATION: i32 = 1 << 5;
pub const FEATURE_REFERENCES: i32 = 1 << 6;
pub const FEATURE_DOCUMENT_HIGHLIGHTS: i32 = 1 << 7;
pub const FEATURE_RENAME: i32 = 1 << 8;
pub const FEATURE_CALL_HIERARCHY: i32 = 1 << 9;
pub const FEATURE_CODE_ACTIONS: i32 = 1 << 10;
pub const FEATURE_FORMATTING: i32 = 1 << 11;
pub const FEATURE_INLAY_HINTS: i32 = 1 << 12;
pub const FEATURE_SEMANTIC_TOKENS: i32 = 1 << 13;
pub const FEATURE_FOLDING_RANGES: i32 = 1 << 14;
pub const FEATURE_SELECTION_RANGES: i32 = 1 << 15;
pub const FEATURE_LINKED_EDITING: i32 = 1 << 16;
pub const FEATURE_AUTO_INSERT: i32 = 1 << 17;
pub const FEATURE_DOCUMENT_SYMBOLS: i32 = 1 << 18;
pub const FEATURE_CODE_LENS: i32 = 1 << 19;
pub const FEATURE_NONE: i32 = 0;
pub const FEATURE_ALL: i32 = SpanSegment::FEATURE_ALL;

/// How faithfully a mapped span reflects the original.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fidelity {
    /// Within one verbatim segment: maps precisely.
    Exact,
    /// Within one atom segment: maps to the atom's span.
    Atom,
    /// Across segment boundaries: endpoints mapped and clamped.
    Approximate,
    /// Entirely synthesized.
    None,
}

impl Fidelity {
    /// port: tsc/internal/spanmap/spanmap.go:Fidelity.IsExact
    pub fn is_exact(self) -> bool {
        self == Self::Exact
    }
    /// port: tsc/internal/spanmap/spanmap.go:Fidelity.IsSingleSegment
    pub fn is_single_segment(self) -> bool {
        matches!(self, Self::Exact | Self::Atom)
    }
    /// port: tsc/internal/spanmap/spanmap.go:Fidelity.IsNone
    pub fn is_none(self) -> bool {
        self == Self::None
    }
}

/// One virtual projection of an original position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MappedPosition {
    pub position: i32,
    pub fidelity: Fidelity,
}

/// One virtual projection of an original range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MappedSpan {
    pub span: TextRange,
    pub fidelity: Fidelity,
}

/// The ways a content mapper's span map can be malformed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingErrorKind {
    Overlap,
    OutOfBounds,
    VerbatimMismatch,
    Kind,
    Feature,
}

/// One validation failure, with the offsets that locate it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MappingError {
    pub kind: MappingErrorKind,
    pub virtual_pos: i32,
    pub original_pos: i32,
}

impl std::fmt::Display for MappingError {
    /// port: tsc/internal/spanmap/spanmap.go:MappingError.Error
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            MappingErrorKind::Overlap => write!(
                f,
                "content mapper position mappings overlap or are out of order near virtual offset {}",
                self.virtual_pos
            ),
            MappingErrorKind::OutOfBounds => write!(
                f,
                "content mapper position mapping points outside the original content at original offset {}",
                self.original_pos
            ),
            MappingErrorKind::VerbatimMismatch => write!(
                f,
                "content mapper verbatim mapping does not match the original content at virtual offset {}, original offset {}",
                self.virtual_pos, self.original_pos
            ),
            MappingErrorKind::Kind => write!(
                f,
                "content mapper position mapping has an invalid kind at virtual offset {}",
                self.virtual_pos
            ),
            MappingErrorKind::Feature => write!(
                f,
                "content mapper position mappings have invalid features near original offset {}",
                self.original_pos
            ),
        }
    }
}

impl std::error::Error for MappingError {}

/// A sparse, ordered set of segments over a content mapper's virtual text.
#[derive(Debug, Default)]
pub struct SpanMap {
    segments: Vec<SpanSegment>,
    /// The original-order interval index, built on the first reverse lookup.
    original_index: OnceLock<OriginalIndex>,
}

impl Clone for SpanMap {
    fn clone(&self) -> Self {
        Self {
            segments: self.segments.clone(),
            original_index: OnceLock::new(),
        }
    }
}

impl PartialEq for SpanMap {
    fn eq(&self, other: &Self) -> bool {
        self.segments == other.segments
    }
}

impl Eq for SpanMap {}

/// port: tsc/internal/spanmap/spanmap.go:clamp
fn clamp(value: i32, low: i32, high: i32) -> i32 {
    value.min(high).max(low)
}

/// port: tsc/internal/spanmap/spanmap.go:supportsFeature
fn supports_feature(segment: &SpanSegment, feature: i32) -> bool {
    segment.features & feature != 0
}

/// Go's `int(a - b)` comparison of two `TextPos` values.
fn compare_positions(a: i32, b: i32) -> Ordering {
    a.wrapping_sub(b).cmp(&0)
}

impl SpanMap {
    /// A span map over `segments` sorted by virtual start.
    /// port: tsc/internal/spanmap/spanmap.go:New
    pub fn new(segments: &[SpanSegment]) -> Self {
        let mut segments = segments.to_vec();
        tsr_core::sort_like_go(&mut segments, &mut |a, b| {
            compare_positions(a.virtual_start, b.virtual_start)
        });
        Self {
            segments,
            original_index: OnceLock::new(),
        }
    }

    /// The segments in virtual order.
    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.Segments
    pub fn segments(&self) -> &[SpanSegment] {
        &self.segments
    }

    /// The first violation of the content-mapper span map contract against
    /// the virtual and original texts, if any.
    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.Validate
    pub fn validate(&self, virtual_text: &[u8], original: &[u8]) -> Option<MappingError> {
        let length = |text: &[u8]| i32::try_from(text.len()).unwrap_or(i32::MAX);
        let (virtual_length, original_length) = (length(virtual_text), length(original));
        let mut previous_virtual_end = 0;
        for segment in &self.segments {
            let error = |kind, original_pos| {
                Some(MappingError {
                    kind,
                    virtual_pos: segment.virtual_start,
                    original_pos,
                })
            };
            if segment.virtual_start < previous_virtual_end
                || segment.virtual_end < segment.virtual_start
                || segment.virtual_end > virtual_length
            {
                return error(MappingErrorKind::Overlap, 0);
            }
            previous_virtual_end = segment.virtual_end;
            if segment.original_start < 0
                || segment.original_end < segment.original_start
                || segment.original_end > original_length
            {
                return error(MappingErrorKind::OutOfBounds, segment.original_end);
            }
            if !matches!(segment.kind, KIND_VERBATIM | KIND_ATOM | KIND_ALIAS) {
                return error(MappingErrorKind::Kind, segment.original_start);
            }
            if segment.kind == KIND_VERBATIM {
                let range = |start: i32, end: i32| start as usize..end as usize;
                if segment.virtual_end.wrapping_sub(segment.virtual_start)
                    != segment.original_end.wrapping_sub(segment.original_start)
                    || virtual_text[range(segment.virtual_start, segment.virtual_end)]
                        != original[range(segment.original_start, segment.original_end)]
                {
                    return error(MappingErrorKind::VerbatimMismatch, segment.original_start);
                }
            }
            if segment.features & !FEATURE_ALL != 0 {
                return error(MappingErrorKind::Feature, segment.original_start);
            }
        }
        None
    }

    /// The index of the segment containing `pos`, or of the segment just
    /// before a gap containing it. Segment starts, including zero-length
    /// segments, are contained; the final segment's end is contained too.
    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.segmentIndexAt
    fn segment_index_at(&self, pos: i32) -> (Option<usize>, bool) {
        segment_at(&self.segments, pos)
    }

    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.insertionPoint
    fn insertion_point(&self, previous: Option<usize>) -> i32 {
        previous.map_or(0, |index| self.segments[index].original_end)
    }

    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.mapLow
    fn map_low(&self, pos: i32, index: Option<usize>, inside: bool) -> i32 {
        if !inside {
            return self.insertion_point(index);
        }
        let segment = &self.segments[index.expect("contained segment")];
        if segment.kind == KIND_VERBATIM {
            verbatim_position(segment, pos)
        } else {
            segment.original_start
        }
    }

    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.mapHigh
    fn map_high(&self, pos: i32, index: Option<usize>, inside: bool) -> i32 {
        if !inside {
            return self.insertion_point(index);
        }
        let segment = &self.segments[index.expect("contained segment")];
        if segment.kind == KIND_VERBATIM {
            verbatim_position(segment, pos)
        } else {
            segment.original_end
        }
    }

    /// Maps a virtual range to an original range with its fidelity.
    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.VirtualToOriginalSpan
    pub fn virtual_to_original_span(&self, range: TextRange) -> (TextRange, Fidelity) {
        let (start, end) = virtual_bounds(range);
        if start == end {
            let (position, fidelity) = self.virtual_to_original_position(start);
            return (point(position), fidelity);
        }
        let (start_index, start_inside) = self.segment_index_at(start);
        let (end_index, end_inside) = self.segment_index_at(end.wrapping_sub(1));
        if start_index == end_index && start_inside == end_inside {
            if start_inside {
                let segment = &self.segments[start_index.expect("contained segment")];
                if segment.kind == KIND_VERBATIM {
                    let low = verbatim_position(segment, start);
                    let high = clamp(
                        segment
                            .original_start
                            .wrapping_add(end.wrapping_sub(segment.virtual_start)),
                        low,
                        segment.original_end,
                    );
                    return (range_of(low, high), Fidelity::Exact);
                }
                return (
                    range_of(segment.original_start, segment.original_end),
                    Fidelity::Atom,
                );
            }
            return (point(self.insertion_point(start_index)), Fidelity::None);
        }
        let low = self.map_low(start, start_index, start_inside);
        let high = self.map_high(end, end_index, end_inside).max(low);
        (range_of(low, high), Fidelity::Approximate)
    }

    /// Maps `range` only when every virtual position is covered by contiguous
    /// segments that participate in `feature`.
    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.VirtualToOriginalSpanForFeature
    pub fn virtual_to_original_span_for_feature(
        &self,
        range: TextRange,
        feature: i32,
    ) -> (TextRange, Fidelity) {
        let (mapped, fidelity) = self.virtual_to_original_span(range);
        if self.virtual_span_supports_feature(range, feature) {
            (mapped, fidelity)
        } else {
            (mapped, Fidelity::None)
        }
    }

    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.virtualSpanSupportsFeature
    fn virtual_span_supports_feature(&self, range: TextRange, feature: i32) -> bool {
        let (start, end) = virtual_bounds(range);
        let (index, inside) = self.segment_index_at(start);
        if start == end {
            return inside && supports_feature(&self.segments[index.expect("contained")], feature);
        }
        if !inside {
            return false;
        }
        let mut index = index.expect("contained segment");
        let mut covered_through = start;
        while index < self.segments.len() && covered_through < end {
            let segment = &self.segments[index];
            if segment.virtual_start > covered_through
                || segment.virtual_end <= covered_through
                || !supports_feature(segment, feature)
            {
                return false;
            }
            covered_through = segment.virtual_end;
            index += 1;
        }
        covered_through >= end
    }

    /// Maps one virtual position with its fidelity.
    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.VirtualToOriginalPosition
    pub fn virtual_to_original_position(&self, pos: i32) -> (i32, Fidelity) {
        let (index, inside) = self.segment_index_at(pos);
        if !inside {
            return (self.insertion_point(index), Fidelity::None);
        }
        let segment = &self.segments[index.expect("contained segment")];
        if segment.kind == KIND_VERBATIM {
            (verbatim_position(segment, pos), Fidelity::Exact)
        } else {
            (segment.original_start, Fidelity::Atom)
        }
    }

    /// Maps a position only when it is unambiguously in verbatim content.
    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.VirtualToOriginalPositionExact
    pub fn virtual_to_original_position_exact(&self, pos: i32) -> (i32, bool) {
        let (mapped, fidelity) = self.virtual_to_original_position(pos);
        if fidelity != Fidelity::Exact {
            return (mapped, false);
        }
        let (index, inside) = self.segment_index_at(pos);
        let Some(index) =
            index.filter(|&index| inside && self.segments[index].kind == KIND_VERBATIM)
        else {
            return (mapped, false);
        };
        if index > 0 {
            let previous = &self.segments[index - 1];
            if previous.virtual_end == pos
                && (previous.kind != KIND_VERBATIM
                    || previous.original_end != self.segments[index].original_start)
            {
                return (mapped, false);
            }
        }
        (mapped, true)
    }

    /// Maps `pos` only when its virtual segment participates in `feature`.
    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.VirtualToOriginalPositionForFeature
    pub fn virtual_to_original_position_for_feature(
        &self,
        pos: i32,
        feature: i32,
    ) -> (i32, Fidelity) {
        let (mapped, fidelity) = self.virtual_to_original_position(pos);
        let (index, inside) = self.segment_index_at(pos);
        match index {
            Some(index) if inside && supports_feature(&self.segments[index], feature) => {
                (mapped, fidelity)
            }
            _ => (mapped, Fidelity::None),
        }
    }

    /// The alias segment exactly covering `range`.
    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.AliasForVirtualSpan
    pub fn alias_for_virtual_span(&self, range: TextRange) -> Option<SpanSegment> {
        // Go converts the position to a TextPos (int32).
        #[allow(clippy::cast_possible_truncation)]
        let (index, inside) = self.segment_index_at(range.pos() as i32);
        if !inside {
            return None;
        }
        let segment = self.segments[index.expect("contained segment")];
        (segment.kind == KIND_ALIAS
            && range.pos() == i64::from(segment.virtual_start)
            && range.end() == i64::from(segment.virtual_end))
        .then_some(segment)
    }

    /// Every virtual projection of an original position through segments
    /// participating in `feature`, in virtual order.
    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.OriginalToVirtualPositions
    pub fn original_to_virtual_positions(&self, pos: i32, feature: i32) -> Vec<MappedPosition> {
        let groups = self
            .original_index()
            .segment_groups_at_original_position(pos);
        let mut results: Vec<MappedPosition> = Vec::new();
        for group in groups {
            for segment in group.segments {
                if !supports_feature(&segment, feature) {
                    continue;
                }
                let mapped = if segment.kind == KIND_VERBATIM {
                    MappedPosition {
                        position: clamp(
                            segment
                                .virtual_start
                                .wrapping_add(pos.wrapping_sub(segment.original_start)),
                            segment.virtual_start,
                            segment.virtual_end,
                        ),
                        fidelity: Fidelity::Exact,
                    }
                } else {
                    MappedPosition {
                        position: if group.at_end {
                            segment.virtual_end
                        } else {
                            segment.virtual_start
                        },
                        fidelity: Fidelity::Atom,
                    }
                };
                if !results.contains(&mapped) {
                    results.push(mapped);
                }
            }
        }
        tsr_core::sort_like_go(&mut results, &mut |a, b| {
            compare_positions(a.position, b.position)
        });
        results
    }

    /// Every feature-compatible virtual projection of an original range.
    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.OriginalToVirtualSpans
    pub fn original_to_virtual_spans(&self, range: TextRange, feature: i32) -> Vec<MappedSpan> {
        let (start, end) = virtual_bounds(range);
        if start == end {
            return self
                .original_to_virtual_positions(start, feature)
                .into_iter()
                .map(|position| MappedSpan {
                    span: point(position.position),
                    fidelity: position.fidelity,
                })
                .collect();
        }
        let index = self.original_index();
        let (start_segments, start_inside) = index.segments_at_original_position(start);
        let (end_segments, end_inside) = index.segments_at_original_position(end.wrapping_sub(1));
        if !start_inside || !end_inside {
            return Vec::new();
        }
        let containing: Vec<SpanSegment> = start_segments
            .iter()
            .copied()
            .filter(|segment| end <= segment.original_end)
            .collect();
        if !containing.is_empty() {
            let mut results =
                original_to_virtual_spans_in_segments(&containing, start, end, feature);
            if !results.is_empty() {
                tsr_core::sort_like_go(&mut results, &mut |a, b| {
                    (a.span.pos() - b.span.pos()).cmp(&0)
                });
                return results;
            }
        }
        let mut starts = original_start_projections(&start_segments, start, feature);
        let mut ends = original_end_projections(&end_segments, end, feature);
        if starts.is_empty() || ends.is_empty() {
            return Vec::new();
        }
        starts.sort_unstable();
        ends.sort_unstable();
        let mut results = Vec::with_capacity(starts.len().min(ends.len()));
        for (i, &virtual_start) in starts.iter().enumerate() {
            let end_index = ends.partition_point(|&end| end < virtual_start);
            if end_index == ends.len() || i + 1 < starts.len() && starts[i + 1] <= ends[end_index] {
                continue;
            }
            results.push(MappedSpan {
                span: range_of(virtual_start, ends[end_index]),
                fidelity: Fidelity::Approximate,
            });
        }
        results
    }

    /// Every feature-enabled segment intersection with `range`, whatever
    /// covers the range's endpoints.
    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.OriginalToVirtualIntersectingSpans
    pub fn original_to_virtual_intersecting_spans(
        &self,
        range: TextRange,
        feature: i32,
    ) -> Vec<MappedSpan> {
        if range.pos() == range.end() {
            return self.original_to_virtual_spans(range, feature);
        }
        let mut results = Vec::new();
        for segment in &self.segments {
            if !supports_feature(segment, feature) {
                continue;
            }
            let (low, high) = virtual_bounds(range);
            let start = low.max(segment.original_start);
            let end = high.min(segment.original_end);
            if start >= end {
                continue;
            }
            results.push(if segment.kind == KIND_VERBATIM {
                MappedSpan {
                    span: range_of(
                        segment
                            .virtual_start
                            .wrapping_add(start.wrapping_sub(segment.original_start)),
                        segment
                            .virtual_start
                            .wrapping_add(end.wrapping_sub(segment.original_start)),
                    ),
                    fidelity: Fidelity::Exact,
                }
            } else {
                MappedSpan {
                    span: range_of(segment.virtual_start, segment.virtual_end),
                    fidelity: Fidelity::Atom,
                }
            });
        }
        results
    }

    /// The original-order interval index, built once.
    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.origIndex
    fn original_index(&self) -> &OriginalIndex {
        self.original_index.get_or_init(|| {
            let mut segments = self.segments.clone();
            tsr_core::sort_like_go(&mut segments, &mut |a, b| {
                compare_positions(a.original_start, b.original_start)
                    .then_with(|| compare_positions(a.original_end, b.original_end))
                    .then_with(|| compare_positions(a.virtual_start, b.virtual_start))
            });
            let mut leaf_count = 1;
            while leaf_count < segments.len() {
                leaf_count *= 2;
            }
            let mut max_ends = vec![0; 2 * leaf_count];
            for (i, segment) in segments.iter().enumerate() {
                max_ends[leaf_count + i] = segment.original_end;
            }
            for i in (1..leaf_count).rev() {
                max_ends[i] = max_ends[2 * i].max(max_ends[2 * i + 1]);
            }
            OriginalIndex {
                segments,
                leaf_count,
                max_ends,
            }
        })
    }

    /// The span map's tuple JSON: `[virtualStart, virtualLength,
    /// originalStart, originalLength, kind(, features)]`, the features left out
    /// when they are all.
    /// port: tsc/internal/spanmap/spanmap.go:SpanMap.Marshal
    pub fn marshal(&self) -> Result<Vec<u8>, tsr_json::Error> {
        let tuples: Vec<Vec<i64>> = self
            .segments
            .iter()
            .map(|segment| {
                let mut tuple = vec![
                    i64::from(segment.virtual_start),
                    i64::from(segment.virtual_end.wrapping_sub(segment.virtual_start)),
                    i64::from(segment.original_start),
                    i64::from(segment.original_end.wrapping_sub(segment.original_start)),
                    i64::from(segment.kind),
                ];
                if segment.features != FEATURE_ALL {
                    tuple.push(i64::from(segment.features));
                }
                tuple
            })
            .collect();
        tsr_json::marshal(&Tuples(&tuples), tsr_json::Options::default())
    }
}

struct Tuples<'a>(&'a [Vec<i64>]);

impl tsr_json::Encode for Tuples<'_> {
    fn encode(&self, out: &mut tsr_json::Encoder<'_>) -> Result<(), tsr_json::Error> {
        struct Tuple<'a>(&'a [i64]);
        impl tsr_json::Encode for Tuple<'_> {
            fn encode(&self, out: &mut tsr_json::Encoder<'_>) -> Result<(), tsr_json::Error> {
                out.array(self.0.iter())
            }
        }
        let tuples: Vec<Tuple<'_>> = self.0.iter().map(|tuple| Tuple(tuple)).collect();
        out.array(tuples.iter())
    }
}

/// A decoding failure of the tuple form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnmarshalError {
    Json(tsr_json::Error),
    Arity { segment: usize, values: usize },
}

impl std::fmt::Display for UnmarshalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(f, "{error}"),
            Self::Arity { segment, values } => write!(
                f,
                "span map segment {segment}: expected 5 or 6 values, got {values}"
            ),
        }
    }
}

impl std::error::Error for UnmarshalError {}

/// Decodes the tuple form an out-of-process mapper produces. Five-value
/// tuples mean every feature; six-value tuples keep their feature mask.
/// port: tsc/internal/spanmap/spanmap.go:Unmarshal
pub fn unmarshal(data: &[u8]) -> Result<SpanMap, UnmarshalError> {
    let mut tuples: Vec<Vec<i32>> = Vec::new();
    tsr_json::unmarshal(data, &mut tuples, tsr_json::Options::default())
        .map_err(UnmarshalError::Json)?;
    let mut segments = Vec::with_capacity(tuples.len());
    for (i, tuple) in tuples.iter().enumerate() {
        if tuple.len() != 5 && tuple.len() != 6 {
            return Err(UnmarshalError::Arity {
                segment: i,
                values: tuple.len(),
            });
        }
        segments.push(SpanSegment {
            virtual_start: tuple[0],
            virtual_end: tuple[0].wrapping_add(tuple[1]),
            original_start: tuple[2],
            original_end: tuple[2].wrapping_add(tuple[3]),
            kind: tuple[4],
            features: tuple.get(5).copied().unwrap_or(FEATURE_ALL),
        });
    }
    Ok(SpanMap::new(&segments))
}

/// Maps `range` through `map`; `None` maps identically (Go's nil map).
pub fn virtual_to_original_span(map: Option<&SpanMap>, range: TextRange) -> (TextRange, Fidelity) {
    map.map_or((range, Fidelity::Exact), |map| {
        map.virtual_to_original_span(range)
    })
}

fn verbatim_position(segment: &SpanSegment, pos: i32) -> i32 {
    clamp(
        segment
            .original_start
            .wrapping_add(pos.wrapping_sub(segment.virtual_start)),
        segment.original_start,
        segment.original_end,
    )
}

/// A range's bounds as `TextPos` values, the end at least the start.
fn virtual_bounds(range: TextRange) -> (i32, i32) {
    // Go converts the positions to TextPos (int32).
    #[allow(clippy::cast_possible_truncation)]
    let start = range.pos() as i32;
    #[allow(clippy::cast_possible_truncation)]
    let end = (range.end() as i32).max(start);
    (start, end)
}

fn point(position: i32) -> TextRange {
    TextRange::new(i64::from(position), i64::from(position))
}

fn range_of(start: i32, end: i32) -> TextRange {
    TextRange::new(i64::from(start), i64::from(end))
}

/// Segment starts, including zero-length segments, are contained. A final end
/// point is also contained; other ends belong to the following gap or segment.
/// port: tsc/internal/spanmap/spanmap.go:SpanMap.segmentIndexAt
pub(crate) fn segment_at(segments: &[SpanSegment], pos: i32) -> (Option<usize>, bool) {
    let index = segments.partition_point(|s| s.virtual_start.wrapping_sub(pos) < 0);
    if segments.get(index).is_some_and(|s| s.virtual_start == pos) {
        return (Some(index), true);
    }
    let previous = index.checked_sub(1);
    let inside = previous.is_some_and(|i| {
        pos < segments[i].virtual_end || i == segments.len() - 1 && pos == segments[i].virtual_end
    });
    (previous, inside)
}

/// port: tsc/internal/spanmap/spanmap.go:originalStartProjections
fn original_start_projections(segments: &[SpanSegment], start: i32, feature: i32) -> Vec<i32> {
    segments
        .iter()
        .filter(|segment| supports_feature(segment, feature))
        .map(|segment| {
            if segment.kind == KIND_VERBATIM {
                clamp(
                    segment
                        .virtual_start
                        .wrapping_add(start.wrapping_sub(segment.original_start)),
                    segment.virtual_start,
                    segment.virtual_end,
                )
            } else {
                segment.virtual_start
            }
        })
        .collect()
}

/// port: tsc/internal/spanmap/spanmap.go:originalEndProjections
fn original_end_projections(segments: &[SpanSegment], end: i32, feature: i32) -> Vec<i32> {
    segments
        .iter()
        .filter(|segment| supports_feature(segment, feature))
        .map(|segment| {
            if segment.kind == KIND_VERBATIM {
                clamp(
                    segment
                        .virtual_start
                        .wrapping_add(end.wrapping_sub(segment.original_start)),
                    segment.virtual_start,
                    segment.virtual_end,
                )
            } else {
                segment.virtual_end
            }
        })
        .collect()
}

/// port: tsc/internal/spanmap/spanmap.go:originalToVirtualSpansInSegments
fn original_to_virtual_spans_in_segments(
    segments: &[SpanSegment],
    start: i32,
    end: i32,
    feature: i32,
) -> Vec<MappedSpan> {
    segments
        .iter()
        .filter(|segment| supports_feature(segment, feature))
        .map(|segment| {
            if segment.kind == KIND_VERBATIM {
                let virtual_start = clamp(
                    segment
                        .virtual_start
                        .wrapping_add(start.wrapping_sub(segment.original_start)),
                    segment.virtual_start,
                    segment.virtual_end,
                );
                let virtual_end = clamp(
                    segment
                        .virtual_start
                        .wrapping_add(end.wrapping_sub(segment.original_start)),
                    virtual_start,
                    segment.virtual_end,
                );
                MappedSpan {
                    span: range_of(virtual_start, virtual_end),
                    fidelity: Fidelity::Exact,
                }
            } else {
                MappedSpan {
                    span: range_of(segment.virtual_start, segment.virtual_end),
                    fidelity: Fidelity::Atom,
                }
            }
        })
        .collect()
}

/// port: tsc/internal/spanmap/spanmap.go:sameOriginalRange
fn same_original_range(left: &SpanSegment, right: &SpanSegment) -> bool {
    left.original_start == right.original_start && left.original_end == right.original_end
}

/// Segments in original order over a complete binary tree of maximum
/// original ends, which prunes subtrees that cannot reach a position.
#[derive(Debug)]
struct OriginalIndex {
    segments: Vec<SpanSegment>,
    leaf_count: usize,
    max_ends: Vec<i32>,
}

struct SegmentGroup {
    segments: Vec<SpanSegment>,
    at_end: bool,
}

impl OriginalIndex {
    /// Every segment containing `pos`; a segment start is contained.
    /// port: tsc/internal/spanmap/spanmap.go:originalIndex.segmentsAtOriginalPosition
    fn segments_at_original_position(&self, pos: i32) -> (Vec<SpanSegment>, bool) {
        let start = self
            .segments
            .partition_point(|segment| segment.original_start < pos);
        let mut results = self.segments_ending_after_position(start, pos);
        let end = self
            .segments
            .partition_point(|segment| segment.original_start <= pos);
        results.extend_from_slice(&self.segments[start..end]);
        let inside = !results.is_empty();
        (results, inside)
    }

    /// port: tsc/internal/spanmap/spanmap.go:originalIndex.segmentsEndingAfterPosition
    fn segments_ending_after_position(&self, limit: usize, pos: i32) -> Vec<SpanSegment> {
        let mut results = Vec::new();
        self.collect_segments_ending_at_or_after(
            1,
            0,
            self.leaf_count,
            limit,
            pos,
            false,
            &mut results,
        );
        results
    }

    /// port: tsc/internal/spanmap/spanmap.go:originalIndex.collectSegmentsEndingAtOrAfter
    #[allow(clippy::too_many_arguments)]
    fn collect_segments_ending_at_or_after(
        &self,
        node: usize,
        start: usize,
        end: usize,
        limit: usize,
        pos: i32,
        include_end: bool,
        results: &mut Vec<SpanSegment>,
    ) {
        if start >= limit || self.max_ends[node] < pos || !include_end && self.max_ends[node] == pos
        {
            return;
        }
        if end - start == 1 {
            results.push(self.segments[start]);
            return;
        }
        let middle = start + (end - start) / 2;
        self.collect_segments_ending_at_or_after(
            2 * node,
            start,
            middle,
            limit,
            pos,
            include_end,
            results,
        );
        self.collect_segments_ending_at_or_after(
            2 * node + 1,
            middle,
            end,
            limit,
            pos,
            include_end,
            results,
        );
    }

    /// Every group of equal-range segments containing or touching `pos`.
    /// port: tsc/internal/spanmap/spanmap.go:originalIndex.segmentGroupsAtOriginalPosition
    fn segment_groups_at_original_position(&self, pos: i32) -> Vec<SegmentGroup> {
        let limit = self
            .segments
            .partition_point(|segment| segment.original_start <= pos);
        let mut segments = Vec::new();
        self.collect_segments_ending_at_or_after(
            1,
            0,
            self.leaf_count,
            limit,
            pos,
            true,
            &mut segments,
        );
        let mut groups = Vec::new();
        let mut start = 0;
        while start < segments.len() {
            let mut end = start + 1;
            while end < segments.len() && same_original_range(&segments[start], &segments[end]) {
                end += 1;
            }
            let segment = segments[start];
            if pos <= segment.original_end {
                groups.push(SegmentGroup {
                    segments: segments[start..end].to_vec(),
                    at_end: pos == segment.original_end && pos != segment.original_start,
                });
            }
            start = end;
        }
        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(v: (i32, i32), o: (i32, i32), kind: i32) -> SpanSegment {
        SpanSegment {
            virtual_start: v.0,
            virtual_end: v.1,
            original_start: o.0,
            original_end: o.1,
            kind,
            features: FEATURE_ALL,
        }
    }

    fn span(start: i64, end: i64) -> TextRange {
        TextRange::new(start, end)
    }

    #[test]
    fn pinned_forward_span_and_boundary_cases() {
        // Expectations from internal/spanmap/spanmap_test.go, including the
        // distinction between nil, empty, a gap, an atom and the final end.
        let verbatim = SpanMap::new(&[
            segment((0, 10), (100, 110), 0),
            segment((20, 30), (200, 210), 0),
        ]);
        for (from, to, fidelity) in [
            ((3, 7), (103, 107), Fidelity::Exact),
            ((12, 15), (110, 110), Fidelity::None),
            ((30, 30), (210, 210), Fidelity::Exact),
            ((10, 10), (110, 110), Fidelity::None),
        ] {
            assert_eq!(
                verbatim.virtual_to_original_span(span(from.0, from.1)),
                (span(to.0, to.1), fidelity)
            );
        }
        let crossing = SpanMap::new(&[
            segment((10, 20), (200, 210), 0),
            segment((0, 10), (100, 110), 0),
        ]);
        assert_eq!(
            crossing.virtual_to_original_span(span(5, 15)),
            (span(105, 205), Fidelity::Approximate)
        );
        for kind in [KIND_ATOM, KIND_ALIAS] {
            let atom = SpanMap::new(&[segment((3, 14), (60, 71), kind)]);
            assert_eq!(
                atom.virtual_to_original_span(span(5, 9)),
                (span(60, 71), Fidelity::Atom)
            );
        }
        assert_eq!(
            SpanMap::new(&[]).virtual_to_original_span(span(5, 10)),
            (span(0, 0), Fidelity::None)
        );
        assert_eq!(
            virtual_to_original_span(None, span(3, 7)),
            (span(3, 7), Fidelity::Exact)
        );
    }

    #[test]
    fn original_ranges_project_through_every_copy() {
        // The pinned example: two original segments each copied twice.
        let map = SpanMap::new(&[
            segment((0, 2), (0, 2), KIND_VERBATIM),
            segment((2, 4), (2, 4), KIND_VERBATIM),
            segment((10, 12), (0, 2), KIND_VERBATIM),
            segment((12, 14), (2, 4), KIND_VERBATIM),
        ]);
        assert_eq!(
            map.original_to_virtual_spans(span(1, 3), FEATURE_ALL),
            vec![
                MappedSpan {
                    span: span(1, 3),
                    fidelity: Fidelity::Approximate
                },
                MappedSpan {
                    span: span(11, 13),
                    fidelity: Fidelity::Approximate
                },
            ]
        );
        assert_eq!(
            map.original_to_virtual_spans(span(0, 1), FEATURE_ALL),
            vec![
                MappedSpan {
                    span: span(0, 1),
                    fidelity: Fidelity::Exact
                },
                MappedSpan {
                    span: span(10, 11),
                    fidelity: Fidelity::Exact
                },
            ]
        );
        // A shared boundary projects from both sides.
        let positions: Vec<i32> = map
            .original_to_virtual_positions(2, FEATURE_ALL)
            .into_iter()
            .map(|position| position.position)
            .collect();
        assert_eq!(positions, [2, 12]);
        assert!(map
            .original_to_virtual_spans(span(1, 3), FEATURE_HOVER << 25)
            .is_empty());
    }

    #[test]
    fn the_tuple_form_round_trips_and_features_default_to_all() {
        let mut hover = segment((4, 5), (3, 4), KIND_VERBATIM);
        hover.features = FEATURE_HOVER;
        let map = SpanMap::new(&[hover, segment((0, 3), (1, 2), KIND_ALIAS)]);
        let json = map.marshal().unwrap();
        assert_eq!(json, b"[[0,3,1,1,2],[4,1,3,1,0,1]]");
        assert_eq!(unmarshal(&json).unwrap(), map);
        assert_eq!(SpanMap::new(&[]).marshal().unwrap(), b"[]");
        assert_eq!(
            unmarshal(b"[[1,2,3]]").unwrap_err().to_string(),
            "span map segment 0: expected 5 or 6 values, got 3"
        );
    }

    #[test]
    fn validation_reports_the_first_violation() {
        let virtual_text = b"let x = 1;";
        let original = b"x = 1";
        let good = SpanMap::new(&[segment((4, 9), (0, 5), KIND_VERBATIM)]);
        assert_eq!(good.validate(virtual_text, original), None);
        let mismatch = SpanMap::new(&[segment((0, 5), (0, 5), KIND_VERBATIM)]);
        assert_eq!(
            mismatch.validate(virtual_text, original).unwrap().to_string(),
            "content mapper verbatim mapping does not match the original content at virtual offset 0, original offset 0"
        );
        let outside = SpanMap::new(&[segment((0, 1), (4, 9), KIND_ATOM)]);
        assert_eq!(
            outside.validate(virtual_text, original).unwrap().kind,
            MappingErrorKind::OutOfBounds
        );
        let overlap = SpanMap::new(&[
            segment((0, 4), (0, 1), KIND_ATOM),
            segment((2, 5), (1, 2), KIND_ATOM),
        ]);
        assert_eq!(
            overlap.validate(virtual_text, original).unwrap().kind,
            MappingErrorKind::Overlap
        );
    }
}
