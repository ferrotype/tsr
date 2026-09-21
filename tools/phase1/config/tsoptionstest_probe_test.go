package tsoptionstest_test

// Phase 1 F3a, access only: `internal/tsoptions/tsoptionstest`, the pinned
// parse-config host factory.
//
// This package is not a `_test.go` file -- it ships in the module and other
// packages' tests import it -- so it has a real caller-visible contract: it is
// how every pinned config test, and every F3a probe, turns a `{path -> content}`
// map into a `tsoptions.ParseConfigHost`. That map is exactly the frozen-input
// shape the plan asks F3a to preserve, which is why this surface is prepared
// rather than treated as harness furniture.
//
// What each case observes is the host's *composition*: which paths the built
// filesystem exposes and as what, what it reports for case sensitivity and the
// current directory, and how a symlink entry's two sides were normalized. The
// underlying filesystem behavior belongs to `internal/vfs/vfstest`, which F2a
// prepared; this probe does not re-test it, and no case here claims a vfstest
// operation.
//
// `fixRoot` (vfsparseconfighost.go:10) is deliberately absent: a grep over the
// whole pin finds its definition and nothing else, so no caller fixes its
// contract. It is carried as a roster exemption with that evidence, not as a
// case that would have to invent a caller.

import (
	"crypto/sha256"
	"encoding/hex"
	stdjson "encoding/json"
	"os"
	"runtime"
	"sort"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions/tsoptionstest"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
)

type hostRequest struct {
	Case             string            `json:"case"`
	Operation        string            `json:"operation"`
	Subject          string            `json:"subject"`
	Action           string            `json:"action"`
	Files            map[string]string `json:"files"`
	Symlinks         map[string]string `json:"symlinks"`
	CurrentDirectory string            `json:"currentDirectory"`
	CaseSensitive    bool              `json:"caseSensitive"`
	// Paths the case asks the built host about, in the order given. The answer
	// travels as an array so the comparison sees the order.
	Probe    []string `json:"probe"`
	JSONText string   `json:"jsonText"`
}

// describeHost reports what the built host exposes, without re-testing the
// filesystem underneath it: for each probed path, whether the host's FS sees a
// file or a directory there, what it reads back, and what it resolves the path
// to. The answers travel in request order under `ordered`.
func describeHost(host *tsoptionstest.VfsParseConfigHost, probe []string) map[string]any {
	rows := make([]map[string]any, 0, len(probe))
	for _, path := range probe {
		rows = append(rows, describePath(host, path))
	}
	return map[string]any{
		"current_directory":             host.GetCurrentDirectory(),
		"use_case_sensitive_file_names": host.FS().UseCaseSensitiveFileNames(),
		"ordered":                       rows,
	}
}

// describePath asks the built host about one path. The filesystem the factory
// builds REFUSES a path that is not absolute, and it refuses it by panicking
// (`vfs: path %q is not absolute`), so a relative spelling is a real,
// observable contract of the host rather than a case that has to be avoided.
// The refusal is recorded as a flag and not as the pin's wording: what the host
// does is the contract, how it phrases its own panic is not, and freezing the
// sentence would make the case fail on a rewording that changes no behavior.
func describePath(host *tsoptionstest.VfsParseConfigHost, path string) (row map[string]any) {
	row = map[string]any{"path": path, "refused": false}
	defer func() {
		if recovered := recover(); recovered != nil {
			row = map[string]any{"path": path, "refused": true}
		}
	}()
	fs := host.FS()
	row["file_exists"] = fs.FileExists(path)
	row["directory_exists"] = fs.DirectoryExists(path)
	if content, ok := fs.ReadFile(path); ok {
		row["content"] = content
	} else {
		row["content"] = nil
	}
	row["realpath"] = fs.Realpath(path)
	return row
}

func TestPhase1ConfigHost(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []hostRequest `json:"requests"`
	}
	if err := stdjson.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Subject != "parseConfigHost" {
			row["result"] = "native_unavailable"
			row["reason"] = "operation is not served by the parse-config host probe"
			observations = append(observations, row)
			continue
		}
		switch request.Action {
		case "from_map":
			host := tsoptionstest.NewVFSParseConfigHost(request.Files, request.CurrentDirectory, request.CaseSensitive)
			row["result"] = "observed"
			row["observation"] = describeHost(host, request.Probe)

		case "from_map_with_symlinks":
			host := tsoptionstest.NewVFSParseConfigHostWithSymlinks(
				request.Files, request.Symlinks, request.CurrentDirectory, request.CaseSensitive)
			observation := describeHost(host, request.Probe)
			// The two sides of every symlink entry are normalized against the
			// current directory before the entry is built
			// (vfsparseconfighost.go:52-53). Record the normalization the
			// factory performed, in a stable order, so a case can tell a
			// normalization difference from a resolution difference.
			links := make([]map[string]any, 0, len(request.Symlinks))
			names := make([]string, 0, len(request.Symlinks))
			for link := range request.Symlinks {
				names = append(names, link)
			}
			sort.Strings(names)
			for _, link := range names {
				links = append(links, map[string]any{
					"declared_link":     link,
					"declared_target":   request.Symlinks[link],
					"normalized_link":   tspath.GetNormalizedAbsolutePath(link, request.CurrentDirectory),
					"normalized_target": tspath.GetNormalizedAbsolutePath(request.Symlinks[link], request.CurrentDirectory),
				})
			}
			observation["symlinks"] = links
			row["result"] = "observed"
			row["observation"] = observation

		case "get_parsed_command_line":
			parsed := tsoptionstest.GetParsedCommandLine(
				t, request.JSONText, request.Files, request.CurrentDirectory, request.CaseSensitive)
			codes := make([]int, 0, len(parsed.Errors))
			for _, diagnostic := range parsed.Errors {
				codes = append(codes, int(diagnostic.Code()))
			}
			row["result"] = "observed"
			row["observation"] = map[string]any{
				"config_file_name": tspath.CombinePaths(request.CurrentDirectory, "tsconfig.json"),
				"file_names":       parsed.ParsedConfig.FileNames,
				"error_codes":      codes,
				"has_config_file":  parsed.ConfigFile != nil,
			}

		default:
			row["result"] = "harness_failed"
			row["error"] = "unknown parse-config host action " + request.Action
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

// Keep the compiler honest about the import the accessor assertion uses.
var _ tsoptions.ParseConfigHost = (*tsoptionstest.VfsParseConfigHost)(nil)
