package json

// Access only: marshals an ordered member list through the pinned json.Marshal
// and records the exact output bytes. Member order is the property under test,
// so the request carries ordered entry pairs and the probe builds a
// jsontext.Value from them rather than a Go map, which would not preserve order.

import (
	"crypto/sha256"
	"encoding/hex"
	stdjson "encoding/json"
	"os"
	"runtime"
	"testing"

	"github.com/go-json-experiment/json/jsontext"
)

type jsonRequest struct {
	Case      string                  `json:"case"`
	Operation string                  `json:"operation"`
	Ordered   [][2]stdjson.RawMessage `json:"ordered"`
}

func TestPhase1PilotJson(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []jsonRequest `json:"requests"`
	}
	if err := stdjson.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Operation != "json.marshalOrdered" {
			row["result"] = "native_unavailable"
			row["reason"] = "operation is not served by the json probe"
			observations = append(observations, row)
			continue
		}
		// Build the object as raw tokens so the declared order survives.
		value := jsontext.Value("{")
		for i, pair := range request.Ordered {
			if i > 0 {
				value = append(value, ',')
			}
			value = append(value, pair[0]...)
			value = append(value, ':')
			value = append(value, pair[1]...)
		}
		value = append(value, '}')
		marshaled, marshalErr := Marshal(value)
		if marshalErr != nil {
			row["result"] = "observed"
			row["observation"] = map[string]any{"error": marshalErr.Error()}
		} else {
			row["result"] = "observed"
			row["observation"] = map[string]any{"bytes": string(marshaled)}
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
