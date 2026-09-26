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

func TestC2InferenceMapper(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("C2_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var source struct {
		Source string `json:"source"`
	}
	if err = json.Unmarshal(raw, &source); err != nil {
		t.Fatal(err)
	}
	fs := bundled.WrapFS(vfstest.FromMap(map[string]string{"/main.ts": source.Source}, false))
	host := compiler.NewCompilerHost("/", fs, bundled.LibPath(), nil, nil, nil)
	options := &core.CompilerOptions{Target: core.ScriptTargetESNext, Strict: core.TSTrue, SkipLibCheck: core.TSTrue, NoLib: core.TSTrue}
	config := tsoptions.NewParsedCommandLine(options, []string{"/main.ts"}, nil, tspath.ComparePathsOptions{})
	program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host})
	c, _ := checker.NewChecker(program, nil)
	result := checker.C2InferenceMapperProbe(c, program.GetSourceFile("/main.ts").AsNode())
	hash := sha256.Sum256(raw)
	out, err := json.Marshal(map[string]any{"version": 1, "go": runtime.Version(), "request_sha256": hex.EncodeToString(hash[:]), "observation": result})
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("C2_OUTPUT"), out, 0600); err != nil {
		t.Fatal(err)
	}
}
