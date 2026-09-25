package compiler

// Phase 1 closure, `projectReferences` group: access only. Each request is a
// small in-memory workspace whose tsconfig is parsed for real with the pinned
// tsoptions.GetParsedCommandLineOfConfigFile; the program is then built by the
// pinned loader. Four actions observe it:
//
//   - load: the loader alone (processAllProgramFiles, as the S07 loader bridge
//     runs it) with its own trace: files, missing files, resolutions, the
//     include processor's diagnostics and each file's reference redirect.
//   - graph: Program.RangeResolvedProjectReference, in walk order.
//   - verify: the verifier's raw writes (programDiagnostics and the emit
//     blocking set) and Program.GetProgramDiagnostics.
//   - explain: an include-explaining processing diagnostic built here, as the
//     S07 include-reason witness does, for each named file.
//
// It is an IN-PACKAGE test file because the loader, the raw verifier writes and
// processing diagnostics are unexported. Every name is prefixed `phase1PR` so
// it cannot collide with a pinned test helper. Nothing semantic runs: no
// checker, declaration diagnostics or emit.
//
// The observation is the actions' ordered answers, one flat object each.
// Diagnostics travel as positional arrays, [file, pos, end, code, category,
// key, args, text, chain, related], as in the syntacticDiagnostics group.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"runtime"
	"sort"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnostics"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

type phase1PRWorkspace struct {
	Cwd                     string            `json:"cwd"`
	CaseSensitive           bool              `json:"case_sensitive"`
	Files                   map[string]string `json:"files"`
	Symlinks                map[string]string `json:"symlinks"`
	Project                 string            `json:"project"`
	UseSource               bool              `json:"use_source_of_project_reference"`
	SuppressOutputPathCheck bool              `json:"suppress_output_path_check"`
}

type phase1PRRequest struct {
	Case      string             `json:"case"`
	Operation string             `json:"operation"`
	Subject   string             `json:"subject"`
	Workspace *phase1PRWorkspace `json:"workspace"`
	Actions   []struct {
		Op string `json:"op"`
	} `json:"actions"`
	Explain []string `json:"explain_files"`
}

func phase1PRDiagnostic(d *ast.Diagnostic) []any {
	file := ""
	if d.File() != nil {
		file = d.File().FileName()
	}
	args := []string{}
	args = append(args, d.MessageArgs()...)
	chain := []any{}
	for _, c := range d.MessageChain() {
		chain = append(chain, phase1PRDiagnostic(c))
	}
	related := []any{}
	for _, r := range d.RelatedInformation() {
		related = append(related, phase1PRDiagnostic(r))
	}
	return []any{file, d.Pos(), d.End(), d.Code(), int(d.Category()), string(d.MessageKey()), args, d.MessageText(), chain, related}
}

func phase1PRDiagnostics(list []*ast.Diagnostic) []any {
	result := []any{}
	for _, d := range list {
		result = append(result, phase1PRDiagnostic(d))
	}
	return result
}

func phase1PRStrings(list []string) []string {
	if list == nil {
		return []string{}
	}
	return list
}

func phase1PRConfigName(config *tsoptions.ParsedCommandLine) any {
	if config == nil {
		return nil
	}
	return config.ConfigName()
}

// The loader alone, as the S07 bridge runs it: the include processor's
// diagnostics are the loader's, before verification adds its own.
func phase1PRLoad(opts ProgramOptions, trace *[]string, config *tsoptions.ParsedCommandLine) map[string]any {
	loaded := processAllProgramFiles(opts, true)
	p := &Program{opts: opts, processedFiles: loaded}
	files := []any{}
	for _, file := range loaded.files {
		meta := loaded.sourceFileMetaDatas[file.Path()]
		files = append(files, []any{
			file.FileName(), string(file.Path()), loaded.libFiles[file.Path()] != nil,
			int(meta.ImpliedNodeFormat), meta.PackageJsonType, meta.PackageJsonDirectory,
			ast.IsExternalModule(file),
			p.GetSourceOfProjectReferenceIfOutputIncluded(file),
			phase1PRConfigName(p.GetRedirectForResolution(file)),
		})
	}
	type keyed struct {
		file, name string
		mode       core.ResolutionMode
		row        []any
	}
	less := func(rows []keyed) func(i, j int) bool {
		return func(i, j int) bool {
			a, b := rows[i], rows[j]
			if a.file != b.file {
				return a.file < b.file
			}
			if a.name != b.name {
				return a.name < b.name
			}
			return a.mode < b.mode
		}
	}
	modules := []keyed{}
	for path, resolutions := range loaded.resolvedModules {
		for key, r := range resolutions {
			modules = append(modules, keyed{string(path), key.Name, key.Mode, []any{
				string(path), key.Name, int(key.Mode), r.ResolvedFileName, r.OriginalPath, r.Extension,
				r.IsExternalLibraryImport, r.PackageId.String(),
			}})
		}
	}
	sort.Slice(modules, less(modules))
	types := []keyed{}
	for path, resolutions := range loaded.typeResolutionsInFile {
		for key, r := range resolutions {
			types = append(types, keyed{string(path), key.Name, key.Mode, []any{
				string(path), key.Name, int(key.Mode), r.ResolvedFileName, r.Primary, r.IsExternalLibraryImport,
			}})
		}
	}
	sort.Slice(types, less(types))
	rows := func(list []keyed) []any {
		result := []any{}
		for _, row := range list {
			result = append(result, row.row)
		}
		return result
	}
	return map[string]any{
		"root_files":          phase1PRStrings(config.FileNames()),
		"reference_paths":     phase1PRStrings(config.ResolvedProjectReferencePaths()),
		"config_errors":       phase1PRDiagnostics(config.GetConfigFileParsingDiagnostics()),
		"files":               files,
		"missing":             phase1PRStrings(loaded.missingFiles),
		"resolutions":         rows(modules),
		"type_resolutions":    rows(types),
		"include_diagnostics": phase1PRDiagnostics(loaded.includeProcessor.getDiagnostics(p).GetDiagnostics()),
		"trace":               phase1PRStrings(*trace),
	}
}

func phase1PRGraph(p *Program) map[string]any {
	walk := []any{}
	result := p.RangeResolvedProjectReference(func(path tspath.Path, config *tsoptions.ParsedCommandLine, parent *tsoptions.ParsedCommandLine, index int) bool {
		walk = append(walk, []any{string(path), phase1PRConfigName(config), parent.ConfigName(), index})
		return true
	})
	return map[string]any{"walk": walk, "result": result}
}

func phase1PRVerify(p *Program) map[string]any {
	blocked := []string{}
	for name := range p.hasEmitBlockingDiagnostics.Keys() {
		blocked = append(blocked, string(name))
	}
	sort.Strings(blocked)
	// The verifier's file-bound include requests (composite file lists, rootDir)
	// are in the include processor's per-file buckets, not the global ones.
	collection := p.includeProcessor.getDiagnostics(p)
	files := []any{}
	for _, file := range p.GetSourceFiles() {
		if !p.IsSourceFileDefaultLibrary(file.Path()) {
			files = append(files, []any{file.FileName(), phase1PRDiagnostics(collection.GetDiagnosticsForFile(file))})
		}
	}
	return map[string]any{
		"raw":     phase1PRDiagnostics(p.programDiagnostics),
		"blocked": blocked,
		"program": phase1PRDiagnostics(p.GetProgramDiagnostics()),
		"files":   files,
	}
}

func phase1PRExplain(p *Program, names []string) map[string]any {
	result := []any{}
	for _, name := range names {
		d := &processingDiagnostic{kind: processingDiagnosticKindExplainingFileInclude, data: &includeExplainingDiagnostic{
			file: p.toPath(name), message: diagnostics.File_0_not_found, args: []any{name},
		}}
		result = append(result, []any{name, phase1PRDiagnostic(d.toDiagnostic(p))})
	}
	return map[string]any{"explanations": result}
}

func phase1PRObserve(t *testing.T, request phase1PRRequest) map[string]any {
	ws := request.Workspace
	if ws == nil {
		t.Fatalf("%s: no workspace", request.Case)
	}
	files := map[string]any{}
	for name, text := range ws.Files {
		files[name] = text
	}
	for name, target := range ws.Symlinks {
		if _, exists := files[name]; exists {
			t.Fatalf("%s: duplicate symlink/file %s", request.Case, name)
		}
		files[name] = vfstest.Symlink(target)
	}
	fs := bundled.WrapFS(vfstest.FromMap(files, ws.CaseSensitive))
	host := func(trace *[]string) CompilerHost {
		return NewCompilerHost(ws.Cwd, fs, bundled.LibPath(), nil, func(msg *diagnostics.Message, args ...any) {
			if trace != nil {
				*trace = append(*trace, fmt.Sprintf("%d:%v", msg.Code(), args))
			}
		}, nil)
	}
	config, errs := tsoptions.GetParsedCommandLineOfConfigFile(ws.Project, &core.CompilerOptions{}, nil, host(nil), nil)
	if config == nil {
		t.Fatalf("%s: config %s was not read: %d errors", request.Case, ws.Project, len(errs))
	}
	if ws.SuppressOutputPathCheck {
		config.CompilerOptions().SuppressOutputPathCheck = core.TSTrue
	}
	opts := ProgramOptions{Host: host(nil), Config: config, SingleThreaded: core.TSTrue, UseSourceOfProjectReference: ws.UseSource}
	var program *Program
	built := func() *Program {
		if program == nil {
			program = NewProgram(opts)
		}
		return program
	}
	actions := []any{}
	for _, action := range request.Actions {
		var value map[string]any
		switch action.Op {
		case "load":
			trace := []string{}
			loadOpts := opts
			loadOpts.Host = host(&trace)
			value = phase1PRLoad(loadOpts, &trace, config)
		case "graph":
			value = phase1PRGraph(built())
		case "verify":
			value = phase1PRVerify(built())
		case "explain":
			value = phase1PRExplain(built(), request.Explain)
		default:
			t.Fatalf("%s: unknown action %q", request.Case, action.Op)
		}
		// Each ordered entry is one action's named fields; none nests an object.
		value["op"] = action.Op
		actions = append(actions, value)
	}
	return map[string]any{"ordered": actions}
}

func TestPhase1SyntaxProjectReferences(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []json.RawMessage `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}
	observations := make([]map[string]any, 0, len(document.Requests))
	for _, raw := range document.Requests {
		var request phase1PRRequest
		if err := json.Unmarshal(raw, &request); err != nil {
			t.Fatal(err)
		}
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Subject == "projectReferences" {
			row["result"] = "observed"
			row["observation"] = phase1PRObserve(t, request)
		} else {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the projectReferences probe"
		}
		observations = append(observations, row)
	}
	hash := sha256.Sum256(input)
	output := map[string]any{
		"request_sha256": hex.EncodeToString(hash[:]),
		"go":             runtime.Version(),
		"goos":           runtime.GOOS,
		"goarch":         runtime.GOARCH,
		"version":        1,
		"observations":   observations,
	}
	data, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), append(data, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}
