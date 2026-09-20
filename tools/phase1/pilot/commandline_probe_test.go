package tsoptions_test

// Access only, and the F0 instance of the plan's shared test-envelope seam.
//
// This file compiles into the pinned `tsoptions_test` package, so it calls the
// original `formatNewBaseline` and `formatNewBaselineBuild` rather than
// reimplementing their section assembly. It records two things per case:
//
//   * the structured parse observation, which is what a Rust comparison should
//     be judged on first, and
//   * the rendered test envelope produced by the pinned renderer, which is the
//     byte authority the 53 + 27 commandLineParsing baselines are written from.
//
// It calls no Rust, reads no expected baseline file and repairs nothing.

import (
	"crypto/sha256"
	"encoding/hex"
	stdjson "encoding/json"
	"os"
	"runtime"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/diagnosticwriter"
	"github.com/microsoft/TypeScript/tsc/internal/json"
	"github.com/microsoft/TypeScript/tsc/internal/repo"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions/tsoptionstest"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/osvfs"
)

type commandLineRequest struct {
	Case             string   `json:"case"`
	Operation        string   `json:"operation"`
	Args             []string `json:"args"`
	CurrentDirectory string   `json:"currentDirectory"`
}

func TestPhase1PilotCommandLine(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []commandLineRequest `json:"requests"`
	}
	if err := stdjson.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		switch request.Operation {
		case "tsoptions.parseCommandLine":
			parsed := tsoptions.ParseCommandLineTestWorker(nil, request.Args, osvfs.FS(), t.TempDir())
			optionBytes, marshalErr := json.Marshal(parsed.Options)
			if marshalErr != nil {
				t.Fatal(marshalErr)
			}
			fileNames := strings.Join(parsed.FileNames, ",")
			var errors strings.Builder
			diagnosticwriter.WriteFormatDiagnostics(&errors,
				diagnosticwriter.FromASTDiagnostics(parsed.Errors),
				&diagnosticwriter.FormattingOptions{NewLine: "\n"})
			row["result"] = "observed"
			row["observation"] = map[string]any{
				"args":      request.Args,
				"options":   stdjson.RawMessage(optionBytes),
				"fileNames": fileNames,
				"errors":    errors.String(),
			}
			// The pinned renderer, not a second implementation of it.
			row["rendered_baseline"] = formatNewBaseline(request.Args, optionBytes, fileNames, errors.String())

		case "tsoptions.parseBuildCommandLine":
			// The same host construction the pinned test uses, so the observed
			// result is the one its baselines are written from.
			parsed := tsoptions.ParseBuildCommandLine(request.Args, &tsoptionstest.VfsParseConfigHost{
				Vfs:              osvfs.FS(),
				CurrentDirectory: tspath.NormalizeSlashes(repo.TestDataPath()),
			})
			optionBytes, marshalErr := json.Marshal(parsed.BuildOptions)
			if marshalErr != nil {
				t.Fatal(marshalErr)
			}
			compilerBytes, marshalErr := json.Marshal(parsed.CompilerOptions)
			if marshalErr != nil {
				t.Fatal(marshalErr)
			}
			projects := strings.Join(parsed.Projects, ",")
			var errors strings.Builder
			diagnosticwriter.WriteFormatDiagnostics(&errors,
				diagnosticwriter.FromASTDiagnostics(parsed.Errors),
				&diagnosticwriter.FormattingOptions{NewLine: "\n"})
			row["result"] = "observed"
			row["observation"] = map[string]any{
				"args":     request.Args,
				"options":  stdjson.RawMessage(optionBytes),
				"projects": projects,
				"errors":   errors.String(),
			}
			row["rendered_baseline"] = formatNewBaselineBuild(request.Args, optionBytes, compilerBytes, projects, errors.String())

		default:
			row["result"] = "native_unavailable"
			row["reason"] = "operation is not served by the command-line probe"
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
	data, err := stdjson.Marshal(output)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), append(data, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}
