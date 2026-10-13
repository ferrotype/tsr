//go:build !js

package budget

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func discoveryFixture(t *testing.T) (string, string, string, []byte) {
	t.Helper()
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	source := []byte("\xef\xbb\xbfexport const answer: number = 'invalid';")
	files := map[string][]byte{"tsconfig.json": []byte(`{"compilerOptions":{"noLib":true},"files":["main.ts"]}`), "main.ts": source, "unread.bin": {0, 255, 42}}
	metadata := filepath.Join(root, "metadata.ndjson")
	file, err := os.Create(metadata)
	if err != nil {
		t.Fatal(err)
	}
	encoder := json.NewEncoder(file)
	if err := encoder.Encode(map[string]any{"version": 1, "kind": "header", "root_b64": encoded(root)}); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"tsconfig.json", "main.ts", "unread.bin"} {
		path := filepath.Join(root, name)
		data := files[name]
		if err := os.WriteFile(path, data, 0600); err != nil {
			t.Fatal(err)
		}
		if err := encoder.Encode(map[string]any{"kind": "physical_file", "path_b64": encoded(path), "raw_size": u(uint64(len(data)))}); err != nil {
			t.Fatal(err)
		}
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}
	inputPath := filepath.Join(root, "input.json")
	input := Input{CurrentDirectory: Bytes(root), CaseSensitive: true, Entries: []Entry{}, Configuration: Configuration{ConfigPath: Bytes(filepath.Join(root, "tsconfig.json")), Arguments: []Bytes{}}, Operation: "check_emit"}
	raw, err := json.Marshal(input)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(inputPath, raw, 0600); err != nil {
		t.Fatal(err)
	}
	return root, metadata, inputPath, source
}
func TestNativeDiscoveryActualReadClosure(t *testing.T) {
	root, metadata, input, source := discoveryFixture(t)
	output := filepath.Join(root, "events.ndjson")
	if err := Discover(metadata, input, output); err != nil {
		t.Fatal(err)
	}
	raw, err := os.ReadFile(output)
	if err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(strings.TrimSpace(string(raw)), "\n")
	found := false
	for _, line := range lines[:len(lines)-1] {
		var event readEvent
		if err := json.Unmarshal([]byte(line), &event); err != nil {
			t.Fatal(err)
		}
		if event.Kind != "read" {
			t.Fatalf("unexpected event: %s", line)
		}
		if event.Canonical != nil && *event.Canonical == encoded(filepath.Join(root, "unread.bin")) {
			t.Fatal("unread physical file acquired")
		}
		if event.Canonical != nil && *event.Canonical == encoded(filepath.Join(root, "main.ts")) {
			found = true
			if event.Hash == nil || *event.Hash != fmtHash(source) || event.Size == nil || *event.Size != u(uint64(len(source))) {
				t.Fatalf("raw source identity lost: %s", line)
			}
		}
	}
	var complete map[string]any
	if err := json.Unmarshal([]byte(lines[len(lines)-1]), &complete); err != nil {
		t.Fatal(err)
	}
	if !found || complete["kind"] != "complete" || complete["read_events"] != u(uint64(len(lines)-1)) {
		t.Fatalf("incomplete actual read closure: %s", raw)
	}
	if err := Discover(metadata, input, output); err == nil {
		t.Fatal("existing event capture overwritten")
	}
}
func fmtHash(raw []byte) string {
	sum := sha256.Sum256(raw)
	return hex.EncodeToString(sum[:])
}
func TestNativeDiscoveryRefusesStalePhysicalSize(t *testing.T) {
	root, metadata, input, _ := discoveryFixture(t)
	if err := os.WriteFile(filepath.Join(root, "main.ts"), []byte("changed"), 0600); err != nil {
		t.Fatal(err)
	}
	output := filepath.Join(root, "failure.ndjson")
	if err := Discover(metadata, input, output); err == nil {
		t.Fatal("stale metadata accepted")
	}
	raw, err := os.ReadFile(output)
	if err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(strings.TrimSpace(string(raw)), "\n")
	var last map[string]any
	if err := json.Unmarshal([]byte(lines[len(lines)-1]), &last); err != nil {
		t.Fatal(err)
	}
	if last["kind"] != "failure" {
		t.Fatalf("missing terminal failure: %s", raw)
	}
}

func TestNativeDiscoveryRecordedFileDisappearanceAndTargetCapture(t *testing.T) {
	root, metadata, input, _ := discoveryFixture(t)
	if err := os.Remove(filepath.Join(root, "main.ts")); err != nil {
		t.Fatal(err)
	}
	output := filepath.Join(root, "missing.ndjson")
	if err := Discover(metadata, input, output); err == nil {
		t.Fatal("recorded missing physical file treated as frozen absence")
	}
	raw, err := os.ReadFile(output)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(raw), `"kind":"complete"`) || !strings.Contains(string(raw), `"kind":"failure"`) {
		t.Fatalf("stale capture completed: %s", raw)
	}
	target := filepath.Join(root, "TaRgEt")
	if err := os.Mkdir(target, 0700); err != nil {
		t.Fatal(err)
	}
	if err := Discover(metadata, input, filepath.Join(target, "events.ndjson")); err == nil {
		t.Fatal("target capture accepted")
	}
	alias := filepath.Join(root, "alias")
	if err := os.Symlink(target, alias); err != nil {
		t.Fatal(err)
	}
	if err := Discover(metadata, input, filepath.Join(alias, "events.ndjson")); err == nil {
		t.Fatal("resolved target capture accepted")
	}
}

func TestNativeDiscoveryRejectsUnsupportedOperation(t *testing.T) {
	root, metadata, input, _ := discoveryFixture(t)
	raw, err := os.ReadFile(input)
	if err != nil {
		t.Fatal(err)
	}
	raw = []byte(strings.Replace(string(raw), `"operation":"check_emit"`, `"operation":"unknown"`, 1))
	if err := os.WriteFile(input, raw, 0600); err != nil {
		t.Fatal(err)
	}
	output := filepath.Join(root, "unsupported.ndjson")
	if err := Discover(metadata, input, output); err == nil || !strings.Contains(err.Error(), "unsupported operation") {
		t.Fatalf("unsupported operation did not fail: %v", err)
	}
	events, err := os.ReadFile(output)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(events), `"kind":"complete"`) || !strings.Contains(string(events), `"kind":"failure"`) {
		t.Fatalf("unsupported operation completed: %s", events)
	}
}

func TestNativeDiscoveryRejectsSnapshotLookupFailure(t *testing.T) {
	root, metadata, input, _ := discoveryFixture(t)
	config := filepath.Join(root, "tsconfig.json")
	raw, err := os.ReadFile(config)
	if err != nil {
		t.Fatal(err)
	}
	// Equal byte length preserves the physical metadata and isolates the lookup
	// failure. FileExists can swallow the error before any ReadFile call sees it.
	if err := os.WriteFile(config, []byte(strings.Replace(string(raw), "main.ts", "loop.ts", 1)), 0600); err != nil {
		t.Fatal(err)
	}
	file, err := os.OpenFile(metadata, os.O_APPEND|os.O_WRONLY, 0600)
	if err != nil {
		t.Fatal(err)
	}
	if err := json.NewEncoder(file).Encode(map[string]any{"kind": "symlink", "path_b64": encoded(filepath.Join(root, "loop.ts")), "target_b64": encoded("loop.ts")}); err != nil {
		t.Fatal(err)
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}
	output := filepath.Join(root, "lookup-failure.ndjson")
	if err := Discover(metadata, input, output); err == nil || !strings.Contains(err.Error(), "snapshot symlink cycle") {
		t.Fatalf("snapshot lookup error did not fail: %v", err)
	}
	events, err := os.ReadFile(output)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(events), `"kind":"complete"`) || !strings.Contains(string(events), `"kind":"failure"`) {
		t.Fatalf("snapshot lookup failure completed: %s", events)
	}
}
