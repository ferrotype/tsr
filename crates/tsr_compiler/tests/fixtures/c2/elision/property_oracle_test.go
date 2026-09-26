package checker_test

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
	"os"
	"runtime"
	"testing"
)

func TestC2PropertyElision(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("C2_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var request struct {
		Cases []struct {
			ID           string `json:"id"`
			Source       string `json:"source"`
			Length       int    `json:"length"`
			NoTruncation bool   `json:"no_truncation"`
		} `json:"cases"`
	}
	if err = json.Unmarshal(raw, &request); err != nil {
		t.Fatal(err)
	}
	cases := []any{}
	for _, q := range request.Cases {
		fs := bundled.WrapFS(vfstest.FromMap(map[string]string{"/main.ts": q.Source}, true))
		host := compiler.NewCompilerHost("/", fs, bundled.LibPath(), nil, nil, nil)
		config := tsoptions.NewParsedCommandLine(&core.CompilerOptions{}, []string{"/main.ts"}, nil, tspath.ComparePathsOptions{})
		program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host})
		program.BindSourceFiles()
		c, _ := checker.NewChecker(program, nil)
		node := program.GetSourceFile("/main.ts").Statements.Nodes[0].Name()
		cases = append(cases, map[string]any{"id": q.ID, "observation": checker.C2PropertyElision(c, node, q.Length, q.NoTruncation)})
	}
	h := sha256.Sum256(raw)
	output := map[string]any{"version": 1, "request_sha256": hex.EncodeToString(h[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "cases": cases}
	encoded, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("C2_OUTPUT"), encoded, 0600); err != nil {
		t.Fatal(err)
	}
}
