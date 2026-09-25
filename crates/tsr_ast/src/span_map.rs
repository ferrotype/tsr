//! Forward source mapping used by diagnostic display. Reverse editor lookups,
//! feature filtering and transport validation remain with the mapper phase.
use crate::SpanSegment;
use std::sync::Arc;
use tsr_core::TextRange;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fidelity {
    Exact,
    Atom,
    Approximate,
    None,
}
/// port: tsc/internal/spanmap/spanmap.go:New
pub fn new(segments: &[SpanSegment]) -> Arc<[SpanSegment]> {
    let mut segments = segments.to_vec();
    tsr_core::sort_like_go(&mut segments, &mut |a, b| {
        a.virtual_start.wrapping_sub(b.virtual_start).cmp(&0)
    });
    segments.into()
}
fn clamp(value: i32, low: i32, high: i32) -> i32 {
    value.min(high).max(low)
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
fn insertion_point(segments: &[SpanSegment], index: Option<usize>) -> i32 {
    index.map_or(0, |i| segments[i].original_end)
}
fn map_boundary(
    segments: &[SpanSegment],
    pos: i32,
    index: Option<usize>,
    inside: bool,
    high: bool,
) -> i32 {
    if !inside {
        return insertion_point(segments, index);
    }
    let segment = &segments[index.expect("contained segment")];
    if segment.kind == 0 {
        clamp(
            segment
                .original_start
                .wrapping_add(pos.wrapping_sub(segment.virtual_start)),
            segment.original_start,
            segment.original_end,
        )
    } else if high {
        segment.original_end
    } else {
        segment.original_start
    }
}
/// The segment slice comes from `new` (sorted in virtual order); absence is an
/// identity map while an empty map marks every position as synthesized.
/// port: tsc/internal/spanmap/spanmap.go:SpanMap.VirtualToOriginalSpan
pub fn virtual_to_original_span(
    segments: Option<&[SpanSegment]>,
    range: TextRange,
) -> (TextRange, Fidelity) {
    let Some(segments) = segments else {
        return (range, Fidelity::Exact);
    };
    let start = range.pos() as i32;
    let end = (range.end() as i32).max(start);
    let (start_index, start_inside) = segment_at(segments, start);
    if start == end {
        let mapped = map_boundary(segments, start, start_index, start_inside, false);
        let fidelity = if !start_inside {
            Fidelity::None
        } else if segments[start_index.unwrap()].kind == 0 {
            Fidelity::Exact
        } else {
            Fidelity::Atom
        };
        return (
            TextRange::new(i64::from(mapped), i64::from(mapped)),
            fidelity,
        );
    }
    let (end_index, end_inside) = segment_at(segments, end.wrapping_sub(1));
    if start_index == end_index && start_inside == end_inside {
        if !start_inside {
            let pos = i64::from(insertion_point(segments, start_index));
            return (TextRange::new(pos, pos), Fidelity::None);
        }
        let segment = &segments[start_index.unwrap()];
        if segment.kind == 0 {
            let low = map_boundary(segments, start, start_index, true, false);
            let high = clamp(
                segment
                    .original_start
                    .wrapping_add(end.wrapping_sub(segment.virtual_start)),
                low,
                segment.original_end,
            );
            return (
                TextRange::new(i64::from(low), i64::from(high)),
                Fidelity::Exact,
            );
        }
        return (
            TextRange::new(
                i64::from(segment.original_start),
                i64::from(segment.original_end),
            ),
            Fidelity::Atom,
        );
    }
    let low = map_boundary(segments, start, start_index, start_inside, false);
    let high = map_boundary(segments, end, end_index, end_inside, true).max(low);
    (
        TextRange::new(i64::from(low), i64::from(high)),
        Fidelity::Approximate,
    )
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
            features: 0,
        }
    }

    #[test]
    fn pinned_forward_span_and_boundary_cases() {
        // Expectations from internal/spanmap/spanmap_test.go, including the
        // distinction between nil, empty, a gap, an atom and the final end.
        let verbatim = new(&[
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
                virtual_to_original_span(Some(&verbatim), TextRange::new(from.0, from.1)),
                (TextRange::new(to.0, to.1), fidelity)
            );
        }
        let crossing = new(&[
            segment((10, 20), (200, 210), 0),
            segment((0, 10), (100, 110), 0),
        ]);
        assert_eq!(
            virtual_to_original_span(Some(&crossing), TextRange::new(5, 15)),
            (TextRange::new(105, 205), Fidelity::Approximate)
        );
        for kind in [1, 2] {
            let atom = new(&[segment((3, 14), (60, 71), kind)]);
            assert_eq!(
                virtual_to_original_span(Some(&atom), TextRange::new(5, 9)),
                (TextRange::new(60, 71), Fidelity::Atom)
            );
        }
        assert_eq!(
            virtual_to_original_span(Some(&[]), TextRange::new(5, 10)),
            (TextRange::new(0, 0), Fidelity::None)
        );
        assert_eq!(
            virtual_to_original_span(None, TextRange::new(3, 7)),
            (TextRange::new(3, 7), Fidelity::Exact)
        );
    }
}
