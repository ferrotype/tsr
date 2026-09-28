package checker_test

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

// TestC4State records, for each C4 case, the JSX entities at the root file's
// first JSX tag and the referenced state of its import specifiers: once after
// the checks the Rust loader runs (syntactic diagnostics of every file; then
// the global diagnostics; then each non-declaration file's diagnostics and the
// global diagnostics again), and once on a fresh checker that has checked
// nothing. The program comes from the case's recorded command line over an
// in-memory file system rooted at "/".
func TestC4State(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("C4_STATE_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var requests []struct {
		ID    string            `json:"id"`
		Root  string            `json:"root"`
		Args  []string          `json:"args"`
		Files map[string]string `json:"files"`
	}
	if err = json.Unmarshal(raw, &requests); err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	rows := []any{}
	for _, r := range requests {
		fs := bundled.WrapFS(vfstest.FromMap(r.Files, true))
		host := compiler.NewCompilerHost("/", fs, bundled.LibPath(), nil, nil, nil)
		config := tsoptions.ParseCommandLine(r.Args, host)
		if len(config.Errors) != 0 {
			t.Fatalf("%s: command line errors %v", r.ID, config.Errors)
		}
		program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host})
		program.BindSourceFiles()
		root := program.GetSourceFile("/" + r.Root)
		if root == nil {
			t.Fatalf("%s: no root file", r.ID)
		}
		checked, _ := checker.NewChecker(program, nil)
		syntactic := false
		for _, file := range program.GetSourceFiles() {
			if len(program.GetSyntacticDiagnostics(ctx, file)) != 0 {
				syntactic = true
			}
		}
		if !syntactic && len(checked.GetGlobalDiagnostics()) == 0 {
			for _, file := range program.GetSourceFiles() {
				if !file.IsDeclarationFile {
					checked.GetDiagnostics(ctx, file)
				}
			}
			checked.GetGlobalDiagnostics()
		}
		fresh, _ := checker.NewChecker(program, nil)
		row := map[string]any{"id": r.ID, "checked": state(checked, root, true), "unchecked": state(fresh, root, false)}
		rows = append(rows, row)
	}
	hash := sha256.Sum256(raw)
	out, err := json.MarshalIndent(map[string]any{"version": 1, "request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "rows": rows}, "", " ")
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("C4_STATE_OUTPUT"), append(out, '\n'), 0600); err != nil {
		t.Fatal(err)
	}
}

func state(c *checker.Checker, root *ast.SourceFile, aliases bool) map[string]any {
	out := map[string]any{"jsx": nil}
	var tag *ast.Node
	specifiers := map[string]any{}
	var visit func(node *ast.Node) bool
	visit = func(node *ast.Node) bool {
		if tag == nil && (ast.IsJsxOpeningElement(node) || ast.IsJsxSelfClosingElement(node) || ast.IsJsxOpeningFragment(node)) {
			tag = node
		}
		if aliases && ast.IsImportSpecifier(node) {
			specifiers[node.Name().Text()] = c.C4AliasReferenced(node)
		}
		return node.ForEachChild(visit)
	}
	root.AsNode().ForEachChild(visit)
	if tag != nil {
		out["jsx"] = c.C4JsxLinkState(tag)
	}
	if aliases {
		out["aliases"] = specifiers
	}
	return out
}
