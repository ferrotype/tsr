package locale

// Access only: records what the pinned locale.Parse returns for a requested
// tag. It adds no canonicalization of its own and declines every operation it
// does not serve, so the comparator sees an explicit native row for each case
// rather than an absent one.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"testing"
)

type localeRequest struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Requested string `json:"requested"`
}

func TestPhase1PilotLocale(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []localeRequest `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Operation != "locale.selectTranslation" {
			row["result"] = "native_unavailable"
			row["reason"] = "operation is not served by the locale probe"
		} else {
			parsed, ok := Parse(request.Requested)
			row["result"] = "observed"
			row["observation"] = map[string]any{
				"requested": request.Requested,
				"ok":        ok,
				"canonical": parsed.String(),
			}
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
