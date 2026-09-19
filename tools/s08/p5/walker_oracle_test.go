package tsbaseline

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/harnessutil"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

func TestS08P5Walker(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var request struct {
		Version int    `json:"version"`
		Scope   string `json:"scope"`
		Cases   []struct {
			ID    string `json:"id"`
			Files []struct {
				Name    string `json:"name"`
				Content string `json:"content"`
			} `json:"files"`
			Roots        []string `json:"roots"`
			Header       string   `json:"header"`
			HadErrors    bool     `json:"had_errors"`
			Enabled      bool     `json:"enabled"`
			AllowJS      bool     `json:"allow_js"`
			NoCheck      bool     `json:"no_check"`
			SkipLibCheck bool     `json:"skip_lib_check"`
		} `json:"cases"`
	}
	if err := json.Unmarshal(raw, &request); err != nil {
		t.Fatal(err)
	}
	if request.Version != 1 || request.Scope != "native-walker-focused" {
		t.Fatal("unknown walker contract")
	}
	cases := []any{}
	for _, r := range request.Cases {
		S08Queries = []S08Query{}
		row := map[string]any{"id": r.ID, "state": "executed"}
		func() {
			defer func() {
				if failure := recover(); failure != nil {
					row["state"] = "failed"
					row["panic"] = failure
				}
			}()
			if !r.Enabled {
				row["types"] = S08Baseline{State: "disabled"}
				row["symbols"] = S08Baseline{State: "disabled"}
				return
			}
			files := map[string]string{}
			inputs := []*harnessutil.TestFile{}
			for _, file := range r.Files {
				files[file.Name] = file.Content
				inputs = append(inputs, &harnessutil.TestFile{UnitName: file.Name, Content: file.Content})
			}
			host := compiler.NewCompilerHost("/", vfstest.FromMap(files, true), "/no-default-lib", nil, nil, nil)
			opts := &core.CompilerOptions{Target: core.ScriptTargetESNext, Module: core.ModuleKindESNext, Strict: core.TSTrue, NoLib: core.TSTrue, AllowJs: core.IfElse(r.AllowJS, core.TSTrue, core.TSFalse)}
			opts.NoCheck = core.IfElse(r.NoCheck, core.TSTrue, core.TSFalse)
			opts.SkipLibCheck = core.IfElse(r.SkipLibCheck, core.TSTrue, core.TSFalse)
			config := tsoptions.NewParsedCommandLine(opts, r.Roots, nil, tspath.ComparePathsOptions{})
			program := compiler.NewProgram(compiler.ProgramOptions{Config: config, Host: host, SingleThreaded: core.TSTrue})
			program.BindSourceFiles()
			program.GetSemanticDiagnostics(context.Background(), nil)
			program.GetGlobalDiagnostics(context.Background())
			row["types"], row["symbols"] = S08TypeSymbolBaselines(program, inputs, r.Header, r.HadErrors)
		}()
		row["queries"] = S08Queries
		cases = append(cases, row)
	}
	hash := sha256.Sum256(raw)
	output := map[string]any{"version": 1, "request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "cases": cases}
	encoded, err := json.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), encoded, 0600); err != nil {
		t.Fatal(err)
	}
}
