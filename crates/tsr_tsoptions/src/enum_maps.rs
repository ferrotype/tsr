//! `tsoptions/enummaps.go` tables beyond the option maps.
//!
//! Ports of `tsc/internal/tsoptions/enummaps.go`, witnessed by the `tsoptions` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).

use tsr_core::ScriptTarget;

/// Go's `targetToLibMap`, in no particular order (Go's is a map).
/// port: tsc/internal/tsoptions/enummaps.go:TargetToLibMap
pub fn target_to_lib_map() -> &'static [(ScriptTarget, &'static str)] {
    &[
        (ScriptTarget::ESNEXT, "lib.esnext.full.d.ts"),
        (ScriptTarget::ES2025, "lib.es2025.full.d.ts"),
        (ScriptTarget::ES2024, "lib.es2024.full.d.ts"),
        (ScriptTarget::ES2023, "lib.es2023.full.d.ts"),
        (ScriptTarget::ES2022, "lib.es2022.full.d.ts"),
        (ScriptTarget::ES2021, "lib.es2021.full.d.ts"),
        (ScriptTarget::ES2020, "lib.es2020.full.d.ts"),
        (ScriptTarget::ES2019, "lib.es2019.full.d.ts"),
        (ScriptTarget::ES2018, "lib.es2018.full.d.ts"),
        (ScriptTarget::ES2017, "lib.es2017.full.d.ts"),
        (ScriptTarget::ES2016, "lib.es2016.full.d.ts"),
        (ScriptTarget::ES2015, "lib.es6.d.ts"),
    ]
}
