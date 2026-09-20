use super::{break_patterns, partial_insertion_sort, partition_equal, pdqsort, sort};
use std::{cmp::Ordering, collections::HashMap};

struct Fixture {
    name: &'static str,
    mode: &'static str,
    lazy: bool,
    keys: &'static [i32],
    start: usize,
    end: usize,
    sorted: &'static [usize],
    trace: &'static [[i32; 3]],
    result: bool,
    result_index: usize,
    panicked: bool,
}
include!("fixtures.rs");

#[test]
fn comparator_order_and_permutations_match_the_pinned_go_standard_library() {
    for fixture in FIXTURES {
        let mut data = (0..fixture.keys.len()).collect::<Vec<_>>();
        let mut trace = Vec::new();
        let mut ids = HashMap::new();
        let mut next = 0_i32;
        let mut id = |value| {
            *ids.entry(value).or_insert_with(|| {
                next += 1;
                next
            })
        };
        let mut compare = |&left: &usize, &right: &usize| {
            let (mut left_key, mut right_key) = (fixture.keys[left], fixture.keys[right]);
            if fixture.lazy {
                left_key = id(left_key);
                right_key = id(right_key);
            }
            let result = left_key.cmp(&right_key);
            let sign = match result {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            };
            trace.push([left as i32, right as i32, sign]);
            assert!(
                fixture.mode != "panic" || trace.len() != 2,
                "sort comparison"
            );
            result
        };
        let mut result = false;
        let mut result_index = 0;
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match fixture.mode {
                "sort" | "panic" => sort(&mut data, &mut compare),
                "heap" => pdqsort(&mut data, fixture.start, fixture.end, 0, &mut compare),
                "partial" => {
                    result =
                        partial_insertion_sort(&mut data, fixture.start, fixture.end, &mut compare);
                }
                "break" => break_patterns(&mut data, fixture.start, fixture.end),
                "partition-equal" => {
                    result_index = partition_equal(
                        &mut data,
                        fixture.start,
                        fixture.end,
                        fixture.start,
                        &mut compare,
                    );
                }
                _ => panic!("unknown frozen sort fixture mode"),
            }));
        assert_eq!(outcome.is_err(), fixture.panicked);
        if let Err(payload) = outcome {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied());
            assert_eq!(
                message,
                Some("sort comparison"),
                "only the named source comparator panic is expected"
            );
        }
        assert_eq!(
            trace,
            fixture.trace,
            "{} {} len={} lazy={}",
            fixture.name,
            fixture.mode,
            fixture.keys.len(),
            fixture.lazy
        );
        assert_eq!(data, fixture.sorted, "{} {}", fixture.name, fixture.mode);
        assert_eq!(result, fixture.result);
        assert_eq!(result_index, fixture.result_index);
    }
}
