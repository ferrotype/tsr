package tsoptions_test

// Phase 1 F3a, access only: the 53 `parseCommandLine` and 27 `parseBuildOptions`
// reference outputs under `tsoptions/commandLineParsing`.
//
// Unlike F2a's carried `config/matchFiles` renderer, this group has a real
// pinned producer. `TestCommandLineParseResult`, `TestParseCommandLineVerifyNull`
// and `TestParseBuildCommandLine` render all 80 outputs through
// `formatNewBaseline` (commandlineparser_test.go:334) and
// `formatNewBaselineBuild` (:456), and `scripts/phase1.py map-baselines`
// re-verifies at this pin that those renderings reproduce the committed bytes.
// Nothing here carries or restates an assembly: this file compiles into the
// pinned `tsoptions_test` package, so it calls those two functions, and for the
// eight outputs that need a synthesised option declaration it calls the pinned
// `createVerifyNullForNonNullIncluded` (:502) rather than restating it.
//
// What this file adds over the pinned tests is the seam the plan asks for. The
// two renderers are pure functions of the sections handed to them -- an
// argument vector plus, for the compiler group, an options payload, a file-name
// string and an error text -- so the same call renders a *Rust* observation's
// sections. That is why each row reports its sections separately as well as the
// assembled bytes: a difference can then be attributed to a section rather than
// to the envelope.
//
// Inputs are recovered from the baseline's own *input* section. Every one of
// the 80 frozen files opens with `Args::` followed by the argument vector the
// invocation used, written by the pinned renderer as `["a", "b"]` with no
// escaping; all 80 round-trip through that rule exactly, which is asserted here
// per row rather than assumed. The probe never reads `CompilerOptions::`,
// `buildOptions::`, `compilerOptions::`, `FileNames::`, `Projects::` or
// `Errors::` -- the result sections -- to build a request, and it never edits,
// repairs or special-cases a baseline.

import (
	"crypto/sha256"
	"encoding/hex"
	stdjson "encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/diagnosticwriter"
	"github.com/microsoft/TypeScript/tsc/internal/json"
	"github.com/microsoft/TypeScript/tsc/internal/repo"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions/tsoptionstest"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/osvfs"
)

const (
	commandLineArgsHead = "Args::\n"
	// The two operations this probe serves. They name the pinned entry point
	// each group's baseline is written from, which for the compiler group is
	// the test worker in the pinned export_test.go, not `ParseCommandLine`:
	// commandlineparser_test.go:280 calls `ParseCommandLineTestWorker`, and a
	// case that claimed `ParseCommandLine` would be claiming an entry point no
	// frozen byte here depends on.
	commandLineOperation  = "tsoptions.parseCommandLineBaseline"
	buildOptionsOperation = "tsoptions.parseBuildOptionsBaseline"
)

type commandLineBaselineRequest struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Baseline  string `json:"baseline"`
	// The synthesised option declaration the eight `option of type ...`
	// outputs need, named by the kind the pinned test passes to
	// `createVerifyNullForNonNullIncluded`. Empty for the other 72.
	SyntheticOption string `json:"synthetic_option"`
}

// commandLineArgs recovers the argument vector from the baseline's `Args::`
// section and proves the recovery exact by re-rendering it with the pinned
// writer's own rule (commandlineparser_test.go:340-350: a bare `"` on each
// side, `, ` between, no escaping) and requiring the result to equal the line
// that was read. A vector containing a quote or a backslash would not round
// trip, and this refuses it rather than guessing.
func commandLineArgs(t *testing.T, content string) []string {
	t.Helper()
	if !strings.HasPrefix(content, commandLineArgsHead) {
		t.Fatalf("baseline does not open with an Args:: section")
	}
	line, _, found := strings.Cut(content[len(commandLineArgsHead):], "\n")
	if !found || !strings.HasPrefix(line, "[") || !strings.HasSuffix(line, "]") {
		t.Fatalf("Args:: section is not a bracketed vector: %q", line)
	}
	body := line[1 : len(line)-1]
	var args []string
	if body != "" {
		if !strings.HasPrefix(body, `"`) || !strings.HasSuffix(body, `"`) {
			t.Fatalf("Args:: entries are not quoted: %q", line)
		}
		args = strings.Split(body[1:len(body)-1], `", "`)
	}
	var rendered strings.Builder
	rendered.WriteByte('[')
	for i, arg := range args {
		if i > 0 {
			rendered.WriteString(", ")
		}
		rendered.WriteByte('"')
		rendered.WriteString(arg)
		rendered.WriteByte('"')
	}
	rendered.WriteByte(']')
	if rendered.String() != line {
		t.Fatalf("Args:: recovery does not round trip: read %q, re-rendered %q", line, rendered.String())
	}
	return args
}

// syntheticDeclarations calls the pinned constructor for the eight outputs
// whose argument vector names `--optionName`, an option that exists only inside
// `TestParseCommandLineVerifyNull`. The kind comes from the request, which took
// it from the baseline's own title ("option of type string", "option of type
// number"); the declaration itself is built by the pinned
// `createVerifyNullForNonNullIncluded` (commandlineparser_test.go:502), so no
// part of the option's shape is restated here.
func syntheticDeclarations(t *testing.T, kind string) []*tsoptions.CommandLineOption {
	t.Helper()
	switch kind {
	case "":
		return nil
	case "string":
		return createVerifyNullForNonNullIncluded("option of type string", tsoptions.CommandLineOptionTypeString, "hello").optDecls
	case "number":
		return createVerifyNullForNonNullIncluded("option of type number", tsoptions.CommandLineOptionTypeNumber, "10").optDecls
	default:
		t.Fatalf("no pinned constructor for synthetic option kind %q", kind)
		return nil
	}
}

func TestPhase1ConfigCommandLine(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []commandLineBaselineRequest `json:"requests"`
	}
	if err := stdjson.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Operation != commandLineOperation && request.Operation != buildOptionsOperation {
			row["result"] = "native_unavailable"
			row["reason"] = "operation is not served by the command-line baseline probe"
			observations = append(observations, row)
			continue
		}
		// `Baseline` is the index name ("tsoptions/commandLineParsing/..."),
		// resolved against the pinned reference root exactly as
		// baseline.Run does (baseline.go:26, :84).
		path := filepath.Join(repo.TestDataPath(), "baselines", "reference", filepath.FromSlash(request.Baseline))
		expected, readErr := os.ReadFile(path)
		if readErr != nil {
			t.Fatalf("%s: %v", request.Case, readErr)
		}
		content := string(expected)
		args := commandLineArgs(t, content)

		var rendered string
		sections := map[string]any{}
		switch request.Operation {
		case commandLineOperation:
			// The pinned host construction at commandlineparser_test.go:280.
			parsed := tsoptions.ParseCommandLineTestWorker(
				syntheticDeclarations(t, request.SyntheticOption), args, osvfs.FS(), t.TempDir())
			optionBytes, marshalErr := json.Marshal(parsed.Options)
			if marshalErr != nil {
				t.Fatalf("%s: %v", request.Case, marshalErr)
			}
			fileNames := strings.Join(parsed.FileNames, ",")
			errorText := formatDiagnostics(parsed.Errors)
			sections["args"] = args
			sections["compilerOptions"] = stdjson.RawMessage(optionBytes)
			sections["fileNames"] = fileNames
			sections["errors"] = errorText
			rendered = formatNewBaseline(args, optionBytes, fileNames, errorText)

		case buildOptionsOperation:
			// The pinned host construction at commandlineparser_test.go:382-385.
			parsed := tsoptions.ParseBuildCommandLine(args, &tsoptionstest.VfsParseConfigHost{
				Vfs:              osvfs.FS(),
				CurrentDirectory: tspath.NormalizeSlashes(repo.TestDataPath()),
			})
			buildBytes, marshalErr := json.Marshal(parsed.BuildOptions)
			if marshalErr != nil {
				t.Fatalf("%s: %v", request.Case, marshalErr)
			}
			compilerBytes, marshalErr := json.Marshal(parsed.CompilerOptions)
			if marshalErr != nil {
				t.Fatalf("%s: %v", request.Case, marshalErr)
			}
			projects := strings.Join(parsed.Projects, ",")
			errorText := formatDiagnostics(parsed.Errors)
			sections["args"] = args
			sections["buildOptions"] = stdjson.RawMessage(buildBytes)
			sections["compilerOptions"] = stdjson.RawMessage(compilerBytes)
			sections["projects"] = projects
			sections["errors"] = errorText
			rendered = formatNewBaselineBuild(args, buildBytes, compilerBytes, projects, errorText)
		}

		renderedSum := sha256.Sum256([]byte(rendered))
		expectedSum := sha256.Sum256(expected)
		row["result"] = "observed"
		row["observation"] = map[string]any{
			"baseline":        request.Baseline,
			"sections":        sections,
			"rendered":        rendered,
			"rendered_sha256": hex.EncodeToString(renderedSum[:]),
			"expected_sha256": hex.EncodeToString(expectedSum[:]),
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
	encoded, err := stdjson.MarshalIndent(output, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), append(encoded, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
}

// formatDiagnostics is the pinned error rendering both baseline groups use
// (commandlineparser_test.go:299 and :391): the production
// `diagnosticwriter.WriteFormatDiagnostics` with a `\n` newline and no other
// formatting option set.
func formatDiagnostics(errors []*ast.Diagnostic) string {
	var formatted strings.Builder
	diagnosticwriter.WriteFormatDiagnostics(&formatted,
		diagnosticwriter.FromASTDiagnostics(errors),
		&diagnosticwriter.FormattingOptions{NewLine: "\n"})
	return formatted.String()
}
