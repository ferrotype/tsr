package compiler

import (
	"encoding/hex"
	"encoding/json"
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/binder"
	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnostics"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
	"os"
	"testing"
)

type s07IncludeRequest struct {
	s07Request
	ConfigName string `json:"config_name"`
	ConfigText string `json:"config_text"`
	Specs      struct {
		Files, BeforeFiles, Includes, BeforeIncludes []string
		Default                                      bool
	} `json:"specs"`
	Queries []struct {
		Path             string
		Identity         string
		Preferred        *int
		AugmentationFrom string
	} `json:"queries"`
	SourceFileQueries []string `json:"source_file_queries"`
}
type s07IncludeDiag struct {
	File           string
	Pos, End       int
	Code           int32
	Args           []string
	Chain, Related []s07IncludeDiag
}

func s07IncludeDiagnostic(d *ast.Diagnostic) s07IncludeDiag {
	result := s07IncludeDiag{Pos: d.Pos(), End: d.End(), Code: d.Code(), Args: []string{}, Chain: []s07IncludeDiag{}, Related: []s07IncludeDiag{}}
	if d.File() != nil {
		result.File = hex.EncodeToString([]byte(d.File().FileName()))
	}
	for _, a := range d.MessageArgs() {
		result.Args = append(result.Args, hex.EncodeToString([]byte(a)))
	}
	for _, c := range d.MessageChain() {
		result.Chain = append(result.Chain, s07IncludeDiagnostic(c))
	}
	for _, c := range d.RelatedInformation() {
		result.Related = append(result.Related, s07IncludeDiagnostic(c))
	}
	return result
}
func TestS07IncludeReasons(t *testing.T) {
	raw, e := os.ReadFile(os.Getenv("S07_INCLUDE_REQUESTS"))
	if e != nil {
		t.Fatal(e)
	}
	var requests []s07IncludeRequest
	if e = json.Unmarshal(raw, &requests); e != nil {
		t.Fatal(e)
	}
	rows := []map[string]any{}
	for _, r := range requests {
		files := map[string]any{}
		for name, value := range r.Files {
			data, e := hex.DecodeString(value)
			if e != nil {
				t.Fatal(e)
			}
			files[name] = data
		}
		for name, target := range r.Symlinks {
			files[name] = vfstest.Symlink(target)
		}
		fs := bundled.WrapFS(vfstest.FromMap(files, r.CaseSensitive))
		host := NewCompilerHost(r.Cwd, fs, bundled.LibPath(), nil, nil, nil)
		compare := tspath.ComparePathsOptions{CurrentDirectory: r.Cwd, UseCaseSensitiveFileNames: r.CaseSensitive}
		config := tsoptions.NewParsedCommandLine(&r.Options, r.Roots, nil, compare)
		if r.ConfigName != "" {
			data, e := hex.DecodeString(r.ConfigText)
			if e != nil {
				t.Fatal(e)
			}
			config.ConfigFile = tsoptions.NewTsconfigSourceFileFromFilePath(r.ConfigName, tspath.ToPath(r.ConfigName, r.Cwd, r.CaseSensitive), string(data))
			s := r.Specs
			tsoptions.S07IncludeSpecs(config.ConfigFile, s.Files, s.BeforeFiles, s.Includes, s.BeforeIncludes, s.Default)
		}
		opts := ProgramOptions{Host: host, Config: config, SkipModuleResolution: r.SkipModuleResolution}
		loaded := processAllProgramFiles(opts, true)
		p := &Program{opts: opts, processedFiles: loaded, comparePathsOptions: compare}
		for _, file := range loaded.files {
			binder.BindSourceFile(file)
		}
		p.verifyCompilerOptions()
		observations := []map[string]any{}
		for _, q := range r.Queries {
			path := tspath.Path(q.Path)
			original := p.includeProcessor.fileIncludeReasons[path]
			reasons := original
			if q.AugmentationFrom != "" {
				source := p.GetSourceFileByPath(tspath.Path(q.AugmentationFrom))
				reasons = append(append([]*FileIncludeReason{}, reasons...), &FileIncludeReason{kind: fileIncludeKindImport, data: &referencedFileData{file: source.Path(), index: len(source.Imports())}})
				p.includeProcessor.fileIncludeReasons[path] = reasons
			}
			if len(reasons) == 0 && q.Path != "" && q.Path != "/missing.ts" {
				t.Fatalf("%s: missing expected reason at %q", r.ID, q.Path)
			}
			if q.Identity != "" {
				if len(reasons) == 0 {
					t.Fatalf("%s has no reason to duplicate", r.ID)
				}
				reasons = append([]*FileIncludeReason{}, reasons...)
				reason := reasons[0]
				if q.Identity == "distinct" {
					reason = &FileIncludeReason{kind: reason.kind, data: reason.data}
				}
				reasons = append(reasons, reason)
				p.includeProcessor.fileIncludeReasons[path] = reasons
			}
			var preferred *FileIncludeReason
			if q.Preferred != nil {
				preferred = reasons[*q.Preferred]
			}
			d := &processingDiagnostic{kind: processingDiagnosticKindExplainingFileInclude, data: &includeExplainingDiagnostic{file: path, diagnosticReason: preferred, message: diagnostics.File_0_is_not_under_rootDir_1_rootDir_is_expected_to_contain_all_source_files, args: []any{q.Path, "/root"}}}
			diagnostic := s07IncludeDiagnostic(d.toDiagnostic(p))
			entries := []map[string]any{}
			for _, reason := range reasons {
				absolute := reason.toDiagnostic(p, false)
				relative := reason.toDiagnostic(p, true)
				related := p.includeProcessor.getRelatedInfo(reason, p)
				var relatedValue any
				if related != nil {
					relatedValue = s07IncludeDiagnostic(related)
				}
				entries = append(entries, map[string]any{"absolute": s07IncludeDiagnostic(absolute), "relative": s07IncludeDiagnostic(relative), "related": relatedValue, "stable": absolute == reason.toDiagnostic(p, false) && relative == reason.toDiagnostic(p, true) && related == p.includeProcessor.getRelatedInfo(reason, p)})
			}
			observations = append(observations, map[string]any{"path": q.Path, "diagnostic": diagnostic, "reasons": entries})
			p.includeProcessor.fileIncludeReasons[path] = original
		}
		globals := []s07IncludeDiag{}
		for _, d := range p.GetProgramDiagnostics() {
			globals = append(globals, s07IncludeDiagnostic(d))
		}
		includeGlobals := []s07IncludeDiag{}
		for _, d := range p.includeProcessor.getDiagnostics(p).GetGlobalDiagnostics() {
			includeGlobals = append(includeGlobals, s07IncludeDiagnostic(d))
		}
		byFile := map[string][]s07IncludeDiag{}
		for _, file := range p.files {
			values := []s07IncludeDiag{}
			for _, d := range p.includeProcessor.getDiagnostics(p).GetDiagnosticsForFile(file) {
				values = append(values, s07IncludeDiagnostic(d))
			}
			if len(values) > 0 {
				byFile[string(file.Path())] = values
			}
		}
		row := map[string]any{"id": r.ID, "queries": observations, "program": globals, "include_globals": includeGlobals, "include_files": byFile}
		if r.SourceFileQueries != nil {
			// Program accessors on the loaded program: its configuration's parsing
			// diagnostics, source files looked up by name, and default-library
			// membership of every file it lists.
			parsing := []s07IncludeDiag{}
			for _, d := range p.GetConfigFileParsingDiagnostics() {
				parsing = append(parsing, s07IncludeDiagnostic(d))
			}
			sourceFiles := []map[string]any{}
			for _, name := range r.SourceFileQueries {
				found := ""
				if file := p.GetSourceFile(name); file != nil {
					found = file.FileName()
				}
				sourceFiles = append(sourceFiles, map[string]any{"query": name, "file": found})
			}
			files := []map[string]any{}
			for _, file := range p.Program().SourceFiles() {
				files = append(files, map[string]any{"path": string(file.Path()), "default_library": p.IsSourceFileDefaultLibrary(file.Path())})
			}
			row["program_accessors"] = map[string]any{"config_file_parsing_diagnostics": parsing, "source_files": sourceFiles, "files": files}
		}
		rows = append(rows, row)
	}
	data, e := json.MarshalIndent(rows, "", "  ")
	if e != nil {
		t.Fatal(e)
	}
	data = append(data, '\n')
	if e = os.WriteFile(os.Getenv("S07_INCLUDE_OUTPUT"), data, 0600); e != nil {
		t.Fatal(e)
	}
	s07MergeObservations(t)
}

func s07MergeObservations(t *testing.T) {
	raw, e := os.ReadFile(os.Getenv("S07_INCLUDE_MERGE_REQUESTS"))
	if e != nil {
		t.Fatal(e)
	}
	var requests []struct {
		ID          string `json:"id"`
		Diagnostics []struct {
			Chain   int
			Related []int
		} `json:"diagnostics"`
	}
	if e = json.Unmarshal(raw, &requests); e != nil {
		t.Fatal(e)
	}
	rows := []map[string]any{}
	for _, r := range requests {
		input := []*ast.Diagnostic{}
		for _, recipe := range r.Diagnostics {
			d := ast.NewCompilerDiagnostic(diagnostics.File_0_is_not_under_rootDir_1_rootDir_is_expected_to_contain_all_source_files, "same", "root")
			if recipe.Chain != 0 {
				d.SetMessageChain([]*ast.Diagnostic{ast.NewExternalDiagnostic(nil, core.NewTextRange(-1, -1), "", diagnostics.CategoryError, int32(9000+recipe.Chain), "x")})
			}
			related := []*ast.Diagnostic{}
			for _, code := range recipe.Related {
				related = append(related, ast.NewExternalDiagnostic(nil, core.NewTextRange(-1, -1), "", diagnostics.CategoryError, int32(8000+code), "x"))
			}
			if len(related) > 0 {
				d.SetRelatedInfo(related)
			}
			input = append(input, d)
		}
		values := []s07IncludeDiag{}
		for _, d := range SortAndDeduplicateDiagnostics(input) {
			values = append(values, s07IncludeDiagnostic(d))
		}
		rows = append(rows, map[string]any{"id": r.ID, "diagnostics": values})
	}
	data, e := json.MarshalIndent(rows, "", "  ")
	if e != nil {
		t.Fatal(e)
	}
	data = append(data, '\n')
	if e = os.WriteFile(os.Getenv("S07_INCLUDE_MERGE_OUTPUT"), data, 0600); e != nil {
		t.Fatal(e)
	}
}
