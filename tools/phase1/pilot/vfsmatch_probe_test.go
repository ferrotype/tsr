package vfsmatch

// Access only: the Phase 1 F0 pilot calls the pinned ReadDirectory entry point
// and records its ordered result. It adds no matching logic of its own, reads
// no expected baseline, and reports an unsupported request rather than
// substituting a result.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

type pilotRequest struct {
	Case                      string   `json:"case"`
	Operation                 string   `json:"operation"`
	Dialect                   string   `json:"dialect"`
	CurrentDirectory          string   `json:"currentDirectory"`
	Path                      string   `json:"path"`
	UseCaseSensitiveFileNames bool     `json:"useCaseSensitiveFileNames"`
	Files                     []string `json:"files"`
	Extensions                []string `json:"extensions"`
	Excludes                  []string `json:"excludes"`
	Includes                  []string `json:"includes"`
	Depth                     *int     `json:"depth"`
}

func TestPhase1PilotReadDirectory(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []pilotRequest `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		switch {
		case request.Operation != "vfsmatch.readDirectory":
			// The native authority for other pilot operations lives in another
			// package; this probe declines rather than guessing.
			row["result"] = "native_unavailable"
			row["reason"] = "operation is not served by the vfsmatch probe"
		case request.Dialect != "vfsmatch":
			row["result"] = "native_unavailable"
			row["reason"] = "request dialect is not the configuration matcher"
		default:
			files := make(map[string]string, len(request.Files))
			for _, name := range request.Files {
				files[name] = ""
			}
			host := vfstest.FromMap(files, request.UseCaseSensitiveFileNames)
			depth := UnlimitedDepth
			if request.Depth != nil && *request.Depth >= 0 {
				depth = *request.Depth
			}
			path := request.Path
			if path == "" {
				path = request.CurrentDirectory
			}
			matched := ReadDirectory(host, request.CurrentDirectory, path,
				request.Extensions, request.Excludes, request.Includes, depth)
			if matched == nil {
				matched = []string{}
			}
			row["result"] = "observed"
			row["observation"] = map[string]any{"files": matched}
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
