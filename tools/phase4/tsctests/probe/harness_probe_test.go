package tsctests

// Phase 4 X0: the native expectations of the Rust harness's unit tests
// (tools/phase4/tsctests/src/tests.rs). An access-only overlay:
// tools/phase4/tsctests/probe/capture.py adds this file to the pinned package
// as phase4_harness_probe_test.go (it replaces nothing) and runs
// TestPhase4HarnessProbe, which drives the pinned harness functions over fixed
// inputs and writes their results to $PHASE4_PROBE_OUTPUT. The capture script
// wraps them with the pin and this file's digest into
// tools/phase4/tsctests/testdata/probe.json.
//
// The inputs are written here and copied into the fixture beside the results,
// so the Rust tests replay the same steps from the fixture alone. Steps that
// mutate a fake system are data ("op" records) interpreted the same way on
// both sides.

import (
	"encoding/hex"
	stdjson "encoding/json"
	"fmt"
	"os"
	"runtime"
	"slices"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/microsoft/TypeScript/tsc/internal/collections"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnostics"
	"github.com/microsoft/TypeScript/tsc/internal/execute/incremental"
	"github.com/microsoft/TypeScript/tsc/internal/execute/watchmanager"
	"github.com/microsoft/TypeScript/tsc/internal/fswatch"
	"github.com/microsoft/TypeScript/tsc/internal/json"
	"github.com/microsoft/TypeScript/tsc/internal/locale"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/fsbaselineutil"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/harnessutil"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

// The version placeholder of probe texts: both sides substitute their
// core.Version().
const phase4Version = "{{VERSION}}"

func phase4Expand(text string) string {
	return strings.ReplaceAll(text, phase4Version, core.Version())
}

// phase4Text is a string as JSON text when it is UTF-8, else as hex.
func phase4Text(s string) map[string]string {
	if utf8.ValidString(s) {
		return map[string]string{"text": s}
	}
	return map[string]string{"hex": hex.EncodeToString([]byte(s))}
}

// phase4Try runs fn and reports its result or its panic.
func phase4Try(fn func() string) (result map[string]any) {
	defer func() {
		if r := recover(); r != nil {
			result = map[string]any{"panic": fmt.Sprint(r)}
		}
	}()
	return map[string]any{"result": phase4Text(fn())}
}

func phase4Sanitizer() []map[string]any {
	inputs := []string{
		"",
		"hello\nworld\n",
		"hello\r\nworld\r\n",
		"build starting at 10:20:30 AM\nsomething\nbuild finished in 1.234s\n",
		"a\n!!! List files start\n/home/a.ts\nbuild starting at 1:00:00 AM\n!!! List files end\nb\n",
		"!!! List files start\n!!! List files end\n!!! List files start\nx\n!!! List files end\n",
		"!!! Statistics start\nFiles: 3\nLines: 10\n!!! Statistics end\nafter\n",
		"!!! Trace start\n======== Resolving module 'x' ========\n!!! Trace end\n",
		"!!! List files start\n!!! Trace start\nx\n!!! List files end\n",
		" !!! List files start\nnot a block\n",
		"!!! Build Status Report Start\n10:20:30 AM - Projects in this build: \n    * tsconfig.json\n\n!!! Build Status Report End\n",
		"!!! Build Status Report Start\n[\x1b[90m10:20:30 AM\x1b[0m] Projects in this build: \r\n    * a/tsconfig.json\r\n\r\n!!! Build Status Report End\n",
		"!!! Build Status Report Start\n1:02:03 PM - Project 'a' is up to date\n!!! Build Status Report End\n",
		"!!! Watch Status Report Start\n\x1b[2J\x1b[3J\x1b[H[\x1b[90m9:00:00 AM\x1b[0m] Starting compilation in watch mode...\n\n!!! Watch Status Report End\n",
		"!!! Watch Status Report Start\n10:00:00 PM - Found 0 errors. Watching for file changes.\n\n!!! Watch Status Report End\n",
		"!!! Build Status Report Start\nno timestamp here\n!!! Build Status Report End\n",
		"!!! Build Status Report Start\n1:2\n!!! Build Status Report End\n",
		"!!! Build Status Report Start\n\n!!! Build Status Report End\n",
		"!!! Build Status Report Start\nx:y\n!!! Build Status Report End\n",
		"!!! List files start\nunterminated\n",
		"!!! Statistics start\nunterminated\n",
		englishVersion + "\n" + "'" + core.Version() + "' and '" + core.Version() + "'\n" + czechVersion + "\n",
		"\uFFFD@foo@123 and \uFFFD@bar@4\n",
		"!!! Build Status Report Start\n10:20:30 AM - \uFFFD@foo@123 " + englishVersion + "\n!!! Build Status Report End\n",
	}
	results := make([]map[string]any, 0, len(inputs))
	for _, input := range inputs {
		sanitize := func(forComparing bool) map[string]any {
			return phase4Try(func() string {
				sys := &TestSys{currentWrite: &strings.Builder{}}
				sys.currentWrite.WriteString(input)
				return sys.getOutput(forComparing)
			})
		}
		results = append(results, map[string]any{
			"input":     phase4Text(input),
			"output":    sanitize(false),
			"comparing": sanitize(true),
		})
	}
	return results
}

func phase4SymbolNames() []map[string]any {
	inputs := []string{
		"no symbol",
		"\uFFFD@foo@123",
		"x\uFFFD@foo@123y",
		"\uFFFD@foo@",
		"\uFFFD@@123",
		"\uFFFD@a@b@12",
		"\uFFFD@na\nme@99",
		"\uFFFD@foo@123\uFFFD@bar@456",
		"\uFFFD@foo@12a",
		"\xff@foo@1 \uFFFD@x@2",
		"\xff@foo@1",
		"\uFFFD@é@7",
		"\uFFFD\uFFFD@a@1",
		"@a@1",
		"\uFFFD@a\xff@3",
		"\uFFFD@x@1@2",
	}
	results := make([]map[string]any, 0, len(inputs))
	for _, input := range inputs {
		results = append(results, map[string]any{
			"input":  phase4Text(input),
			"output": phase4Text(fsbaselineutil.SanitizeInternalSymbolName(input)),
		})
	}
	return results
}

type phase4TraceStep struct {
	Msg      string `json:"msg"`
	UseCache bool   `json:"use_cache"`
	Other    bool   `json:"other"`
	Reset    bool   `json:"reset"`
}

func phase4Tracer() []map[string]any {
	cases := []struct {
		caseSensitive bool
		cwd           string
		steps         []phase4TraceStep
	}{
		{true, "/home/src/workspaces/project", []phase4TraceStep{
			{Msg: "Found 'package.json' at '/a/package.json'.", UseCache: true},
			{Msg: "Found 'package.json' at '/a/package.json'.", UseCache: true},
			{Msg: "File '/a/package.json' exists according to earlier cached lookups.", UseCache: true},
			{Msg: "File '/c/package.json' exists according to earlier cached lookups.", UseCache: true},
			{Msg: "File '/c/package.json' exists according to earlier cached lookups.", UseCache: true},
			{Msg: "File '/b/package.json' does not exist.", UseCache: true},
			{Msg: "File '/b/package.json' does not exist.", UseCache: true},
			{Msg: "File '/d/package.json' does not exist according to earlier cached lookups.", UseCache: true},
			{Msg: "File '/d/package.json' does not exist according to earlier cached lookups.", UseCache: true},
			{Msg: "File '/e/package.json' does not exist according to earlier cached lookups.", Other: true},
			{Msg: "File '/e/package.json' exists according to earlier cached lookups.", Other: true},
			{Msg: "File '/e/package.json' does not exist.", Other: true},
			{Msg: "Found 'package.json' at '/e/package.json'.", Other: true},
			{Msg: "File 'x/package.json' does not exist.", UseCache: true},
			{Msg: "File '/home/src/workspaces/project/x/package.json' does not exist.", UseCache: true},
			{Reset: true},
			{Msg: "Found 'package.json' at '/a/package.json'.", UseCache: true},
			{Msg: "Version '" + phase4Version + "' and '" + phase4Version + "'", UseCache: true},
			{Msg: "Resolving module 'x' from '/a.ts'.", UseCache: true},
		}},
		{false, "/Home/Project", []phase4TraceStep{
			{Msg: "Found 'package.json' at '/A/package.json'.", UseCache: true},
			{Msg: "Found 'package.json' at '/a/PACKAGE.json'.", UseCache: true},
			{Msg: "File 'X/package.json' does not exist.", UseCache: true},
			{Msg: "File '/home/project/x/package.json' does not exist.", UseCache: true},
		}},
	}
	results := make([]map[string]any, 0, len(cases))
	for _, c := range cases {
		builder := &strings.Builder{}
		other := &strings.Builder{}
		tracer := harnessutil.NewTracerForBaselining(tspath.ComparePathsOptions{
			UseCaseSensitiveFileNames: c.caseSensitive,
			CurrentDirectory:          c.cwd,
		}, builder)
		for _, step := range c.steps {
			switch {
			case step.Reset:
				tracer.Reset()
			case step.Other:
				tracer.TraceWithWriter(other, phase4Expand(step.Msg), step.UseCache)
			default:
				tracer.TraceWithWriter(builder, phase4Expand(step.Msg), step.UseCache)
			}
		}
		tracer.Trace(diagnostics.File_0_does_not_exist, "/z/package.json")
		tracer.Trace(diagnostics.File_0_does_not_exist, "/z/package.json")
		results = append(results, map[string]any{
			"case_sensitive": c.caseSensitive,
			"cwd":            c.cwd,
			"steps":          c.steps,
			"builder":        builder.String(),
			"other":          other.String(),
		})
	}
	return results
}

type phase4WatchRequest struct {
	Dir       string `json:"dir"`
	Recursive bool   `json:"recursive"`
	Ignore    string `json:"ignore,omitempty"`
}

type phase4Change struct {
	Path    string `json:"path"`
	Deleted bool   `json:"deleted,omitempty"`
}

type phase4WatchStep struct {
	Op       string               `json:"op"`
	Requests []phase4WatchRequest `json:"requests,omitempty"`
	Dir      string               `json:"dir,omitempty"`
	Changes  []phase4Change       `json:"changes,omitempty"`
}

func phase4Watch() []map[string]any {
	existing := []string{
		"/home/src/workspaces/project",
		"/home/src/workspaces/project/src",
		"/home/src/workspaces/project/node_modules",
		"/home/src/tslibs/TS/Lib",
		"/Users/Me/Project",
	}
	cases := []struct {
		caseSensitive bool
		steps         []phase4WatchStep
	}{
		{true, []phase4WatchStep{
			{Op: "has_watches"},
			{Op: "state"},
			{Op: "watch", Requests: []phase4WatchRequest{
				{Dir: "/home/src/workspaces/project", Recursive: true, Ignore: "/node_modules/"},
				{Dir: "/home/src/tslibs/TS/Lib"},
			}},
			{Op: "has_watches"},
			{Op: "state"},
			{Op: "watch", Requests: []phase4WatchRequest{{Dir: "/missing"}}},
			{Op: "watch", Requests: []phase4WatchRequest{{Dir: "/home/src/workspaces/project/src"}, {Dir: "/nowhere"}}},
			{Op: "state"},
			{Op: "watch", Requests: []phase4WatchRequest{{Dir: "/home/src/workspaces/project/src"}}},
			{Op: "send_changed_paths", Changes: []phase4Change{
				{Path: "/home/src/workspaces/project/src/a.ts"},
				{Path: "/home/src/workspaces/project/src/deep/b.ts"},
				{Path: "/home/src/workspaces/project/node_modules/x/index.d.ts"},
				{Path: "/home/src/workspaces/projectx/c.ts"},
				{Path: "/home/src/tslibs/TS/Lib/lib.d.ts", Deleted: true},
				{Path: "/home/src/workspaces/project/src/a.ts", Deleted: true},
			}},
			{Op: "overflow"},
			{Op: "close", Dir: "/home/src/tslibs/TS/Lib"},
			{Op: "state"},
			{Op: "send_changed_paths", Changes: []phase4Change{{Path: "/home/src/tslibs/TS/Lib/lib.es5.d.ts"}}},
			{Op: "watch", Requests: []phase4WatchRequest{{Dir: "/home/src/workspaces/project", Recursive: false}}},
			{Op: "state"},
			{Op: "close", Dir: "/home/src/workspaces/project"},
			{Op: "close", Dir: "/home/src/workspaces/project/src"},
			{Op: "state"},
			{Op: "has_watches"},
		}},
		{false, []phase4WatchStep{
			{Op: "watch", Requests: []phase4WatchRequest{{Dir: "/Users/Me/Project", Recursive: true}}},
			{Op: "send_changed_paths", Changes: []phase4Change{
				{Path: "/users/me/project/A.ts"},
				{Path: "/USERS/ME/PROJECT/b/c.ts"},
				{Path: "/Users/Me/Projectile/d.ts"},
			}},
			{Op: "state"},
		}},
	}
	results := make([]map[string]any, 0, len(cases))
	for _, c := range cases {
		backend := NewMockWatchBackend()
		backend.DirectoryExists = func(dir string) bool { return slices.Contains(existing, dir) }
		backend.UseCaseSensitiveFileNames = c.caseSensitive
		var received []string
		closers := map[string]interface{ Close() error }{}
		var outputs []any
		for _, step := range c.steps {
			switch step.Op {
			case "has_watches":
				outputs = append(outputs, backend.HasWatches())
			case "state":
				outputs = append(outputs, backend.WatchState())
			case "watch":
				outputs = append(outputs, phase4WatchRegister(backend, step.Requests, &received, closers))
			case "close":
				outputs = append(outputs, fmt.Sprint(closers[step.Dir].Close()))
			case "send_changed_paths":
				received = nil
				var changes []fsbaselineutil.FileChange
				for _, change := range step.Changes {
					changes = append(changes, fsbaselineutil.FileChange{Path: change.Path, Deleted: change.Deleted})
				}
				backend.SendChangedPaths(changes)
				slices.Sort(received)
				outputs = append(outputs, received)
			case "overflow":
				received = nil
				backend.SendOverflow()
				slices.Sort(received)
				outputs = append(outputs, received)
			}
		}
		results = append(results, map[string]any{
			"case_sensitive": c.caseSensitive,
			"existing":       existing,
			"steps":          c.steps,
			"outputs":        outputs,
		})
	}
	return results
}

// phase4WatchRegister registers requests whose callbacks record what they
// receive as "<watch>|<kind>|<path>" (sorted after each delivery, since the
// mock invokes callbacks in map order) or "<watch>|error|<err>".
func phase4WatchRegister(backend *MockWatchBackend, requests []phase4WatchRequest, received *[]string, closers map[string]interface{ Close() error }) string {
	var pinned []watchmanager.WatchDirectoryRequest
	for _, r := range requests {
		dir := r.Dir
		ignore := r.Ignore
		request := watchmanager.WatchDirectoryRequest{
			Dir:       dir,
			Recursive: r.Recursive,
			Callback: func(events []fswatch.Event, err error) {
				if err != nil {
					*received = append(*received, dir+"|error|"+err.Error())
				}
				for i, e := range events {
					*received = append(*received, fmt.Sprintf("%s|%03d|%s|%s", dir, i, e.Kind, e.Path))
				}
			},
		}
		if ignore != "" {
			request.Ignore = func(path string) bool { return strings.Contains(path, ignore) }
		}
		pinned = append(pinned, request)
	}
	created, err := backend.WatchDirectories(pinned)
	if err != nil {
		return "error: " + err.Error()
	}
	for i, closer := range created {
		closers[requests[i].Dir] = closer
	}
	return "ok"
}

func phase4Clock() map[string]any {
	start := time.Unix(1_700_000_000, 500)
	clock := &TestClock{start: start}
	var readings []int64
	for range 3 {
		readings = append(readings, int64(clock.Now().Sub(start)/time.Second))
	}
	since := clock.SinceStart()
	readings = append(readings, int64(clock.Now().Sub(start)/time.Second))
	return map[string]any{"readings": readings, "since_start_ns": since.Nanoseconds()}
}

func phase4SubFolders() []map[string]any {
	inputs := [][]string{
		nil, {}, {"-b"}, {"--b"}, {"-build"}, {"--build"}, {"-B"}, {"--BUILD"},
		{"-w"}, {"--w"}, {"-watch"}, {"--watch"}, {"-W"}, {"-b", "-w"}, {"-p", "x", "--build", "--watch"},
		{"--watchFile"}, {"-b=true"}, {"build"},
	}
	results := make([]map[string]any, 0, len(inputs))
	for _, args := range inputs {
		results = append(results, map[string]any{
			"args":   args,
			"folder": (&tscInput{commandLineArgs: args}).getBaselineSubFolder(),
		})
	}
	return results
}

func phase4EmitKinds() []string {
	var results []string
	for kind := range 64 {
		results = append(results, toReadableFileEmitKind(incremental.FileEmitKind(kind)))
	}
	return results
}

func phase4PathIsUnder() []map[string]any {
	inputs := []struct {
		event, dir             string
		recursive, sensitive   bool
	}{
		{"/a/b", "/a", false, true},
		{"/a/b/c", "/a", false, true},
		{"/a/b/c", "/a", true, true},
		{"/a", "/a", true, true},
		{"/ab", "/a", true, true},
		{"/A/b", "/a", true, true},
		{"/A/b", "/a", true, false},
		{"/a/B/c", "/A", false, false},
		{"/a/", "/a", false, true},
		{"/a//b", "/a", false, true},
		{"/a/b", "/", true, true},
		{"/a/b", "", true, true},
		{"D:/x/y", "D:/x", false, true},
		{"d:/X/y", "D:/x", false, false},
	}
	results := make([]map[string]any, 0, len(inputs))
	for _, in := range inputs {
		results = append(results, map[string]any{
			"event": in.event, "dir": in.dir, "recursive": in.recursive, "case_sensitive": in.sensitive,
			"under": pathIsUnder(in.event, in.dir, in.recursive, in.sensitive),
		})
	}
	return results
}

func phase4TerminalWidth() []map[string]any {
	values := []*string{nil}
	for _, v := range []string{"", "80", "-5", "+7", "007", "abc", "8 0", "99999999999999999999", "0x10"} {
		values = append(values, &v)
	}
	results := make([]map[string]any, 0, len(values))
	for _, value := range values {
		sys := &TestSys{}
		entry := map[string]any{"value": value}
		if value != nil {
			sys.env = map[string]string{"TS_TEST_TERMINAL_WIDTH": *value}
		}
		func() {
			defer func() {
				if r := recover(); r != nil {
					entry["panic"] = fmt.Sprint(r)
				}
			}()
			entry["width"] = sys.GetWidthOfTerminal()
		}()
		results = append(results, entry)
	}
	return results
}

func phase4Readable() []map[string]any {
	texts := []string{
		`{"version":"FakeTSVersion"}`,
		`{"version":"FakeTSVersion","errors":true,"checkPending":true,"root":["./a.ts","./b.ts"],"semanticErrors":true}`,
		`{"version":"FakeTSVersion","root":[],"fileNames":[],"fileInfos":[]}`,
		`{"version":"FakeTSVersion","root":[[2,4],5],"packageJsons":["../package.json"],"missingPackageJsons":["./package.json","../../package.json"],"fileNames":["lib.d.ts","./a.ts","./b.ts","./c.ts","./d.ts"],` +
			`"fileInfos":["v1",{"version":"v2","noSignature":true,"affectsGlobalScope":true},{"version":"v3","signature":"s3","impliedNodeFormat":99},{"version":"v4","impliedNodeFormat":1},{"version":"v5","noSignature":true,"impliedNodeFormat":100}],` +
			`"fileIdsList":[[2,3],[4],[]],"options":{"composite":true,"declaration":true,"declarationMap":true,"outDir":"./out","target":99,"lib":["lib.es5.d.ts"],"paths":{"@x/*":["./x/*"]}},` +
			`"referencedMap":[[2,1],[3,2],[5,3]],` +
			`"semanticDiagnosticsPerFile":[1,[2,[{"pos":1,"end":2,"code":2322,"category":1,"messageKey":"Type_0_is_not_assignable_to_type_1_2322","messageArgs":["number","string"],"messageChain":[{"pos":0,"end":0,"code":2322,"category":1,"messageKey":"Type_0_is_not_assignable_to_type_1_2322","messageArgs":["a","b"]}],"relatedInformation":[{"file":3,"pos":5,"end":6,"code":2728,"category":3,"messageKey":"_0_is_declared_here_2728","messageArgs":["x"]}]}]],[3,[]],[4,[{"noFile":true,"code":6053,"category":1,"messageKey":"File_0_not_found_6053","messageArgs":["/x.ts"],"reportsUnnecessary":true,"reportsDeprecated":true,"skippedOnNoEmit":true}]],` +
			`[5,[{"pos":3,"end":4,"code":2307,"category":1,"messageKey":"Cannot_find_module_0_or_its_corresponding_type_declarations_2307","messageArgs":["x"],"repopulateInfo":{"kind":2,"moduleReference":"x","mode":99}},{"pos":3,"end":4,"code":1,"category":0,"messageKey":"k","repopulateInfo":{"kind":1,"packageName":"p"}}]]],` +
			`"emitDiagnosticsPerFile":[[2,[{"pos":7,"end":8,"code":4025,"category":1,"messageKey":"Exported_variable_0_has_or_is_using_private_name_1_4025","messageArgs":["a","B"]}]]],` +
			`"changeFileSet":[3,5],"affectedFilesPendingEmit":[2,[3],[4,17],[5,1]],"latestChangedDtsFile":"./out/b.d.ts",` +
			`"emitSignatures":[2,[3,"sig3"],[4,[]],[5,["sig5"]]],"resolvedRoot":[[4,5]]}`,
		`{"version":"FakeTSVersion","fileNames":["./a.ts"],"fileInfos":["h"],"options":{"sourceMap":true,"inlineSourceMap":false},"affectedFilesPendingEmit":[1]}`,
		`{"version":"FakeTSVersion","fileNames":["./a.ts"],"fileInfos":["h"],"options":{"noEmit":true,"declaration":true},"affectedFilesPendingEmit":[1,[1,0]]}`,
		`{"version":"FakeTSVersion","fileNames":["./a.ts"],"fileInfos":["h"],"options":{"emitDeclarationOnly":true,"declaration":true,"declarationMap":true},"affectedFilesPendingEmit":[1]}`,
		`{"version":"FakeTSVersion","root":["./a.ts"],"errors":true,"semanticErrors":true,"packageJsons":["./package.json"]}`,
		`{"version":"FakeTSVersion","fileNames":["./\u00e9.ts","./\uFFFD@x@12.ts"],"fileInfos":["h-\uFFFD@x@12","g"],"root":[[1,2]],"latestChangedDtsFile":"./\u00e9.d.ts"}`,
	}
	results := make([]map[string]any, 0, len(texts))
	for _, text := range texts {
		entry := map[string]any{"text": text}
		var buildInfo incremental.BuildInfo
		if err := json.Unmarshal([]byte(text), &buildInfo); err != nil {
			entry["error"] = err.Error()
		} else {
			entry["readable"] = phase4Try(func() string { return toReadableBuildInfo(&buildInfo, text) })
		}
		results = append(results, entry)
	}
	return results
}

type phase4FsStep struct {
	Op     string   `json:"op"`
	Path   string   `json:"path,omitempty"`
	Text   string   `json:"text,omitempty"`
	Target string   `json:"target,omitempty"`
	Files  []string `json:"files,omitempty"`
	Cache  []string `json:"cache,omitempty"`
}

type phase4FsCase struct {
	Files            map[string]string `json:"files"`
	Symlinks         map[string]string `json:"symlinks"`
	Cwd              string            `json:"cwd"`
	IgnoreCase       bool              `json:"ignore_case"`
	WindowsStyleRoot string            `json:"windows_style_root"`
	Steps            []phase4FsStep    `json:"steps"`
}

// phase4FsSteps runs the steps of a fake-system case and returns one output
// per step.
func phase4FsSteps(c phase4FsCase) []any {
	files := FileMap{}
	for path, text := range c.Files {
		files[path] = phase4Expand(text)
	}
	for path, target := range c.Symlinks {
		files[path] = vfstest.Symlink(target)
	}
	sys := newTestSys(&tscInput{files: files, cwd: c.Cwd, ignoreCase: c.IgnoreCase, windowsStyleRoot: c.WindowsStyleRoot}, false)
	readings := func() int64 {
		sys.clock.nowMu.Lock()
		defer sys.clock.nowMu.Unlock()
		if sys.clock.now.IsZero() {
			return 0
		}
		return int64(sys.clock.now.Sub(sys.clock.start) / time.Second)
	}
	var outputs []any
	for _, step := range c.Steps {
		output := phase4Try(func() string {
			switch step.Op {
			case "baseline":
				var builder strings.Builder
				sys.baselineFSwithDiff(&builder)
				return builder.String()
			case "changed_paths":
				changes := sys.fsDiffer.ChangedPaths()
				var deleted []string
				var lines []string
				for _, change := range changes {
					if change.Deleted {
						deleted = append(deleted, "deleted "+change.Path)
					} else {
						lines = append(lines, "changed "+change.Path)
					}
				}
				slices.Sort(deleted)
				return strings.Join(append(lines, deleted...), "\n")
			case "write":
				sys.writeFileNoError(step.Path, phase4Expand(step.Text))
			case "fs_write":
				if err := sys.FS().WriteFile(step.Path, phase4Expand(step.Text)); err != nil {
					return "error: " + err.Error()
				}
			case "fs_read":
				text, ok := sys.FS().ReadFile(step.Path)
				if !ok {
					return "<missing>"
				}
				return strings.ReplaceAll(text, core.Version(), phase4Version)
			case "remove":
				sys.removeNoError(step.Path)
			case "fs_remove":
				if err := sys.FS().Remove(step.Path); err != nil {
					return "error: " + err.Error()
				}
			case "touch":
				if err := sys.FS().Chtimes(step.Path, time.Time{}, sys.Now()); err != nil {
					return "error: " + err.Error()
				}
			case "symlink":
				sys.mapFs().AddSymlink(step.Path, step.Target)
			case "readings":
				return fmt.Sprint(readings())
			case "written":
				written := sys.fs.writtenFiles.ToSlice()
				slices.Sort(written)
				return strings.Join(written, "\n")
			case "default_libs":
				if sys.fs.defaultLibs == nil {
					return "<nil>"
				}
				libs := sys.fs.defaultLibs.ToSlice()
				slices.Sort(libs)
				return fmt.Sprint(len(libs))
			case "emitted":
				cache := &collections.SyncMap[tspath.Path, time.Time]{}
				for _, path := range step.Cache {
					cache.Store(tspath.Path(path), time.Time{})
				}
				before := readings()
				sys.OnEmittedFiles(&compiler.EmitResult{EmittedFiles: step.Files}, cache)
				var entries []string
				for _, path := range step.Cache {
					value, _ := cache.Load(tspath.Path(path))
					if value.IsZero() {
						entries = append(entries, path+"=zero")
					} else {
						entries = append(entries, fmt.Sprintf("%s=+%d", path, int64(value.Sub(sys.clock.start)/time.Second)-before))
					}
				}
				return fmt.Sprintf("readings +%d\n%s", readings()-before, strings.Join(entries, "\n"))
			case "emitted_nil":
				sys.OnEmittedFiles(nil, nil)
			case "trace":
				other := &strings.Builder{}
				trace := sys.GetTrace(sys.Writer(), locale.Default)
				trace(diagnostics.Found_package_json_at_0, "/a/package.json")
				trace(diagnostics.Found_package_json_at_0, "/a/package.json")
				traceOther := sys.GetTrace(other, locale.Default)
				traceOther(diagnostics.File_0_does_not_exist, "/b/package.json")
				traceOther(diagnostics.File_0_does_not_exist, "/b/package.json")
				sys.OnListFilesStart(other)
				sys.OnListFilesEnd(other)
				sys.OnStatisticsStart(other)
				sys.OnStatisticsEnd(other)
				sys.OnBuildStatusReportStart(other)
				sys.OnBuildStatusReportEnd(other)
				sys.OnWatchStatusReportStart()
				sys.OnWatchStatusReportEnd()
				return sys.currentWrite.String() + "|other|" + other.String()
			case "output":
				return sys.getOutput(false)
			case "clear_output":
				sys.clearOutput()
			case "write_output":
				fmt.Fprint(sys.Writer(), step.Text)
			}
			return ""
		})
		outputs = append(outputs, output)
	}
	return outputs
}

func phase4FsCases() []phase4FsCase {
	buildInfo := `{"version":"` + phase4Version + `","root":[2],"fileNames":["lib.d.ts","./a.ts"],"fileInfos":["h1","h2"],"options":{"composite":true},"latestChangedDtsFile":"./a.d.ts"}`
	return []phase4FsCase{
		{
			Files: map[string]string{
				"/home/src/workspaces/project/a.ts":          "export const a = 1;",
				"/home/src/workspaces/project/sub/b.ts":      "export const b = \uFFFD@b@123;",
				"/home/src/workspaces/project/tsconfig.json": "{}",
				"/home/src/tslibs/TS/Lib/lib.dom.d.ts":       "interface Dom {}",
			},
			Symlinks: map[string]string{
				"/home/src/workspaces/project/link.ts": "/home/src/workspaces/project/a.ts",
				"/home/src/workspaces/linkdir":         "/home/src/workspaces/project/sub",
			},
			Steps: []phase4FsStep{
				{Op: "readings"},
				{Op: "default_libs"},
				{Op: "changed_paths"},
				{Op: "baseline"},
				{Op: "changed_paths"},
				{Op: "baseline"},
				{Op: "write", Path: "/home/src/workspaces/project/a.ts", Text: "export const a = 2;"},
				{Op: "write", Path: "/home/src/workspaces/project/new/c.ts", Text: "c"},
				{Op: "readings"},
				{Op: "fs_write", Path: "/home/src/workspaces/project/tsconfig.json", Text: "{}"},
				{Op: "touch", Path: "/home/src/workspaces/project/sub/b.ts"},
				{Op: "fs_read", Path: "/home/src/tslibs/TS/Lib/lib.es5.d.ts"},
				{Op: "fs_read", Path: "/home/src/tslibs/TS/Lib/lib.dom.d.ts"},
				{Op: "remove", Path: "/home/src/workspaces/project/sub/b.ts"},
				{Op: "remove", Path: "/home/src/workspaces/project/link.ts"},
				{Op: "written"},
				{Op: "changed_paths"},
				{Op: "baseline"},
				{Op: "written"},
				{Op: "baseline"},
				{Op: "write", Path: "/home/src/workspaces/linkdir/d.ts", Text: "d"},
				{Op: "fs_remove", Path: "/home/src/tslibs/TS/Lib/lib.es2015.d.ts"},
				{Op: "remove", Path: "/home/src/workspaces/project/new"},
				{Op: "baseline"},
				{Op: "fs_write", Path: "/home/src/workspaces/project/out/a.tsbuildinfo", Text: buildInfo},
				{Op: "written"},
				{Op: "baseline"},
				{Op: "fs_read", Path: "/home/src/workspaces/project/out/a.tsbuildinfo"},
				{Op: "write", Path: "/home/src/workspaces/project/raw.tsbuildinfo", Text: `{"version":"other","fileNames":["x"]}`},
				{Op: "fs_read", Path: "/home/src/workspaces/project/raw.tsbuildinfo"},
				{Op: "write", Path: "/home/src/workspaces/project/fake.tsbuildinfo", Text: `{"version":"FakeTSVersion","root":["./a.ts"]}`},
				{Op: "fs_read", Path: "/home/src/workspaces/project/fake.tsbuildinfo"},
				{Op: "fs_read", Path: "/home/src/workspaces/project/missing.tsbuildinfo"},
				{Op: "fs_write", Path: "/home/src/workspaces/project/bad.tsbuildinfo", Text: `not json`},
				{Op: "baseline"},
				{Op: "fs_write", Path: "/home/src/workspaces/project/a.js", Text: "a"},
				{Op: "fs_write", Path: "/home/src/workspaces/project/b.js", Text: "b"},
				{Op: "emitted", Files: []string{"/home/src/workspaces/project/a.js", "/home/src/workspaces/project/b.js"}, Cache: []string{"/home/src/workspaces/project/a.js", "/home/src/workspaces/project/x.js"}},
				{Op: "baseline"},
				{Op: "emitted", Files: []string{"/home/src/workspaces/project/a.js"}, Cache: []string{"/home/src/workspaces/project/a.js"}},
				{Op: "emitted", Files: []string{"/home/src/workspaces/project/missing.js"}},
				{Op: "emitted_nil"},
				{Op: "readings"},
				{Op: "write_output", Text: "build starting at 10:00:00 AM\nhello\n"},
				{Op: "trace"},
				{Op: "output"},
				{Op: "clear_output"},
				{Op: "trace"},
			},
		},
		{
			Files: map[string]string{
				"D:/Work/Project/A.ts":           "a",
				"D:/Work/Project/tsconfig.json":  "{}",
				"D:/home/src/tslibs/TS/Lib/lib.d.ts": "custom",
			},
			Cwd:              "D:/Work/Project",
			IgnoreCase:       true,
			WindowsStyleRoot: "D:/",
			Steps: []phase4FsStep{
				{Op: "readings"},
				{Op: "default_libs"},
				{Op: "baseline"},
				{Op: "write", Path: "D:/work/project/a.ts", Text: "b"},
				{Op: "fs_write", Path: "D:/WORK/PROJECT/Out.js", Text: "o"},
				{Op: "changed_paths"},
				{Op: "baseline"},
				{Op: "fs_read", Path: "d:/home/src/tslibs/ts/lib/LIB.ES5.D.TS"},
				{Op: "baseline"},
				{Op: "remove", Path: "D:/work/PROJECT/out.js"},
				{Op: "changed_paths"},
				{Op: "baseline"},
			},
		},
	}
}

type phase4DiffSide struct {
	Writes [][2]string `json:"writes"`
	Raw    [][2]string `json:"raw"`
	Output string      `json:"output"`
}

func phase4IncrementalDiffs() []map[string]any {
	buildInfo := `{"version":"` + phase4Version + `"}`
	cases := []struct {
		NonIncremental phase4DiffSide `json:"non_incremental"`
		Incremental    phase4DiffSide `json:"incremental"`
	}{
		{
			phase4DiffSide{Writes: [][2]string{{"/p/a.js", "x\ny\nz\n"}, {"/p/b.js", "same"}}, Output: "error\n"},
			phase4DiffSide{Writes: [][2]string{{"/p/a.js", "x\nY\nz\n"}, {"/p/b.js", "same"}}, Output: "error\n"},
		},
		{
			phase4DiffSide{Writes: [][2]string{{"/p/c.js", "only in clean"}, {"/p/out.tsbuildinfo", buildInfo}}, Output: "a\n!!! List files start\nl\n!!! List files end\nb\n"},
			phase4DiffSide{Output: "a\nbuild starting at 1:00:00 PM\nc\n"},
		},
		{
			phase4DiffSide{Writes: [][2]string{{"/p/out.tsbuildinfo", buildInfo}, {"/p/d.js", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10"}}},
			phase4DiffSide{Raw: [][2]string{{"/p/out.tsbuildinfo", "{}"}, {"/p/d.js", "1\n2\n3\n4\n5\nsix\n7\n8\n9\n10"}}},
		},
		{
			phase4DiffSide{Output: "same\n"},
			phase4DiffSide{Output: "same\n"},
		},
	}
	results := make([]map[string]any, 0, len(cases))
	for _, c := range cases {
		build := func(side phase4DiffSide, shadow bool) *TestSys {
			sys := newTestSys(&tscInput{files: FileMap{"/p/tsconfig.json": "{}"}, cwd: "/p"}, shadow)
			for _, write := range side.Writes {
				if err := sys.FS().WriteFile(write[0], phase4Expand(write[1])); err != nil {
					panic(err)
				}
			}
			for _, write := range side.Raw {
				sys.writeFileNoError(write[0], phase4Expand(write[1]))
			}
			fmt.Fprint(sys.Writer(), side.Output)
			return sys
		}
		diff := phase4Try(func() string {
			return getDiffForIncremental(build(c.Incremental, false), build(c.NonIncremental, true))
		})
		results = append(results, map[string]any{
			"non_incremental": c.NonIncremental,
			"incremental":     c.Incremental,
			"diff":            diff,
		})
	}
	return results
}

func TestPhase4HarnessProbe(t *testing.T) {
	output := os.Getenv("PHASE4_PROBE_OUTPUT")
	if output == "" {
		t.Fatal("PHASE4_PROBE_OUTPUT is required")
	}
	var fsResults []map[string]any
	for _, c := range phase4FsCases() {
		fsResults = append(fsResults, map[string]any{"case": c, "outputs": phase4FsSteps(c)})
	}
	result := map[string]any{
		"version_placeholder": phase4Version,
		"sanitizer":           phase4Sanitizer(),
		"symbol_names":        phase4SymbolNames(),
		"tracer":              phase4Tracer(),
		"watch":               phase4Watch(),
		"path_is_under":       phase4PathIsUnder(),
		"clock":               phase4Clock(),
		"sub_folders":         phase4SubFolders(),
		"emit_kinds":          phase4EmitKinds(),
		"terminal_width":      phase4TerminalWidth(),
		"readable":            phase4Readable(),
		"fs":                  fsResults,
		"incremental_diff":    phase4IncrementalDiffs(),
		"go":                  runtime.Version(),
	}
	data, err := stdjson.MarshalIndent(result, "", " ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(output, data, 0o644); err != nil {
		t.Fatal(err)
	}
}
