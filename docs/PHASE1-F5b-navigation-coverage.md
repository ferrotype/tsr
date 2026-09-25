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
| `shouldRescanLessThanLessThanToken`, `scanNavigationToken` (both arms of the rescan) | `jsx-child-shift-gap-scan`, `token_at`, `touching_token`, `preceding` and `preceding_exclude_jsdoc`. True arm: each `<` of `<div><<</div>`, `<>{1}<<</>` and `<a><<b/></a>` opens an error-recovery JsxSelfClosingElement (a JSX child) whose other children are zero-width, so navigation scans the `<`, the scanner reads `<<`, and the rescan returns the width-1 LessThanToken (kind29) at 15, 16, 40, 41 and 60. Pinned `-cover` shows tokens.go:18 executes in this request and in no other astnav request. False arm with a `<<`: in `type F = A<<T>() => T>;` and `let g = f<<T>(x: T) => T>(y);` navigation scans the type-argument `<<` under a TypeReference and a CallExpression (not JSX children), and it stays width 2: `token_at` at 81 and 104 is the containing TypeReference 79..93 and CallExpression 102..123, `preceding` at 82 and 105 is the LessThanLessThanToken (kind47) 81..83 and 104..106. A pinned-Go overlay whose predicate drops `IsJsxChild` answers a width-1 `<` at those four positions (so this request differs from `token_at` on), and changes no other astnav request. |
| `getTokenAtPosition` (gap scan), `findRightmostValidToken` (rescan call sites) | The same request. Without the rescan at tokens.go:240 the `token_at` answer is the JsxSelfClosingElement itself (the `<<` overruns it); without it at tokens.go:564/582 the `preceding` answer is the width-2 LessThanLessThanToken (kind47). Disabling either Rust call site (crates/tsr_astnav/src/lib.rs `scan_for_token` and `rightmost`), or passing a JSX child for every containing node at either of them, or dropping the JSX-child test from `scan_navigation_token`, makes only this case differ. |
| `isValidPrecedingNode` | The same `preceding` action. The fallback tests prior AST children before scanning the omitted comma. The sweep separately returns Identifier 11..13 at position 13 and CommaToken 13..14 at 14..15, preserving the node/token boundary. |
| `shouldSkipChild` | `trivia-comments-and-unterminated-jsdoc`, `token_at`. At 68..82 the enclosing unterminated JSDoc (kind315) is returned rather than scanning ordinary tokens from its text. The private worker calls this predicate on the childless, non-token JSDoc. |

The Rust homes are the corresponding methods in
`crates/tsr_astnav/src/lib.rs`. `getNodeVisitor` is an explicit callback-order
adapter there rather than a second heap-allocated visitor object.

`jsx-less-than-less-than` enters `shouldRescanLessThanLessThanToken` but never with a `<<`: every shift there
is the operator token of a binary expression, so navigation never scans one. Both arms are witnessed by the
parsed production request `jsx-child-shift-gap-scan` above, which has a `<<` scanned inside JSX children and a
`<<` scanned under ordinary containing nodes. It replaced the earlier access-only overlay that called the private
helpers with constructed containing nodes (retired with its TSV, unit test and reproduction script, review item
I40).

Shared helpers these requests also witness (PB11):

| Operation | Exact request/action and necessary observation |
| --- | --- |
| `core/binarysearch.go:BinarySearchUniqueFunc` | `mapcode-sweeps` and `trivia-comments-and-unterminated-jsdoc` (`token_at`, `touching_property_name`, `next_in_file`), `dotted-namespace-reparsed-nodes` (`token_at`, `touching_property_name`), `upstream-preceding-after-comma` (`token_at`, `preceding`). Each action is the first whose answer changes under a Rust mutant of `binary_search_unique` (match reported as a miss, miss index 0, swapped halves; the first two requests also catch an upper-middle probe order, which only changes the comparer's side effects). |
| `ast/utilities.go:ForEachChildAndJSDoc` | `js-jsdoc-reparsed-nodes` and `find-child-of-kind-stale-token`, `child_of_kind`. The first observes JSDoc visited before the children; in the second a visitor that ignored the stop would overwrite the found Identifier node with a stale-scanner Identifier token of the same range, which the later `visit_nodes` identities expose. |
| `ast/ast.go:SourceFile.GetOrCreateToken`, `ast/ast.go:createToken` | `upstream-pointer-equality` (`token_at`, `token_at_repeat`, `child_of_kind`), `find-child-of-kind-stale-token` (`child_of_kind`, which creates Identifier-kind tokens), `jsx-child-shift-gap-scan` (`token_at`, `token_at_repeat`). Answers carry kind, range and first-seen identity, so a token range off by one or a repeated question answered by a new node differs. They cannot see the payload createToken builds. |
| `ast/ast.go:createToken` (payload) | `created-token-payloads`, `child_of_kind_payload`, whose answers append the node's text, token flags, raw text and template flags. FindChildOfKind tests the scanner's stale token after a literal- or name-led child, so it creates StringLiteral, NumericLiteral, BigIntLiteral, NoSubstitutionTemplateLiteral, TemplateHead, Identifier and PrivateIdentifier tokens: the text must be the raw slice from the full start (leading newline included), the flags the scanner's under the constructor's mask (the stale PrecedingLineBreak dropped), and a template piece's token flags 0 and raw text empty. A Rust `create_token` that drops a mask, blanks or trims the text, or fills a template's raw text or token flags differs here and in no other astnav request. The JsxText, RegularExpressionLiteral, TemplateMiddle and TemplateTail arms run in no astnav request (pinned `-cover`): navigation scans with `Scan()` and the JSX `<<` rescan, and neither produced those kinds on any input tried. |
