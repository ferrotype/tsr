package budget

import (
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"reflect"
	"testing"
)

func streamCall(e *Engine, name string, extra map[string]any) (Response, error) {
	c := map[string]any{"version": 2, "command": name, "id": "1"}
	for key, value := range extra {
		c[key] = value
	}
	raw, _ := json.Marshal(c)
	return e.ExecuteCommand(raw)
}
func streamedTiny(t *testing.T, e *Engine, unavailable bool) {
	t.Helper()
	input := tiny("check_emit")
	entries := input.Entries
	input.Entries = []Entry{}
	expectedContents := "2"
	if unavailable {
		expectedContents = "1"
	}
	_, err := streamCall(e, "prepare_begin", map[string]any{"input": input, "expected_entries": "3", "expected_files": "3", "expected_contents": expectedContents})
	if err != nil {
		t.Fatal(err)
	}
	metadata := []any{}
	for _, entry := range entries {
		metadata = append(metadata, map[string]any{"kind": "physical_file", "path_b64": base64.StdEncoding.EncodeToString(entry.Path), "raw_size": fmt.Sprint(len(*entry.Bytes))})
	}
	metadata = append(metadata, map[string]any{"kind": "physical_file", "path_b64": base64.StdEncoding.EncodeToString([]byte("/project/unread.bin")), "raw_size": "42"})
	if unavailable {
		metadata[1].(map[string]any)["raw_size"] = "42"
	}
	if _, err := streamCall(e, "prepare_metadata", map[string]any{"entries": metadata}); err != nil {
		t.Fatal(err)
	}
	for i, entry := range entries {
		if unavailable && i == 1 {
			break
		}
		data := *entry.Bytes
		hash := fmt.Sprintf("%x", sha256.Sum256(data))
		if _, err := streamCall(e, "content_begin", map[string]any{"path_b64": base64.StdEncoding.EncodeToString(entry.Path), "raw_size": fmt.Sprint(len(data)), "sha256": hash}); err != nil {
			t.Fatal(err)
		}
		// Two contiguous chunks prove offset progression is byte-based.
		mid := len(data) / 2
		for _, chunk := range []struct {
			offset int
			data   []byte
		}{{0, data[:mid]}, {mid, data[mid:]}} {
			if _, err := streamCall(e, "content_chunk", map[string]any{"offset": fmt.Sprint(chunk.offset), "bytes_b64": base64.StdEncoding.EncodeToString(chunk.data)}); err != nil {
				t.Fatal(err)
			}
		}
		if _, err := streamCall(e, "content_end", nil); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := streamCall(e, "prepare_end", nil); err != nil {
		t.Fatal(err)
	}
}
func TestStreamedSnapshotMatchesOrdinaryWorkAndDropsTransport(t *testing.T) {
	e := &Engine{}
	streamedTiny(t, e, false)
	if e.preparation != nil || len(e.input.Entries) != 0 || !e.fs.FileExists("/project/unread.bin") {
		t.Fatal("stream retention or unavailable metadata wrong")
	}
	for _, command := range []string{"execute", "checkpoint"} {
		if _, err := streamCall(e, command, nil); err != nil {
			t.Fatal(err)
		}
	}
	r, err := streamCall(e, "observe", nil)
	if err != nil {
		t.Fatal(err)
	}
	o := r.Value.(Observation)
	if r.Version != 2 || o.Checkers != "1" || len(o.Diagnostics) != 0 || len(o.Writes) != 1 {
		t.Fatalf("unexpected streamed work: %+v", o)
	}
	if _, err := streamCall(e, "release", nil); err != nil {
		t.Fatal(err)
	}
}
func TestUnavailableStreamFileFailsOnReadAndIsTerminal(t *testing.T) {
	e := &Engine{}
	streamedTiny(t, e, true)
	if !e.fs.FileExists("/project/main.ts") {
		t.Fatal("unavailable file pretended missing")
	}
	if _, err := streamCall(e, "execute", nil); err == nil {
		t.Fatal("read unavailable bytes succeeded")
	}
	if _, err := streamCall(e, "release", nil); err == nil {
		t.Fatal("failed stream was reusable")
	}
}
func TestStreamRefusesDuplicateMetadataAndPrematureFinalization(t *testing.T) {
	for _, name := range []string{"prepare_end", "duplicate"} {
		e := &Engine{}
		input := tiny("check")
		input.Entries = []Entry{}
		if _, err := streamCall(e, "prepare_begin", map[string]any{"input": input, "expected_entries": "2", "expected_files": "0", "expected_contents": "0"}); err != nil {
			t.Fatal(err)
		}
		if name == "prepare_end" {
			if _, err := streamCall(e, name, nil); err == nil {
				t.Fatal("incomplete stream accepted")
			}
			continue
		}
		entry := map[string]any{"kind": "directory", "path_b64": base64.StdEncoding.EncodeToString([]byte("/project/a"))}
		if _, err := streamCall(e, "prepare_metadata", map[string]any{"entries": []any{entry, entry}}); err == nil {
			t.Fatal("duplicate metadata accepted")
		}
	}
}

func TestBoundedObservationMatchesWholeAndReusesFreeze(t *testing.T) {
	e := &Engine{}
	streamedTiny(t, e, false)
	for _, name := range []string{"execute", "checkpoint"} {
		if _, err := streamCall(e, name, nil); err != nil {
			t.Fatal(err)
		}
	}
	whole, err := streamCall(e, "observe", nil)
	if err != nil {
		t.Fatal(err)
	}
	begin, err := streamCall(e, "observe_begin", nil)
	if err != nil {
		t.Fatal(err)
	}
	cached := e.observation
	raw, err := json.Marshal(begin.Value)
	if err != nil {
		t.Fatal(err)
	}
	var header map[string]json.RawMessage
	if err = json.Unmarshal(raw, &header); err != nil {
		t.Fatal(err)
	}
	if _, ok := header["sources"]; ok {
		t.Fatal("begin contains source rows")
	}
	if _, ok := header["writes"]; ok {
		t.Fatal("begin contains write rows")
	}
	o := whole.Value.(Observation)
	if string(header["source_count"]) != fmt.Sprintf("%q", n(len(o.Sources))) || string(header["write_count"]) != fmt.Sprintf("%q", n(len(o.Writes))) {
		t.Fatal("wrong row counts")
	}
	for i, want := range o.Sources {
		row, err := streamCall(e, "observe_source", map[string]any{"index": n(i)})
		if err != nil || !reflect.DeepEqual(row.Value, want) {
			t.Fatalf("source %d differs: %v", i, err)
		}
	}
	for i, want := range o.Writes {
		row, err := streamCall(e, "observe_write", map[string]any{"index": n(i)})
		if err != nil || !reflect.DeepEqual(row.Value, want) {
			t.Fatalf("write %d differs: %v", i, err)
		}
	}
	if _, err := streamCall(e, "observe_begin", nil); err != nil || e.observation != cached {
		t.Fatal("repeat observation refroze results")
	}
	if _, err := streamCall(e, "release", nil); err != nil || e.observation != nil {
		t.Fatal("release retained observation cache")
	}
}
func TestBoundedObservationRefusesPrematureAndInvalidIndex(t *testing.T) {
	for _, index := range []any{nil, "01", "18446744073709551616", "999999", 1} {
		e := &Engine{}
		streamedTiny(t, e, false)
		for _, name := range []string{"execute", "checkpoint"} {
			if _, err := streamCall(e, name, nil); err != nil {
				t.Fatal(err)
			}
		}
		if _, err := streamCall(e, "observe_source", map[string]any{"index": index}); err == nil {
			t.Fatalf("accepted bad index %v", index)
		}
	}
	e := &Engine{}
	streamedTiny(t, e, false)
	if _, err := streamCall(e, "observe_begin", nil); err == nil {
		t.Fatal("observation before checkpoint accepted")
	}
}
