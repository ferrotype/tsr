// Observe actual baseline comparisons, including the pin's nonfailing deletion
// branch. stdout is consumed by test2json, never used as the expected baseline.
package baseline

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"testing"
)

var tsrReportMu sync.Mutex

func tsrLocalRoot(fallback string) string {
	if path := os.Getenv("TSR_BASELINE_LOCAL"); path != "" {
		return path
	}
	return fallback
}
func tsrReportBaseline(t *testing.T, name, folder, actual, local, reference string, writeError *error) func() {
	expected := NoContent
	found := false
	read, err := os.ReadFile(reference)
	if err == nil {
		expected = string(read)
		found = true
	}
	return func() {
		state, reason := "pass", ""
		switch {
		case *writeError != nil:
			state, reason = "fail", "baseline write failed: "+(*writeError).Error()
		case actual == "":
			state, reason = "fail", "empty baseline content"
		case err != nil && !os.IsNotExist(err):
			state, reason = "fail", "cannot read reference: "+err.Error()
		case actual == expected && !(actual == NoContent && found):
		case actual == NoContent:
			state, reason = "fail", "deleted baseline"
		case !found:
			state, reason = "fail", "new baseline"
		default:
			state, reason = "fail", "baseline bytes differ"
		}
		row := map[string]string{"test": t.Name(), "path": filepath.ToSlash(filepath.Join(folder, name)), "state": state, "reason": reason}
		data, marshalErr := json.Marshal(row)
		if marshalErr != nil {
			panic(marshalErr)
		}
		tsrReportMu.Lock()
		defer tsrReportMu.Unlock()
		fmt.Printf("TSR_BASELINE %s\n", data)
	}
}
