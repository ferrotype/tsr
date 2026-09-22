package testrunner

// Phase 1 F4a native syntax schedule. Compiled into the pinned testrunner
// package through a go test overlay, so it reads the real skippedTests list and
// executes the real harness option guard. Per request it builds the program
// the way the S07 loader bridge does, through the pinned compiler.NewProgram,
// and calls only Program.GetSyntacticDiagnostics: no binder, checker,
// declaration or emit phase runs, so nothing semantic can reach the result.
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
	"slices"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnosticwriter"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/harnessutil"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

type phase1SyntaxRequest struct {
	ID string `json:"id"`
	// Path is the physical source; skippedTests is keyed by its basename.
	Path string `json:"path"`
	// Guard is false only when the harness fails in option setup, before the
	// runner reaches SkipUnsupportedCompilerOptions.
	Guard bool `json:"guard"`
	// Load is false for a named boundary: rejected options, or content mappers
	// this request cannot reproduce.
	Load    bool `json:"load"`
	Request struct {
		ID                   string               `json:"id"`
		Cwd                  string               `json:"cwd"`
		CaseSensitive        bool                 `json:"case_sensitive"`
		Files                map[string]string    `json:"files"`
		Symlinks             map[string]string    `json:"symlinks"`
		Roots                []string             `json:"roots"`
		Options              core.CompilerOptions `json:"options"`
		SkipModuleResolution bool                 `json:"skip_module_resolution"`
	} `json:"request"`
}

type phase1SyntaxDiagnostic struct {
	File     string
	Pos      int
	End      int
	Code     int32
	Category int
	Key      string
	Args     []string
	Text     string
	Chain    []phase1SyntaxDiagnostic
	Related  []phase1SyntaxDiagnostic
}

func phase1SyntaxDiag(d *ast.Diagnostic) phase1SyntaxDiagnostic {
	r := phase1SyntaxDiagnostic{Pos: d.Pos(), End: d.End(), Code: d.Code(), Category: int(d.Category()), Key: string(d.MessageKey()), Args: d.MessageArgs(), Text: d.MessageText()}
	if d.File() != nil {
		r.File = d.File().FileName()
	}
	for _, c := range d.MessageChain() {
		r.Chain = append(r.Chain, phase1SyntaxDiag(c))
	}
	for _, c := range d.RelatedInformation() {
		r.Related = append(r.Related, phase1SyntaxDiag(c))
	}
	return r
}

type phase1ParseKey struct {
	options    ast.SourceFileParseOptions
	text       string
	scriptKind core.ScriptKind
}

// phase1MemoHost memoizes bundled library parses across requests, keyed the
// way harnessutil's cachedCompilerHost keys its cache. The key holds every
// parse input, so reuse cannot change a result; only library files are kept so
// memory stays bounded by the library set rather than the corpus.
type phase1MemoHost struct {
	compiler.CompilerHost
	memo map[phase1ParseKey]*ast.SourceFile
}

func (h *phase1MemoHost) GetSourceFile(options ast.SourceFileParseOptions) *ast.SourceFile {
	if !strings.HasPrefix(options.FileName, bundled.LibPath()+"/") {
		return h.CompilerHost.GetSourceFile(options)
	}
	text, ok := h.FS().ReadFile(options.FileName)
	if !ok {
		return nil
	}
	key := phase1ParseKey{options, text, core.EnsureScriptKindFromFileName(options.FileName)}
	if file, ok := h.memo[key]; ok {
		return file
	}
	file := parser.ParseSourceFile(options, text, key.scriptKind)
	h.memo[key] = file
	return file
}

func TestPhase1SyntaxSchedule(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("PHASE1_SYNTAX_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var requests []phase1SyntaxRequest
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&requests); err != nil {
		t.Fatal(err)
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		t.Fatal("trailing request data")
	}
	memo := map[phase1ParseKey]*ast.SourceFile{}
	seen := map[string]bool{}
	rows := make([]map[string]any, 0, len(requests))
	ctx := context.Background()
	for _, req := range requests {
		if req.ID == "" || req.ID != req.Request.ID || seen[req.ID] {
			t.Fatalf("invalid or duplicate request %q", req.ID)
		}
		seen[req.ID] = true
		row := map[string]any{"id": req.ID, "filename_skip": slices.Contains(skippedTests, filepath.Base(req.Path))}
		if req.Guard {
			row["option_guard"] = "not_executed"
			guarded := req.Request.Options
			t.Run(req.ID, func(t *testing.T) {
				defer func() {
					if t.Failed() {
						row["option_guard"] = "failed"
					} else if t.Skipped() {
						row["option_guard"] = "skipped"
					}
				}()
				// The original guard, including its fail-before-skip order. A
				// fatal outcome fails this command; it is not a new exclusion.
				harnessutil.SkipUnsupportedCompilerOptions(t, &guarded)
				row["option_guard"] = "allowed"
			})
		} else {
			row["option_guard"] = "not_reached"
		}
		if !req.Load {
			row["load"] = "not_loaded"
			rows = append(rows, row)
			continue
		}
		func() {
			defer func() {
				if value := recover(); value != nil {
					row["load"] = "panic"
					row["panic"] = fmt.Sprint(value)
				}
			}()
			files := map[string]any{}
			for name, textHex := range req.Request.Files {
				data, err := hex.DecodeString(textHex)
				if err != nil {
					t.Fatalf("%s: %s: %v", req.ID, name, err)
				}
				files[name] = data
			}
			for name, target := range req.Request.Symlinks {
				if _, exists := files[name]; exists {
					t.Fatalf("%s: duplicate symlink/file %s", req.ID, name)
				}
				files[name] = vfstest.Symlink(target)
			}
			fs := bundled.WrapFS(vfstest.FromMap(files, req.Request.CaseSensitive))
			host := &phase1MemoHost{compiler.NewCompilerHost(req.Request.Cwd, fs, bundled.LibPath(), nil, nil, nil), memo}
			options := req.Request.Options
			compare := tspath.ComparePathsOptions{CurrentDirectory: req.Request.Cwd, UseCaseSensitiveFileNames: req.Request.CaseSensitive}
			config := tsoptions.NewParsedCommandLine(&options, req.Request.Roots, nil, compare)
			program := compiler.NewProgram(compiler.ProgramOptions{Host: host, Config: config, SkipModuleResolution: req.Request.SkipModuleResolution, SingleThreaded: core.TSTrue})
			row["load"] = "loaded"
			names := []string{}
			for _, file := range program.GetSourceFiles() {
				names = append(names, file.FileName())
			}
			hash := sha256.Sum256([]byte(strings.Join(names, "\n")))
			row["files"] = len(names)
			row["file_names_sha256"] = hex.EncodeToString(hash[:])
			diagnostics := program.GetSyntacticDiagnostics(ctx, nil)
			structured := []phase1SyntaxDiagnostic{}
			for _, d := range diagnostics {
				structured = append(structured, phase1SyntaxDiag(d))
			}
			row["syntactic"] = structured
			wrapped := diagnosticwriter.ToDiagnostics(diagnosticwriter.WrapASTDiagnostics(diagnostics))
			format := &diagnosticwriter.FormattingOptions{NewLine: "\r\n", ComparePathsOptions: compare}
			var plain, pretty strings.Builder
			diagnosticwriter.WriteFormatDiagnostics(&plain, wrapped, format)
			diagnosticwriter.FormatDiagnosticsWithColorAndContext(&pretty, wrapped, format)
			row["plain_hex"] = hex.EncodeToString([]byte(plain.String()))
			row["pretty_hex"] = hex.EncodeToString([]byte(pretty.String()))
		}()
		rows = append(rows, row)
	}
	hash := sha256.Sum256(raw)
	result, err := json.Marshal(map[string]any{"request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "rows": rows})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("PHASE1_SYNTAX_OUTPUT"), result, 0600); err != nil {
		t.Fatal(err)
	}
}
