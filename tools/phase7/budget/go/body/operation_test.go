package budget

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/vfs/iovfs"
)

func tiny(operation string) Input {
	config := Bytes(`{"compilerOptions":{"strict":true,"outDir":"out"},"files":["main.ts"]}`)
	source := Bytes("export const answer: number = 42;\n")
	return Input{CurrentDirectory: Bytes("/project"), CaseSensitive: true, Entries: []Entry{{Kind: "physical_file", Path: Bytes("/project/tsconfig.json"), Bytes: &config}, {Kind: "physical_file", Path: Bytes("/project/main.ts"), Bytes: &source}}, Configuration: Configuration{ConfigPath: Bytes("/project/tsconfig.json"), Arguments: []Bytes{}}, Operation: operation}
}
func call(t *testing.T, e *Engine, kind string, input *Input) Response {
	t.Helper()
	raw, err := json.Marshal(Command{Version: 1, Command: kind, ID: "1", Input: input})
	if err != nil {
		t.Fatal(err)
	}
	r, err := e.ExecuteCommand(raw)
	if err != nil {
		t.Fatal(err)
	}
	return r
}
func TestTinyProductionLifecycle(t *testing.T) {
	e := &Engine{}
	input := tiny("check_emit")
	call(t, e, "prepare", &input)
	if len(e.input.Entries) != 0 {
		t.Fatal("decoded file transport retained after snapshot preparation")
	}
	call(t, e, "execute", nil)
	call(t, e, "checkpoint", nil)
	o := call(t, e, "observe", nil).Value.(Observation)
	if o.Outcome != "compiled" || o.Checkers != "1" || o.Types == "0" || len(o.Diagnostics) != 0 || len(o.Sources) < 2 || len(o.Writes) != 1 || string(o.Writes[0].Name) != "/project/out/main.js" || !strings.Contains(string(o.Writes[0].Text), "answer") {
		t.Fatalf("unexpected production work: outcome=%s checkers=%s types=%s diagnostics=%v sources=%d writes=%v", o.Outcome, o.Checkers, o.Types, o.Diagnostics, len(o.Sources), o.Writes)
	}
	call(t, e, "release", nil)
	if e.program != nil || e.snapshot != nil || e.diagnostics != nil || e.writes != nil || e.config != nil || e.emit != nil || e.input != nil || e.fs != nil {
		t.Fatal("retained operation escaped release")
	}
}
func TestPhysicalSnapshotBOMCaseAndSymlink(t *testing.T) {
	data := Bytes{0xff, 0xfe, 'x', 0}
	target := Bytes("../Real")
	s, err := newSnapshot(&Input{CurrentDirectory: Bytes("/p"), CaseSensitive: false, Entries: []Entry{{Kind: "physical_file", Path: Bytes("/p/Real/X.ts"), Bytes: &data}, {Kind: "symlink", Path: Bytes("/p/links/alias"), Target: &target}}})
	if err != nil {
		t.Fatal(err)
	}
	f := iovfs.From(s, false)
	text, ok := f.ReadFile("/p/links/alias/x.ts")
	if !ok || text != "x" {
		t.Fatalf("physical decoding/link: %q %v", text, ok)
	}
	if f.Realpath("/P/LINKS/alias/x.ts") != "/p/Real/X.ts" {
		t.Fatal("canonical target casing lost")
	}
	entries := f.GetAccessibleEntries("/p/links")
	if len(entries.Directories) != 1 || entries.Directories[0] != "alias" {
		t.Fatalf("symlink directory entries: %+v", entries)
	}
	if _, ok := f.ReadFile("/etc/passwd"); ok {
		t.Fatal("unexpected OS fallback")
	}
}
func TestStrictProtocolAndTerminalState(t *testing.T) {
	input := tiny("check")
	raw, _ := json.Marshal(Command{Version: 1, Command: "prepare", ID: "1", Input: &input})
	for _, bad := range []string{strings.Replace(string(raw), `"case_sensitive":true,`, "", 1), strings.Replace(string(raw), `"arguments":[]`, `"arguments":null`, 1), strings.Replace(string(raw), `"version":1`, `"version":1,"version":1`, 1), strings.Replace(string(raw), `"version":1`, `"version":2`, 1), strings.Replace(string(raw), `"id":"1"`, `"id":"01"`, 1), strings.Replace(string(raw), `"operation":"check"`, `"operation":"check","extra":1`, 1)} {
		if _, err := Decode([]byte(bad)); err == nil {
			t.Fatalf("accepted malformed command %s", bad)
		}
	}
	e := &Engine{}
	call(t, e, "prepare", &input)
	if _, err := e.ExecuteCommand([]byte(`{"version":1,"command":"observe","id":"1"}`)); err == nil {
		t.Fatal("observe before checkpoint accepted")
	}
	if _, err := e.ExecuteCommand([]byte(`{"version":1,"command":"execute","id":"1"}`)); err == nil {
		t.Fatal("failed engine reused")
	}
}

func TestProtocolCaseAliasesAndNullBytes(t *testing.T) {
	input := tiny("check")
	raw, _ := json.Marshal(Command{Version: 1, Command: "prepare", ID: "1", Input: &input})
	for _, bad := range []string{
		strings.Replace(string(raw), `"version":1`, `"version":2,"Version":1`, 1),
		strings.Replace(string(raw), `"case_sensitive":true`, `"case_sensitive":true,"Case_Sensitive":false`, 1),
		strings.Replace(string(raw), `"current_directory":[47`, `"current_directory":[null`, 1),
	} {
		if _, err := Decode([]byte(bad)); err == nil {
			t.Fatalf("accepted alias/null byte: %s", bad)
		}
	}
}
