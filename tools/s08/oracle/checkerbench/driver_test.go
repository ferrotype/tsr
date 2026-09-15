package testrunner

// S08 checkerbench Go child (data/s08/checker-workload.json). One fresh
// process runs every request serially: harness setup and parsing, then a
// bound program, then the checker interval (diagnostics in native order and
// the type/symbol walker with its displays), then the retained checkpoint and
// release. S08_MODE selects normal timing, the phase timer or allocation
// accounting; rows go to S08_OUTPUT (NDJSON) and totals to S08_SUMMARY.

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnosticwriter"
	"github.com/microsoft/TypeScript/tsc/internal/repo"
	"github.com/microsoft/TypeScript/tsc/internal/testutil"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/baseline"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/harnessutil"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/tsbaseline"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/osvfs"
)

type s08BenchTotals struct {
	Version    int              `json:"version"`
	Mode       string           `json:"mode"`
	Variants   int              `json:"variants"`
	Executed   int              `json:"executed"`
	Failed     int              `json:"failed"`
	IntervalNs int64            `json:"interval_ns"`
	PhasesNs   map[string]int64 `json:"phases_ns"`
	Allocation map[string]int64 `json:"allocation"`
	Census     map[string]int64 `json:"census"`
	Failures   []map[string]any `json:"failures"`
	ProcessNs  int64            `json:"process_ns"`
	Outputs    string           `json:"outputs_sha256"`
	Actions    string           `json:"actions_sha256"`
	Go         string           `json:"go"`
	GOOS       string           `json:"goos"`
	GOARCH     string           `json:"goarch"`
	Request    string           `json:"request_sha256"`
}

// Same fields, same order as the Rust child's output digest.
func s08BenchDigest(types, symbols, errors tsbaseline.S08Baseline, typeStrings []map[string]any, diagnostics []*ast.Diagnostic) string {
	h := sha256.New()
	part := func(label string, value string) {
		h.Write([]byte(label))
		h.Write([]byte{0})
		h.Write([]byte(value))
		h.Write([]byte{'\n'})
	}
	text := func(b tsbaseline.S08Baseline) string {
		if b.TextHex != nil {
			return *b.TextHex
		}
		return ""
	}
	part("types", text(types))
	part("symbols", text(symbols))
	part("errors", text(errors))
	for _, row := range typeStrings {
		part("tts", row["text_hex"].(string))
	}
	for _, d := range diagnostics {
		file := ""
		if d.File() != nil {
			file = hex.EncodeToString([]byte(d.File().FileName()))
		}
		part("diagnostic", fmt.Sprintf("%d:%s:%d:%d", d.Code(), file, d.Pos(), d.End()))
	}
	return hex.EncodeToString(h.Sum(nil))
}

func TestS08Checkerbench(t *testing.T) {
	mode := os.Getenv("S08_MODE")
	if mode != "normal" && mode != "phase" && mode != "alloc" {
		t.Fatal("S08_MODE must be normal, phase or alloc")
	}
	if !testutil.TestProgramIsSingleThreaded() {
		t.Fatal("checkerbench requires a single-threaded test program (one checker, workers 1)")
	}
	raw, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	var requests []s08Request
	if err := decoder.Decode(&requests); err != nil {
		t.Fatal(err)
	}
	if err := decoder.Decode(new(any)); err != io.EOF {
		t.Fatalf("trailing input: %v", err)
	}
	output, err := os.Create(os.Getenv("S08_OUTPUT"))
	if err != nil {
		t.Fatal(err)
	}
	defer output.Close()
	encoder := json.NewEncoder(output)
	clock := core.S08Bench
	clock.Mode = mode
	harnessutil.S08InstallCheckerbenchCompile()
	defer func() { harnessutil.S08CheckerbenchCompile = nil }()
	tsbaseline.S08CountOnly = true
	tsbaseline.S08CollectRoots = mode == "alloc"
	tsbaseline.S08CollectTypeStrings = true
	defer func() {
		tsbaseline.S08CountOnly = false
		tsbaseline.S08CollectRoots = false
		tsbaseline.S08CollectTypeStrings = false
	}()
	totals := s08BenchTotals{Version: 1, Mode: mode, Variants: len(requests),
		PhasesNs: map[string]int64{"init": 0, "check": 0, "display": 0},
		Allocation: map[string]int64{"requested_bytes": 0, "allocation_calls": 0, "retained_bytes": 0},
		Census:     map[string]int64{"type_storage_bytes": 0, "checker_bytes": 0, "types_reachable": 0, "types_created": 0, "unavailable": 0, "failed": 0},
		Failures:   []map[string]any{}, Go: runtime.Version(), GOOS: runtime.GOOS, GOARCH: runtime.GOARCH}
	sum := sha256.Sum256(raw)
	totals.Request = hex.EncodeToString(sum[:])
	digests := sha256.New()
	actionsHash := sha256.New()
	started := time.Now()
	ctx := context.Background()
	for _, request := range requests {
		row := map[string]any{"id": request.ID, "acceptance_tier": request.AcceptanceTier, "outcome": "failed"}
		clock.Reset()
		clock.Push(core.S08PhaseCheck)
		tsbaseline.S08QueryCounts = map[string]int{}
		tsbaseline.S08Roots = nil
		harnessutil.S08CheckerbenchLastProgram = nil
		var checkpointLive uint64
		t.Run(request.ID, func(t *testing.T) {
			defer func() {
				if value := recover(); value != nil {
					row["failure"] = fmt.Sprint("panic: ", value)
				}
				if t.Skipped() {
					row["failure"] = "native option guard skipped the variant"
				}
			}()
			fail := func(values ...any) {
				row["failure"] = fmt.Sprint(values...)
				t.Fatal(values...)
			}
			path := filepath.Join(repo.RootPath(), strings.TrimPrefix(request.Path, "tsc/"))
			loaded, ok := osvfs.FS().ReadFile(path)
			if !ok || s08Hash([]byte(loaded)) != request.LoadedSHA256 {
				fail("loaded source digest differs")
			}
			payload := makeUnitsFromTest(loaded, path)
			configuration := &harnessutil.NamedTestConfiguration{Config: request.Settings, Name: request.ConfigurationName}
			// Setup, parsing and binding, then the first half of the interval
			// (diagnostics in native order) inside the installed compile.
			c := newCompilerTest(t, request.ID, path, &payload, configuration)
			if c.configuredName != request.ConfiguredName {
				fail("configured name drift ", c.configuredName)
			}
			if request.AcceptanceTier != "informational" {
				harnessutil.SkipUnsupportedCompilerOptions(t, c.options)
			}
			program := c.result.Program
			if program == nil || harnessutil.S08CheckerbenchLastProgram != program {
				fail("checkerbench compile hook did not produce the program")
			}
			diagnostics := c.result.Diagnostics
			types := tsbaseline.S08Baseline{State: "disabled"}
			symbols := tsbaseline.S08Baseline{State: "disabled"}
			if !c.harnessOptions.NoTypesAndSymbols {
				allFiles := core.Filter(core.Concatenate(c.toBeCompiled, c.otherFiles), func(f *harnessutil.TestFile) bool { return program.GetSourceFile(f.UnitName) != nil })
				header := tspath.GetPathFromPathComponents(tspath.GetPathComponentsRelativeTo(repo.TestDataPath(), path, tspath.ComparePathsOptions{}))
				clock.Start()
				types, symbols = tsbaseline.S08TypeSymbolBaselines(program, allFiles, header, len(diagnostics) > 0)
				clock.Stop()
			}
			clock.Pop()
			if clock.Depth() != 0 {
				fail("unbalanced phase clocks")
			}
			// Error rendering is baseline decoration, outside the interval; the
			// native verifyDiagnostics selection applies.
			files := core.Concatenate(c.tsConfigFiles, core.Concatenate(c.toBeCompiled, c.otherFiles))
			renderDiagnostics := diagnostics
			if contentMapped := c.contentMappedFileNames(); len(contentMapped) > 0 {
				files = core.Filter(files, func(f *harnessutil.TestFile) bool {
					return !contentMapped[tspath.GetNormalizedAbsolutePath(f.UnitName, c.currentDirectory)]
				})
				renderDiagnostics = core.Filter(diagnostics, func(d *ast.Diagnostic) bool {
					return d.File() == nil || !contentMapped[d.File().FileName()]
				})
			}
			errorValue := baseline.NoContent
			if len(renderDiagnostics) > 0 {
				errorValue = tsbaseline.GetErrorBaseline(t, files, diagnosticwriter.WrapASTDiagnostics(renderDiagnostics), diagnosticwriter.CompareASTDiagnostics, c.options.Pretty.IsTrue())
			}
			errors := tsbaseline.S08BaselineValue(errorValue)
			actions := map[string]any{}
			for name, count := range tsbaseline.S08QueryCounts {
				actions[name] = count
			}
			if len(tsbaseline.S08TypeStrings) > 0 {
				actions["TypeToString"] = len(tsbaseline.S08TypeStrings)
			}
			actions["diagnostics"] = len(renderDiagnostics)
			row["actions"] = actions
			row["output_sha256"] = s08BenchDigest(types, symbols, errors, tsbaseline.S08TypeStrings, renderDiagnostics)
			row["interval_ns"] = clock.IntervalNs
			if mode == "phase" {
				row["phases_ns"] = map[string]int64{"init": clock.PhaseNs[0], "check": clock.PhaseNs[1], "display": clock.PhaseNs[2]}
			}
			checkerHandle, done := program.Program().GetTypeChecker(ctx)
			checkpoint := map[string]any{"types_created": checkerHandle.TypeCount, "symbols_created": checkerHandle.SymbolCount,
				"signatures_created": checkerHandle.SignatureCount, "roots": len(tsbaseline.S08Roots)}
			if mode == "alloc" {
				// Checker, program, baselines and type strings are live here.
				checkpointLive = harnessutil.S08LiveHeap()
				func() {
					defer func() {
						if value := recover(); value != nil {
							checkpoint["census"] = map[string]any{"state": "failed", "reason": fmt.Sprint(value)}
						}
					}()
					checkpoint["census"] = checker.S08FamiliesCensus(checkerHandle, tsbaseline.S08Roots)
				}()
				row["allocation"] = map[string]any{"requested_bytes": clock.Requested, "allocation_calls": clock.Mallocs,
					"live_before_interval": harnessutil.S08LiveBeforeInterval, "live_at_checkpoint": checkpointLive}
			}
			done()
			row["checkpoint"] = checkpoint
			runtime.KeepAlive(c)
			runtime.KeepAlive(types)
			runtime.KeepAlive(symbols)
			row["outcome"] = "executed"
		})
		// Release: the variant's program, checker and results are unreferenced.
		tsbaseline.S08Roots = nil
		tsbaseline.S08TypeStrings = nil
		harnessutil.S08CheckerbenchLastProgram = nil
		if mode == "alloc" {
			allocation, _ := row["allocation"].(map[string]any)
			if allocation != nil {
				allocation["live_after_release"] = harnessutil.S08LiveHeap()
				retained := int64(checkpointLive) - int64(harnessutil.S08LiveBeforeInterval)
				totals.Allocation["retained_bytes"] += retained
				totals.Allocation["requested_bytes"] += int64(clock.Requested)
				totals.Allocation["allocation_calls"] += int64(clock.Mallocs)
			}
		}
		if row["outcome"] == "executed" {
			totals.Executed++
		} else {
			totals.Failed++
			totals.Failures = append(totals.Failures, map[string]any{"id": request.ID, "failure": row["failure"]})
		}
		if digest, ok := row["output_sha256"].(string); ok {
			digests.Write([]byte(digest))
		}
		digests.Write([]byte{'\n'})
		if actions, ok := row["actions"]; ok {
			encoded, _ := json.Marshal(actions)
			actionsHash.Write(encoded)
		}
		actionsHash.Write([]byte{'\n'})
		totals.IntervalNs += clock.IntervalNs
		for i, name := range core.S08PhaseNames {
			totals.PhasesNs[name] += clock.PhaseNs[i]
		}
		if checkpoint, ok := row["checkpoint"].(map[string]any); ok {
			if census, ok := checkpoint["census"].(map[string]any); ok {
				if census["state"] == "failed" {
					totals.Census["failed"]++
				} else {
					totals.Census["type_storage_bytes"] += census["type_storage_bytes"].(int64)
					totals.Census["checker_bytes"] += census["checker_bytes"].(int64)
					types := census["types"].(map[string]any)
					totals.Census["types_reachable"] += int64(types["reachable"].(int))
					totals.Census["types_created"] += int64(types["created"].(uint32))
					totals.Census["unavailable"] += int64(len(census["unavailable"].([]string)))
				}
			}
		}
		if err := encoder.Encode(row); err != nil {
			t.Fatal(err)
		}
	}
	totals.ProcessNs = time.Since(started).Nanoseconds()
	totals.Outputs = hex.EncodeToString(digests.Sum(nil))
	totals.Actions = hex.EncodeToString(actionsHash.Sum(nil))
	summary, err := json.Marshal(totals)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_SUMMARY"), summary, 0600); err != nil {
		t.Fatal(err)
	}
	_ = compiler.SortAndDeduplicateDiagnostics
}
