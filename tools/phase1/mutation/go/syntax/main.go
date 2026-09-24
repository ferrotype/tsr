// Phase 1 mutation witnesses: the syntax oracle's native driver.
//
// scripts/phase1_mutation_go.py copies this file into internal/phase1syntax of
// a git-archive export of the pin (upstream/ is never edited) and builds it as
// a main package. The per-request load path -- the vfs, the memoizing host,
// the options, compiler.NewProgram(SingleThreaded), GetSyntacticDiagnostics and
// the two diagnosticwriter renderings -- is copied verbatim from
// tools/phase1/syntax/program_probe_test.go, which froze
// data/phase1/syntax-native.json; the testrunner-only selection fields
// (skippedTests, SkipUnsupportedCompilerOptions) are omitted because they feed
// nothing the program load observes. Every native freeze checks that this
// driver reproduces the committed native values on every row.
//
// Usage: phase1syntax <requests.ndjson> <rows.ndjson>. Each request line is a
// materialized syntax row (scripts/phase1_mutation_go.py requests), each output
// line is {"row","outcome","values"?,"message"?,"micros"}. With
// PHASE1_SYNTAX_MEMO=0 the bundled-library memo is dropped before every
// request, so each row parses its own library files, as a fresh process would.
//
// Coverage segments (inert in the plain build, see phase1_cover_off.go):
// "load" (production: host, options and NewProgram), "files" (observation:
// the file-name list), "syntactic" (production: GetSyntacticDiagnostics) and
// "render" (observation: the structured and rendered diagnostics).
package main

import (
	"bufio"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"strings"
	"time"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnosticwriter"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

type phase1SyntaxRequest struct {
	ID            string `json:"id"`
	RequestSHA256 string `json:"request_sha256"`
	Path          string `json:"path"`
	Guard         bool   `json:"guard"`
	Load          bool   `json:"load"`
	Request       struct {
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

func fatal(format string, args ...any) {
	fmt.Fprintf(os.Stderr, "phase1syntax: "+format+"\n", args...)
	os.Exit(2)
}

// observe loads and observes one request. A panic of the pinned code is the
// row's outcome, as in the probe; malformed input is fatal.
func observe(req *phase1SyntaxRequest, memo map[phase1ParseKey]*ast.SourceFile, ctx context.Context) (row map[string]any) {
	row = map[string]any{"row": req.ID}
	files := map[string]any{}
	for name, textHex := range req.Request.Files {
		data, err := hex.DecodeString(textHex)
		if err != nil {
			fatal("%s: %s: %v", req.ID, name, err)
		}
		files[name] = data
	}
	for name, target := range req.Request.Symlinks {
		if _, exists := files[name]; exists {
			fatal("%s: duplicate symlink/file %s", req.ID, name)
		}
		files[name] = vfstest.Symlink(target)
	}
	started := time.Now()
	defer func() {
		if value := recover(); value != nil {
			phase1CoverEnd(req.ID)
			delete(row, "values")
			row["outcome"] = "panic"
			row["message"] = fmt.Sprint(value)
		}
		row["micros"] = time.Since(started).Microseconds()
	}()
	phase1CoverBegin("load")
	fs := bundled.WrapFS(vfstest.FromMap(files, req.Request.CaseSensitive))
	host := &phase1MemoHost{compiler.NewCompilerHost(req.Request.Cwd, fs, bundled.LibPath(), nil, nil, nil), memo}
	options := req.Request.Options
	compare := tspath.ComparePathsOptions{CurrentDirectory: req.Request.Cwd, UseCaseSensitiveFileNames: req.Request.CaseSensitive}
	config := tsoptions.NewParsedCommandLine(&options, req.Request.Roots, nil, compare)
	program := compiler.NewProgram(compiler.ProgramOptions{Host: host, Config: config, SkipModuleResolution: req.Request.SkipModuleResolution, SingleThreaded: core.TSTrue})
	phase1CoverEnd(req.ID)
	phase1CoverBegin("files")
	names := []string{}
	for _, file := range program.GetSourceFiles() {
		names = append(names, file.FileName())
	}
	hash := sha256.Sum256([]byte(strings.Join(names, "\n")))
	phase1CoverEnd(req.ID)
	phase1CoverBegin("syntactic")
	diagnostics := program.GetSyntacticDiagnostics(ctx, nil)
	phase1CoverEnd(req.ID)
	phase1CoverBegin("render")
	structured := []phase1SyntaxDiagnostic{}
	for _, d := range diagnostics {
		structured = append(structured, phase1SyntaxDiag(d))
	}
	wrapped := diagnosticwriter.ToDiagnostics(diagnosticwriter.WrapASTDiagnostics(diagnostics))
	format := &diagnosticwriter.FormattingOptions{NewLine: "\r\n", ComparePathsOptions: compare}
	var plain, pretty strings.Builder
	diagnosticwriter.WriteFormatDiagnostics(&plain, wrapped, format)
	diagnosticwriter.FormatDiagnosticsWithColorAndContext(&pretty, wrapped, format)
	phase1CoverEnd(req.ID)
	row["outcome"] = "ok"
	row["values"] = map[string]any{
		"files":             len(names),
		"file_names_sha256": hex.EncodeToString(hash[:]),
		"syntactic":         structured,
		"plain_hex":         hex.EncodeToString([]byte(plain.String())),
		"pretty_hex":        hex.EncodeToString([]byte(pretty.String())),
	}
	return row
}

func main() {
	if len(os.Args) != 3 {
		fatal("usage: phase1syntax <requests.ndjson> <rows.ndjson>")
	}
	input, err := os.Open(os.Args[1])
	if err != nil {
		fatal("%v", err)
	}
	defer input.Close()
	output, err := os.Create(os.Args[2])
	if err != nil {
		fatal("%v", err)
	}
	memoize := os.Getenv("PHASE1_SYNTAX_MEMO") != "0"
	reader := bufio.NewReaderSize(input, 1<<20)
	writer := bufio.NewWriter(output)
	memo := map[phase1ParseKey]*ast.SourceFile{}
	seen := map[string]bool{}
	ctx := context.Background()
	for {
		line, err := reader.ReadBytes('\n')
		if err != nil && !errors.Is(err, io.EOF) {
			fatal("%v", err)
		}
		last := err != nil
		if len(bytes.TrimSpace(line)) == 0 {
			if last {
				break
			}
			fatal("blank request line")
		}
		var req phase1SyntaxRequest
		decoder := json.NewDecoder(bytes.NewReader(line))
		decoder.DisallowUnknownFields()
		if err := decoder.Decode(&req); err != nil {
			fatal("request: %v", err)
		}
		if req.ID == "" || req.ID != req.Request.ID || seen[req.ID] {
			fatal("invalid or duplicate request %q", req.ID)
		}
		if !req.Load || !req.Guard {
			fatal("%s: the syntax oracle observes loaded rows only", req.ID)
		}
		seen[req.ID] = true
		if !memoize {
			memo = map[phase1ParseKey]*ast.SourceFile{}
		}
		encoded, err := json.Marshal(observe(&req, memo, ctx))
		if err != nil {
			fatal("%s: %v", req.ID, err)
		}
		writer.Write(encoded)
		writer.WriteByte('\n')
		// One flushed line per row: a crash loses no finished row.
		if err := writer.Flush(); err != nil {
			fatal("%v", err)
		}
		if last {
			break
		}
	}
	if err := output.Close(); err != nil {
		fatal("%v", err)
	}
}
