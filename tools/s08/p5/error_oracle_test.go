package tsbaseline

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnostics"
	"github.com/microsoft/TypeScript/tsc/internal/diagnosticwriter"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/testutil/harnessutil"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"os"
	"runtime"
	"strings"
	"testing"
)

type S08ErrorSpec struct {
	File      *int                 `json:"file"`
	Pos       int                  `json:"pos"`
	End       int                  `json:"end"`
	Code      int32                `json:"code"`
	Category  diagnostics.Category `json:"category"`
	TextHex   *string              `json:"text_hex"`
	Key       string               `json:"key"`
	ArgsHex   []string             `json:"args_hex"`
	SourceHex string               `json:"source_hex"`
	Chain     []S08ErrorSpec       `json:"chain"`
	Related   []S08ErrorSpec       `json:"related"`
}

func TestS08P5Errors(t *testing.T) {
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
				NameHex    string `json:"name_hex"`
				ContentHex string `json:"content_hex"`
			} `json:"files"`
			Inputs      []int          `json:"inputs"`
			Diagnostics []S08ErrorSpec `json:"diagnostics"`
			Cwd         string         `json:"cwd"`
		} `json:"cases"`
	}
	if err = json.Unmarshal(raw, &request); err != nil {
		t.Fatal(err)
	}
	if request.Version != 1 || request.Scope != "diagnostic-writer-focused" {
		t.Fatal("unknown contract")
	}
	unhex := func(s string) string {
		v, e := hex.DecodeString(s)
		if e != nil {
			t.Fatal(e)
		}
		return string(v)
	}
	cases := []any{}
	for _, r := range request.Cases {
		row := map[string]any{"id": r.ID, "state": "executed"}
		func() {
			defer func() {
				if failure := recover(); failure != nil {
					row["state"] = "failed"
					row["panic"] = failure
				}
			}()
			files := []*ast.SourceFile{}
			inputs := []*harnessutil.TestFile{}
			for _, f := range r.Files {
				files = append(files, parser.ParseSourceFile(ast.SourceFileParseOptions{FileName: unhex(f.NameHex), Path: tspath.Path(unhex(f.NameHex))}, unhex(f.ContentHex), core.ScriptKindTS))
			}
			for _, i := range r.Inputs {
				inputs = append(inputs, &harnessutil.TestFile{UnitName: unhex(r.Files[i].NameHex), Content: unhex(r.Files[i].ContentHex)})
			}
			var build func(S08ErrorSpec) *ast.Diagnostic
			build = func(s S08ErrorSpec) *ast.Diagnostic {
				var file *ast.SourceFile
				if s.File != nil {
					file = files[*s.File]
				}
				args := []string{}
				for _, a := range s.ArgsHex {
					args = append(args, unhex(a))
				}
				chain := []*ast.Diagnostic{}
				for _, a := range s.Chain {
					chain = append(chain, build(a))
				}
				related := []*ast.Diagnostic{}
				for _, a := range s.Related {
					related = append(related, build(a))
				}
				if s.TextHex != nil {
					return ast.NewExternalDiagnostic(file, core.NewTextRange(s.Pos, s.End), unhex(s.SourceHex), s.Category, s.Code, unhex(*s.TextHex)).SetMessageChain(chain).SetRelatedInfo(related)
				}
				return ast.NewDiagnosticFromSerialized(file, core.NewTextRange(s.Pos, s.End), s.Code, s.Category, diagnostics.Key(s.Key), args, chain, related, false, false, false)
			}
			diags := []*ast.Diagnostic{}
			for _, s := range r.Diagnostics {
				diags = append(diags, build(s))
			}
			wrapped := diagnosticwriter.WrapASTDiagnostics(diags)
			opts := &diagnosticwriter.FormattingOptions{NewLine: "\r\n", ComparePathsOptions: tspath.ComparePathsOptions{CurrentDirectory: r.Cwd}}
			var plain, pretty, summary strings.Builder
			diagnosticwriter.WriteFormatDiagnostics(&plain, diagnosticwriter.ToDiagnostics(wrapped), opts)
			diagnosticwriter.FormatDiagnosticsWithColorAndContext(&pretty, diagnosticwriter.ToDiagnostics(wrapped), opts)
			diagnosticwriter.WriteErrorSummaryText(&summary, diagnosticwriter.ToDiagnostics(wrapped), opts)
			row["plain_hex"] = hex.EncodeToString([]byte(plain.String()))
			row["pretty_hex"] = hex.EncodeToString([]byte(pretty.String()))
			row["summary_hex"] = hex.EncodeToString([]byte(summary.String()))
			for _, p := range []bool{false, true} {
				key := "errors_plain"
				if p {
					key = "errors_pretty"
				}
				if len(diags) == 0 {
					row[key] = map[string]any{"state": "no_content"}
				} else {
					value := GetErrorBaseline(t, inputs, wrapped, diagnosticwriter.CompareASTDiagnostics, p)
					row[key] = map[string]any{"state": "content", "text_hex": hex.EncodeToString([]byte(value))}
				}
			}
		}()
		cases = append(cases, row)
	}
	hash := sha256.Sum256(raw)
	out := map[string]any{"version": 1, "request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH, "cases": cases}
	data, err := json.Marshal(out)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("S08_OUTPUT"), data, 0600); err != nil {
		t.Fatal(err)
	}
}
