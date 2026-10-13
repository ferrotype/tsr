# Browser smoke

The runner serves the production `tsr_wasm` checker artifact to the matching
Chromium, Firefox and WebKit engines from Playwright 1.62.1. It applies no
engine stack flags. `toolchains.json` fixes the engine and case rosters.
Both WebAssembly compilation and instantiation are asynchronous on the page,
using `createInstanceAsync`; the synchronous Node entry point is retained.
This is a correctness smoke, not the full compiler corpus or a performance
measurement. The check/API-error case also compiles a real two-file import
with resolution tracing enabled and checks the recorded message identities,
typed argument bytes and start/success order. The depth case includes the
existing S10 fixture with 64 pairs
of parentheses and a 5,001-term left-addition chain from ADR 0011's native
transform source family. It requires parse/encode/check/emit and exact whole
expression output. It does not establish ADR 0011's full stress matrix.

Provision the pinned Playwright package and its matching engines outside
Cargo's `target` and capture output. An installed package can be passed
explicitly; otherwise normal Node package resolution must find it. Browser
installation is a separate prerequisite, never an automatic runner fallback.

```sh
PLAYWRIGHT_BROWSERS_PATH=/absolute/tool-cache/browsers \
node /absolute/tool-cache/node_modules/playwright/cli.js install chromium firefox webkit

PLAYWRIGHT_BROWSERS_PATH=/absolute/tool-cache/browsers \
node tools/phase7/portable/browser/run.mjs \
  --playwright-module /absolute/tool-cache/node_modules/playwright \
  --artifacts target/s10/checker \
  --output /absolute/evidence/r3-browser-new-capture
```

`--artifacts` contains `build.json`, `checker_bg.wasm` and `bindings.mjs`
from `scripts/s10_build.py --checker`. The runner verifies the served wasm
and binding bytes against that build record and snapshots its HTTP inputs
before launching an engine. Captures require a new directory outside every
`target` ancestor, including symlink aliases. `browser-smoke.json` retains
each engine's executable, actual version, user agent, individual case results,
errors and the hashes of the artifact, page, wrapper and fixture bytes.
Before launching engines, the runner copies those exact HTTP snapshot buffers
to `artifacts/` in the capture. `artifacts/manifest.json` maps every served URL
to a preserved filename, MIME type, size and hash; the capture authenticates
the manifest hash. The wasm is saved once. Original paths remain provenance
only: deleting Cargo's `target` cannot remove these archived input bytes.
Any missing engine, missing/reordered case, trap during an ordinary case,
assertion failure or mismatched artifact causes a nonzero exit.

The runner listens on an ephemeral `127.0.0.1` port and serves only these
explicit routes: `/`, `/smoke.mjs`, `/runtime/instance.mjs`,
`/runtime/imports.mjs`, `/artifact/build.json`, `/artifact/checker_bg.wasm`,
`/artifact/bindings.mjs`, and `/fixtures/parser.json`. The HTML page exposes
`globalThis.runSmoke()` and a manual Run button. `startServer(artifactDir)`
is exported by `run.mjs` for a host that needs to inspect the same page.

The trap case calls the existing production parser with `relative.ts`,
violating `AstBuilder::new_source_file`'s normalized absolute filename
precondition. It requires an actual `WebAssembly.RuntimeError`, refusal to
reuse the poisoned instance, invalidation of an existing host, survival of
already copied output bytes, and a successful parse in a fresh production
instance. It adds no fault flags or test-only wasm exports. Recoverable API
errors are exercised separately through queries, retirement and emit.

Focused regressions cover the output-path guard and exact detached input
preservation after source artifact mutation and deletion:

```sh
node --test tools/phase7/portable/browser/test-runner.mjs
```
