package collections

// Access only: replays an ordered action trace against the pinned collection
// types and records what each action observed.
//
// It is an in-package test file because several operations under test are
// unexported (newMapWithSizeHint, OrderedMap.clone, resolveKeyName). Note that
// every *existing* test file in this directory declares `package
// collections_test`; in-package is legal but has no precedent here, so this
// file deliberately adds no helper the pinned tests could pick up.
//
// The trace shape is the contract the Rust driver answers: one ordered result
// per action, under `ordered`, so member order survives canonicalisation.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"testing"
)

type action struct {
	Op    string `json:"op"`
	Key   string `json:"key"`
	Value string `json:"value"`
	Index int    `json:"index"`
	Size  int    `json:"size"`
	Stop  int    `json:"stop"`
}

type leafRequest struct {
	Case      string   `json:"case"`
	Operation string   `json:"operation"`
	Subject   string   `json:"subject"`
	Actions   []action `json:"actions"`
}

// entries renders an ordered map as an entry array, never as a JSON object:
// an object's member order does not survive canonical comparison.
func entries(m *OrderedMap[string, string]) []any {
	out := []any{}
	for key, value := range m.Entries() {
		out = append(out, []any{key, value})
	}
	return out
}

func keysOf(m *OrderedMap[string, string]) []any {
	out := []any{}
	for key := range m.Keys() {
		out = append(out, key)
	}
	return out
}

// guarded runs f and converts a panic into a recorded class, so a case whose
// subject is the panic boundary observes it instead of failing the probe.
func guarded(f func() any) (result any, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			result = nil
			panicked = classify(r)
		}
	}()
	return f(), ""
}

func classify(r any) string {
	text := ""
	if err, ok := r.(error); ok {
		text = err.Error()
	} else if s, ok := r.(string); ok {
		text = s
	} else {
		text = "non-error panic"
	}
	switch {
	case contains(text, "index out of range"), contains(text, "slice bounds out of range"):
		return "index_out_of_range"
	case contains(text, "nil pointer dereference"), contains(text, "invalid memory address"):
		return "nil_pointer_dereference"
	default:
		return "other:" + text
	}
}

func contains(s, sub string) bool {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return true
		}
	}
	return false
}

func replayOrderedMap(request leafRequest) []any {
	var m *OrderedMap[string, string]
	var clone *OrderedMap[string, string]
	ordered := []any{}
	for _, a := range request.Actions {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "new":
			m = &OrderedMap[string, string]{}
		case "new_nil":
			m = nil
		case "new_size_hint":
			m = NewOrderedMapWithSizeHint[string, string](a.Size)
		case "set":
			value, panicked := guarded(func() any { m.Set(a.Key, a.Value); return nil })
			_ = value
			row["panic"] = panicked
		case "delete":
			value, panicked := guarded(func() any {
				got, ok := m.Delete(a.Key)
				return []any{got, ok}
			})
			row["result"], row["panic"] = value, panicked
		case "get":
			value, panicked := guarded(func() any {
				got, ok := m.Get(a.Key)
				return []any{got, ok}
			})
			row["result"], row["panic"] = value, panicked
		case "has":
			value, panicked := guarded(func() any { return m.Has(a.Key) })
			row["result"], row["panic"] = value, panicked
		case "size":
			value, panicked := guarded(func() any { return m.Size() })
			row["result"], row["panic"] = value, panicked
		case "keys":
			value, panicked := guarded(func() any { return keysOf(m) })
			row["result"], row["panic"] = value, panicked
		case "entries":
			value, panicked := guarded(func() any { return entries(m) })
			row["result"], row["panic"] = value, panicked
		case "clear":
			_, panicked := guarded(func() any { m.Clear(); return nil })
			row["panic"] = panicked
		case "clone":
			value, panicked := guarded(func() any {
				clone = m.Clone()
				return nil
			})
			_ = value
			row["panic"] = panicked
		case "clone_entries":
			value, panicked := guarded(func() any { return entries(clone) })
			row["result"], row["panic"] = value, panicked
		case "clone_set":
			_, panicked := guarded(func() any { clone.Set(a.Key, a.Value); return nil })
			row["panic"] = panicked
		case "early_stop_keys":
			// Break out after `stop` keys: the pinned iterator is a range-over-func,
			// so stopping early is an observable contract, not an implementation detail.
			value, panicked := guarded(func() any {
				seen := []any{}
				count := 0
				for key := range m.Keys() {
					if count >= a.Stop {
						break
					}
					seen = append(seen, key)
					count++
				}
				return seen
			})
			row["result"], row["panic"] = value, panicked
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func TestPhase1LeavesCollections(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []leafRequest `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		switch request.Subject {
		case "OrderedMap":
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": replayOrderedMap(request)}
		default:
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the collections probe"
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
