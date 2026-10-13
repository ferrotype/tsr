# Shared type and symbol observations

This collector is extracted from the native type-baseline walker. It performs
one file's pinned preorder traversal and selected type or symbol queries using
an already-held operation. The caller owns file lookup, checker dispatch,
type-before-symbol scheduling and error/panic handling. `rows.rs` contains only
owned byte/position observations, without AST or checker handles.

The native host in `crates/tsr_testrunner/src/harness/typebaseline` includes this
module. Its existing `FileCheckers` selection, AST lookup scan, trace/count
hooks, deferred TypeToString dispatch and timing calls are preserved.
`decorate.rs` composes `Symbol(...)`, escapes names, chooses declaration
basenames, masks library positions, truncates declaration text and writes the
baseline envelope. The collector returns raw symbol names, at most the first
five declaration locations and the total declaration count; it does not
compose that envelope.

A public embedding consumer can include the same source with this parent
alias module:

```rust
mod walker_api {
    pub use crate::compiler_protocol::observation_rows;
    pub use tsr_embed::{
        Program, QueryOperation as Operation, TypeRef,
        type_flags as tf, type_format_flags as ff, symbol_format_flags,
        node_builder_flags as builder_flags,
        node_builder_internal_flags as builder_internal_flags,
    };
}
```

It supplies `CompilerOperations::for_file(file)` to `Collector::new`, keeping
all semantic checker queries on the public embedding surface. AST, scanner,
printer and string APIs provide public syntax/display host values. The opaque
`owner_key` identifies the caller's originating checker/file for deferred
TypeToString; it is never transported. Deferred TypeRefs remain scoped query
handles and require explicit retention to outlive their owner.

The native runner and persistent external compiler consumer use this collector;
the extraction alone does not certify full compiler parity. The older measured
`tools/s08/p5/baseline` implementation is an existing duplicate;
it remains unchanged to preserve its display timer hooks and AST-directory
lookup fast path until a separate coordinated migration. No measured receipt
has been re-frozen for this extraction.
