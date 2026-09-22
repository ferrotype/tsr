# F5b handwritten parser witness audit

This audit adds 36 exact parser operation IDs to six existing syntax requests,
with 37 operation/action links. It does not add a corpus-wide parser claim or
change any request, native answer, acceptance denominator or production code.
The pin is `1f70213d4922b434345f639b441681e470c7cfc1`.

The reviewed requests are in `data/phase1/requests/syntax-diagnostics.json` and
`syntax-parse-outputs.json`. Each link lives in that case's `operation_actions`
in `data/phase1/cases.json`, whose request digest binds the claim to the exact
input and action schedule. The observations below were read from the F5a
syntax capture at `target/phase1-f5a-syntax`: native `diagnostics` and
`parse_outputs`, and `rust-observations.json`. Complete observations for all
six cases match there. This historical inspection establishes the witnesses;
it does not make that capture current after F5b source changes. The normal
syntax capture/compare/record path must supply current evidence.

All operation names in the following tables have the prefix
`tsc/internal/parser/`. A link means the named branch necessarily executes
and contributes to the stated observation. It does not claim every branch of
the operation has a witness.

## Source metadata: raw pragmas

Case `syntax/parse-outputs/pragmas`, action `side_fields`:

| Operations | Observable discriminator | Rust home |
| --- | --- | --- |
| `parser.go:extractPragmas`, `match`, `extractName` | Four reference pragmas and four normalized JSX pragma names are present, while the two AMD directives are omitted. Reference attribute names, including `no-default-lib` and `resolution-mode`, are preserved. | `crates/tsr_parser/src/pragmas.rs` |
| `parser.go:skipBlanks`, `extractQuotedString` | The path argument is `a.ts` at 21..25, `types` is `node` at 52..56, and resolution mode is `import` at 75..81. Quotation delimiters and surrounding whitespace are excluded from the values and their ranges. | Same |
| `parser.go:skipTo`, `skipNonBlanks` | The block-comment scan finds `jsx`, `jsxfrag`, `jsximportsource` and `jsxruntime` and extracts exactly `h`, `Fragment`, `preact`, `classic`, at 237..238, 255..263, 288..294, 314..321. | Same |

The native path is `finishSourceFile -> getCommentPragmas -> extractPragmas`.
The probe's `ordered[0][1]` records raw pragmas, not resolved reference fields.
Consequently this case does **not** gain credit for `parseResolutionMode`,
reference-list validation or `processPragmasIntoFields`. Quoted-string error
branches, Unicode line separators and multiline pragma traversal remain
unwitnessed by this input.

## Syntactic diagnostics

Both cases below use the actual request action
`tsc/internal/compiler/program.go:Program.GetSyntacticDiagnostics`. The native
probe constructs a program and obtains its diagnostics; it does not rebuild
these diagnostics in the harness. `observation.ordered` retains file, range,
code, arguments and related information, and the same records also reach the
plain and pretty writers.

| Case suffix under `syntax/diagnostics/` | Operations | Observable discriminator | Rust home |
| --- | --- | --- | --- |
| `js-only-typescript-syntax` | `parser.go:Parser.checkJSSyntax`, `Parser.jsErrorAtRange` | Only `/a.js` gets TS8010 annotations at 7..13, 34..40, 43..47, TS8009 `?` at 31..32 and `public` at 89..95, TS8006 interface/enum, TS8013 non-null assertion and TS8008 type alias. The corresponding TypeScript file gets none. | `crates/tsr_parser/src/js_syntax.rs` |
| Same | `parser.go:Parser.parseJsxChild`, `Parser.parseErrorAt` | The unmatched `<any>` is TS17008 over the tag name at 143..146, with argument `any`, rather than at EOF. The EOF branch in `parseJsxChild` calls `parseErrorAt` with that explicit range. | `crates/tsr_parser/src/jsx.rs`, `diagnostics.rs` |
| `missing-nodes-zero-width` | `parser.go:Parser.parseExpected`, `Parser.parseExpectedWithDiagnostic`, `Parser.parseErrorAtCurrentToken`, `Parser.parseErrorAtRange` | The incomplete `for` header produces TS1005 with argument `)` at 57..57. The default `parseForStatement` branch calls `parseExpected(CloseParenToken)` and the error preserves the current token range. | `crates/tsr_parser/src/tokens.rs`, `diagnostics.rs` |
| Same | `parser.go:Parser.abortParsingListOrMoveToNextToken`, `Parser.parsingContextErrors` | The unexpected comma in the argument list produces TS1135 at 14..15. `parseDelimitedList(PCArgumentExpressions)` takes the invalid-element branch and dispatches this context-specific diagnostic before advancing. | `crates/tsr_parser/src/lists.rs` |

The parameter-decorator fixtures exercise the program-level additional-JS
walk. They are not a witness for the separate parser
`checkJSDecoratorSyntax` rejection branches, and no such credit is added here.

`syntax/diagnostics/unterminated-literals` was also inspected. Despite its
broad input description, its observed diagnostics are only TS1002 at 12..12
and TS1160 at 70..70: the unterminated template consumes the later regex and
comment-looking text. It cannot witness regex or block-comment errors.
Rust's scanner-diagnostic draining in `diagnostics.rs` is a plausible
mechanism equivalent to the native `Parser.scanError` callback, but that
implementation classification is not silently resolved by this witness pass.

## Lazy and eager JSDoc

Case `syntax/parse-outputs/lazy-jsdoc-in-typescript`, action `lazy_jsdoc`,
records the eager roots before lookup, two JSDoc lookups, eager roots after
lookup and the complete subtree kind/range/identity traversal. The native
path is `Node.JSDoc -> SourceFile.resolveJSDoc -> parseJSDocForNode`, then
`parseJSDocComment` and its worker. Rust uses `ParserJsDocProvider` in
`crates/tsr_parser/src/lazy_jsdoc.rs` and the parser in `src/jsdoc.rs`.

| Operations in `jsdoc.go` | Observable discriminator |
| --- | --- |
| `parseJSDocForNode`, `Parser.withJSDoc` | TypeScript's initial eager list is empty; first use creates the JSDoc roots; repeated and subsequent eager reads retain exactly the same ordinals. The companion `syntax/parse-outputs/eager-jsdoc-in-javascript` request has the same first function JSDoc tree already present before lookup. Both cases credit `withJSDoc`, distinguishing its JS eager and TS deferred paths. |
| `Parser.parseJSDocComment`, `Parser.parseJSDocCommentWorker` | Root 315 has range 0..61 with an initial JSDocText child 316 at 0..19. The final declaration has two separate ordered comment roots at 180..193 and 193..207. |
| `Parser.parseTag`, `Parser.parseParameterOrPropertyTag`, `Parser.parseReturnTag` | The first tree contains ParameterTag 333 at 19..41 with identifier children at 20..25 and 26..27, followed by ReturnTag 334 at 41..59 with its identifier at 42..49. Swapping or dropping either tag changes the ordered tree. |
| `Parser.parseSimpleTag`, `Parser.parseUnknownTag` | `@deprecated` yields DeprecatedTag 325 at 106..118; `@internal` takes the unknown-tag branch, yielding kind 322 at 159..169. |
| `Parser.parseJSDocIdentifierName` | Tag-name identifiers are separate nodes with the token ranges above, excluding `@` and following whitespace. |
| `Parser.parseTagComments` | Parameter and return descriptions become separate JSDocText nodes at 28..41 and 50..59 under their respective tags. |

The subtree observation does not include comment string contents. It therefore
cannot certify whitespace/text normalization helpers such as
`removeLeadingNewlines` or `removeTrailingWhitespace`. No such links are
added. The TS `@see`/`@link` eager exception, malformed-tag diagnostics and
parser-state restoration after malformed comments need their own fixtures.

## JavaScript JSDoc reparsing

Case `syntax/parse-outputs/reparsed-clones-from-jsdoc`, action `side_fields`,
records `ordered[0][4]`: ten clones, each with kind, range, flags, parent kind
and membership in the resulting tree. Native dispatch enters `reparseTags`
from `withJSDoc`. The Rust homes are the corresponding methods in
`crates/tsr_parser/src/reparser.rs`.

| Operations in `reparser.go` | Observable discriminator |
| --- | --- |
| `Parser.reparseTags`, `Parser.addDeepCloneReparse` | Ten clones exist in source order; all retain their source ranges, are in the tree and carry flags 4259848 (JavaScript/JSDoc/reparsed). |
| `Parser.reparseHosted` | `@type {number}` produces a NumberKeyword clone at 11..17 attached to a VariableDeclaration (261); parameter and return type references are attached to their actual parameter/function parents. |
| `Parser.reparseUnhosted` | `@typedef` produces the TypeLiteral clone at 70..81 and name clone at 83..84, both attached to the synthetic JSTypeAliasDeclaration parent (345); the overload produces parameter clones independently of the real implementation signature. |
| `Parser.gatherTypeParameters` | The template `U` clone is a TypeParameter (169) at 105..106 with FunctionDeclaration parent (263). |
| `Parser.reparseJSDocSignature`, `Parser.reparseJSDocTypeLiteral` | The overload's StringKeyword at 239..245 and identifier at 247..248 have Parameter parents (170); the ordinary parameter's `U` type reference at 118..119 also reaches this type-cloning path. This witnesses `reparseJSDocTypeLiteral`'s ordinary-type fallback, **not** conversion of a JSDoc property-tag type literal. |
| `Parser.makeNewCast` | The inline type cast's `any` clone at 43..46 has an AsExpression parent (235), while `@satisfies`'s `T` clone at 188..189 has a SatisfiesExpression parent (239). |

This output does not independently observe every synthesized parent's flags
and span. It is not assigned to `finishReparsedNode` solely because that
helper is reached. Invalid-name rewriting, optional/rest parameters, nested
JSDoc namespaces and property-tag type-literal conversion remain separate
unresolved witnesses.

## Validation of this attribution change

- All 36 newly linked IDs exist in the pinned operation inventory and have
  explicit production port homes; no name-inferred implementation question is
  converted to covered by this change.
- The existing child-free `operation_coverage_problems` validation passes for
  all 32 requests in the two request files, including unchanged request hashes,
  exact action names and agreement between action links and case operations.
- All six complete native/Rust observations match in the inspected F5a capture.
  No broad compilation, new producer capture or benchmark was run for this
  source-and-observation audit.
