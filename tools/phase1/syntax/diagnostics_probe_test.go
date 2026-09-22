package compiler

// Phase 1 F4a, `syntacticDiagnostics` group (plan task 4): access only. Each
// request is a small in-memory program. The probe builds it with the pinned
// NewProgram, calls only Program.GetSyntacticDiagnostics -- for the whole
// program, or for the one file the request names -- and renders the result
// with the production diagnosticwriter. It never runs the binder, the checker,
// declaration diagnostics or emit, and it carries no expected value.
//
// It is an IN-PACKAGE test file so a later probe may reach unexported program
// state; this one needs only exported entry points. Every name is prefixed
// `phase1Syntax` so it cannot collide with a pinned test helper.
//
// Diagnostics travel as positional arrays, [file, pos, end, code, category,
// key, args, text, chain, related], because the comparison canonicalises with
// sorted keys and the diagnostic ORDER is the subject: a multi-key object
// nested in the ordered payload would lose nothing here, but the rule that it
// may not is what keeps every order-sensitive family honest.

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnosticwriter"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

type phase1SyntaxProgram struct {
	Cwd           string               `json:"cwd"`
	CaseSensitive bool                 `json:"case_sensitive"`
	Files         map[string]string    `json:"files"`
	Roots         []string             `json:"roots"`
	Options       core.CompilerOptions `json:"options"`
}

type phase1SyntaxRequest struct {
	Case      string               `json:"case"`
	Operation string               `json:"operation"`
	Subject   string               `json:"subject"`
	Program   *phase1SyntaxProgram `json:"program"`
	Scope     *string              `json:"scope"`
}

func phase1SyntaxDiagnostic(d *ast.Diagnostic) []any {
	file := ""
	if d.File() != nil {
		file = d.File().FileName()
	}
	args := []string{}
	args = append(args, d.MessageArgs()...)
	chain := []any{}
	for _, c := range d.MessageChain() {
		chain = append(chain, phase1SyntaxDiagnostic(c))
	}
	related := []any{}
	for _, r := range d.RelatedInformation() {
		related = append(related, phase1SyntaxDiagnostic(r))
	}
	return []any{file, d.Pos(), d.End(), d.Code(), int(d.Category()), string(d.MessageKey()), args, d.MessageText(), chain, related}
}

func phase1SyntaxObserve(t *testing.T, request phase1SyntaxRequest) map[string]any {
	if request.Program == nil {
		t.Fatalf("%s: no program", request.Case)
	}
	spec := request.Program
	files := map[string]any{}
	for name, text := range spec.Files {
		files[name] = text
	}
	fs := bundled.WrapFS(vfstest.FromMap(files, spec.CaseSensitive))
	host := NewCompilerHost(spec.Cwd, fs, bundled.LibPath(), nil, nil, nil)
	options := spec.Options
	compare := tspath.ComparePathsOptions{CurrentDirectory: spec.Cwd, UseCaseSensitiveFileNames: spec.CaseSensitive}
	config := tsoptions.NewParsedCommandLine(&options, spec.Roots, nil, compare)
	program := NewProgram(ProgramOptions{Host: host, Config: config, SingleThreaded: core.TSTrue})
	var target *ast.SourceFile
	if request.Scope != nil {
		target = program.GetSourceFile(*request.Scope)
		if target == nil {
			t.Fatalf("%s: scope %s is not a program file", request.Case, *request.Scope)
		}
	}
	diagnostics := program.GetSyntacticDiagnostics(context.Background(), target)
	ordered := []any{}
	for _, d := range diagnostics {
		ordered = append(ordered, phase1SyntaxDiagnostic(d))
	}
	names := []string{}
	for _, file := range program.GetSourceFiles() {
		names = append(names, file.FileName())
	}
	wrapped := diagnosticwriter.ToDiagnostics(diagnosticwriter.WrapASTDiagnostics(diagnostics))
	format := &diagnosticwriter.FormattingOptions{NewLine: "\r\n", ComparePathsOptions: compare}
	var plain, pretty strings.Builder
	diagnosticwriter.WriteFormatDiagnostics(&plain, wrapped, format)
	diagnosticwriter.FormatDiagnosticsWithColorAndContext(&pretty, wrapped, format)
	return map[string]any{
		"ordered":    ordered,
		"files":      names,
		"plain_hex":  hex.EncodeToString([]byte(plain.String())),
		"pretty_hex": hex.EncodeToString([]byte(pretty.String())),
	}
}

func TestPhase1SyntaxDiagnostics(t *testing.T) {
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
		var request phase1SyntaxRequest
		if err := json.Unmarshal(raw, &request); err != nil {
			t.Fatal(err)
		}
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Subject == "syntacticDiagnostics" {
			row["result"] = "observed"
			row["observation"] = phase1SyntaxObserve(t, request)
		} else {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the syntacticDiagnostics probe"
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
