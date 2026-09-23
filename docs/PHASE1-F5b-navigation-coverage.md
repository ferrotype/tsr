# F5b navigation helper witnesses

These are links to existing paired observations, not credit from a general
parser corpus. `tools/phase1/syntax/astnav_probe_test.go` and the Rust `astnav`
driver execute each action over every byte position. Each run-length entry
retains the returned node's kind, full start, end and first-seen identity.

| Operation in `astnav/tokens.go` | Exact request/action and necessary observation |
| --- | --- |
| `getTokenAtPosition` | `trivia-comments-and-unterminated-jsdoc`, `token_at` and `touching_token`. Both public entry points immediately invoke the private worker. Byte 0 belongs to the trivia-inclusive `let` token in the former and SourceFile in the latter. |
| `getPosition` | The same request/actions. Every tested child calls this helper; it returns raw start in the first mode and skips trivia in the second. The `[0,13]` token-at range versus `[0,10]` SourceFile touching range observes the distinction. |
| `getNodeVisitor` | `mapcode-visitor-list-ranges`, `visit_lists`, and `js-jsdoc-reparsed-nodes`, `visit_nodes`. `VisitEachChildAndJSDoc` always constructs this wrapper. The outputs retain ordered hooks and list ranges, while the JSDoc case observes filtered comment nodes rather than treating the wrapper's allocation as behavior. Rust's equivalent is `visit_child_slots_and_jsdoc` plus `push_node_hook`. |
| `findRightmostValidToken` | `upstream-preceding-after-comma`, `preceding`. At position 15 the comma at 13..14 is absent from child-node traversal and must be recovered through this private fallback. The recorded output is `[27,13,14,6]` for positions 14..15. |
| `scanNavigationToken` | The same fallback and observation. Its intervening scanner loop obtains the comma through this helper. This establishes ordinary-token passthrough only; it does not establish the specialized JSX rescan. |
| `isValidPrecedingNode` | The same `preceding` action. The fallback tests prior AST children before scanning the omitted comma. The sweep separately returns Identifier 11..13 at position 13 and CommaToken 13..14 at 14..15, preserving the node/token boundary. |
| `shouldSkipChild` | `trivia-comments-and-unterminated-jsdoc`, `token_at`. At 68..82 the enclosing unterminated JSDoc (kind315) is returned rather than scanning ordinary tokens from its text. The private worker calls this predicate on the childless, non-token JSDoc. |

The Rust homes are the corresponding methods in
`crates/tsr_astnav/src/lib.rs`. `getNodeVisitor` is an explicit callback-order
adapter there rather than a second heap-allocated visitor object.

The old JSX/shift source row still does not establish a true rescan arm: the
containing node passed to the scanner matters. A new separate direct witness,
`tools/phase1/syntax/astnav-rescan/`, calls both private pinned helpers with
explicit JSX and non-JSX containing nodes. Five native answers are retained
with source/request/output digests. They show `<<` rescanning from kind47,
end2 to kind29, end1 in JSX, ordinary `<<` staying unchanged, and `<`/`<<=`
remaining unchanged. The Rust crate unit test reads the native-derived TSV
and compares kinds, ranges and flags. A script regression binds that TSV to
the recorded native observations and current pinned source/probe request.
This separate witness closes `shouldRescanLessThanLessThanToken`; it does not
retroactively broaden the claim made by the earlier parsed JSX case.
