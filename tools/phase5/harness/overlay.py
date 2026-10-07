"""Access-only patches for the pinned native test harness; never edit upstream."""
from pathlib import Path
import json

ROOT = Path(__file__).resolve().parents[3]
UPSTREAM = ROOT / "upstream/tsc"
HERE = Path(__file__).resolve().parent


def replace_once(source, anchor, replacement):
    if source.count(anchor) != 1:
        raise ValueError(f"pinned overlay anchor must occur once: {anchor!r}")
    return source.replace(anchor, replacement, 1)


def state_adapter():
    source = (ROOT / "tools/phase5/project/projection_test.go").read_text()
    # Reuse the independently round-tripped projection decoder. The rendering
    # remains the three original Go methods with mechanical type substitutions.
    types = source[source.index("type wireID"):source.index("type action struct")]
    first = types.index("func projectState(")
    last = types.index("type wireFile")
    types = types[:first] + types[last:]
    render = source[source.index("func renderProjection("):source.index("func TestRustProjectStateNative(")]
    # The pin's open-file table comes from the fourslash client's immediate
    # bookkeeping, while projects/configs come from the published snapshot.
    render = replace_once(render, """\twriter.openFiles = map[string]struct{}{}
\tfor _, f := range data.Open {
\t\twriter.openFiles[f.Name] = struct{}{}
\t}
""", "")
    original = (UPSTREAM / "internal/fourslash/statebaseline.go").read_text()
    methods = original[original.index("func (f *FourslashTest) printProjectsDiff"):]
    methods = methods.replace("(f *FourslashTest)", "(f *projectionWriter)")
    methods = methods.replace("snapshot *project.Snapshot", "snapshot *wireSnapshot")
    methods = methods.replace("*compiler.Program", "*wireProgram")
    methods = methods.replace("map[string]projectInfo", "map[string]*wireProgram")
    return '''// Projection data adapter; writer method bodies come from the pin.
package fourslash
import (
 "encoding/json"
 "fmt"
 "io"
 "iter"
 "maps"
 "reflect"
 "slices"
 "strings"
 "testing"
 "github.com/microsoft/TypeScript/tsc/internal/project"
 "github.com/microsoft/TypeScript/tsc/internal/tspath"
 "github.com/microsoft/TypeScript/tsc/internal/vfs"
)
''' + types + render + methods


def create(stage, *, include_l2_fixture=False):
    stage = Path(stage).resolve()
    stage.mkdir(parents=True, exist_ok=True)
    replacements = {}

    def install(relative, text):
        target = stage / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
        replacements[str(UPSTREAM / relative)] = str(target)

    # The pin derives fixture roots from runtime.Caller's build-host path.
    # Downloaded binaries need an explicit checkout root; unset keeps Go intact.
    path = "internal/repo/paths.go"
    repo = (UPSTREAM / path).read_text()
    repo = replace_once(repo, "var rootPath = sync.OnceValue(func() string {", """var rootPath = sync.OnceValue(func() string {
 if root := os.Getenv("TSR_UPSTREAM_ROOT"); root != "" {
  if !filepath.IsAbs(root) { panic("TSR_UPSTREAM_ROOT must be absolute") }
  info, err := os.Stat(filepath.Join(root, "go.mod"))
  if err != nil || !info.Mode().IsRegular() { panic("TSR_UPSTREAM_ROOT must contain go.mod") }
  return filepath.Clean(root)
 }""")
    install(path, repo)

    path = "internal/testutil/lsptestutil/lspclient.go"
    client = (UPSTREAM / path).read_text()
    client = replace_once(client, '"io"', '"io"\n "os"')
    client = replace_once(client, "Server       *lsp.Server", "Server interface { InitComplete() <-chan struct{}; SetCompilerOptionsForInferredProjects(context.Context, *core.CompilerOptions) }")
    anchor = "func NewLSPClient(t *testing.T, serverOpts lsp.ServerOptions, onServerRequest ServerRequestHandler) (*LSPClient, func() error) {"
    client = replace_once(client, anchor, anchor + '\n if os.Getenv("TSR_LSP_SERVER") != "" { return newRustClient(t,serverOpts,onServerRequest) }')
    client = replace_once(client, "server := lsp.NewServer(&serverOpts)", 'if os.Getenv("TSR_LSP_SERVER") != "" { panic("native server reached in Rust mode") }\n server := lsp.NewServer(&serverOpts)')
    client = replace_once(client, "if err := c.writeToServer(msg); err != nil {", "if rustWriteNotification(t, c, msg) { return }\n if err := c.writeToServer(msg); err != nil {")
    install(path, client)
    for name in ("rust_client.go", "rust_mappers.go"):
        replacements[str(UPSTREAM / "internal/testutil/lsptestutil" / name)] = str(HERE / name)
    if include_l2_fixture:
        replacements[str(UPSTREAM / "internal/lsp/l2_options_sync_test.go")] = str(ROOT / "tools/phase5/lsp/options_sync_test.go")

    path = "internal/fourslash/statebaseline.go"
    state = (UPSTREAM / path).read_text()
    state = replace_once(state, '"github.com/microsoft/TypeScript/tsc/internal/ls/lsconv"', '"github.com/microsoft/TypeScript/tsc/internal/ls/lsconv"\n "github.com/microsoft/TypeScript/tsc/internal/lsp"')
    state = replace_once(state, "type stateBaseline struct {", "type stateBaseline struct {\n rustWriter *projectionWriter\n rustDecoder *projectionDecoder")
    state = replace_once(state, "session := f.client.Server.Session()", '''if raw, rust := lsptestutil.RustProjectState(t, f.client); rust {
 if f.stateBaseline.rustWriter == nil {
  f.stateBaseline.rustWriter = &projectionWriter{stateBaseline: &projectionBaseline{}, vfs: f.vfs}
  f.stateBaseline.rustDecoder = newProjectionDecoder()
 }
 var data stateData
 assert.NilError(t,json.Unmarshal(raw,&data))
 f.stateBaseline.rustWriter.openFiles = f.openFiles
 fmt.Fprint(w,renderProjection(t,f.stateBaseline.rustWriter,f.stateBaseline.rustDecoder,data))
 return
}
session := f.client.Server.(*lsp.Server).Session()''')
    install(path, state)
    install("internal/fourslash/rust_state.go", state_adapter())

    path = "internal/testutil/baseline/baseline.go"
    baseline = (UPSTREAM / path).read_text()
    baseline = replace_once(baseline, "writeComparison(t, actual, localPath, referencePath)", '''if os.Getenv("TSR_FAULT") == "baseline" && actual != NoContent { actual += "\\nTSR deliberate baseline fault\\n" }
 var writeError error
 finish := tsrReportBaseline(t, fileName, subfolder, actual, localPath, referencePath, &writeError)
 defer finish()
 writeComparison(t, actual, localPath, referencePath, &writeError)''')
    baseline = replace_once(baseline,
        "func writeComparison(t *testing.T, actualContent string, local, reference string) {",
        "func writeComparison(t *testing.T, actualContent string, local, reference string, writeError *error) {")
    # Observe the original writer's actual error, preserving every assertion and
    # branch. A post-hoc file check can mistake a stale local file for a write.
    for operation in ("create directories for", "remove", "write"):
        expression = f'fmt.Errorf("failed to {operation} the local baseline file %s: %w", local, err)'
        baseline = replace_once(baseline, "t.Error(" + expression + ")",
                                "*writeError = " + expression + "\n t.Error(*writeError)")
    expression = 'fmt.Errorf("failed to write the local baseline file %s: %w", local+".delete", err)'
    baseline = replace_once(baseline, "t.Error(" + expression + ")",
                            "*writeError = " + expression + "\n t.Error(*writeError)")
    baseline = replace_once(baseline, 'localRoot     = filepath.Join(repo.TestDataPath(), "baselines", "local")', 'localRoot = tsrLocalRoot(filepath.Join(repo.TestDataPath(), "baselines", "local"))')
    install(path, baseline)
    replacements[str(UPSTREAM / "internal/testutil/baseline/tsr_reporting.go")] = str(HERE / "reporting.go")
    if any("/internal/fourslash/tests/" in path for path in replacements):
        raise ValueError("test assertion files cannot be overlaid")
    overlay = stage / "overlay.json"
    overlay.write_text(json.dumps({"Replace": replacements}, indent=2) + "\n")
    return overlay
