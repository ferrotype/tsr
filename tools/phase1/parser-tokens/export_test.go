package parser

// Access-only witness for isKeywordOrPunctuation (utilities.go:50-52). Its
// only callers at the pin are assertion guards that pass keyword or
// punctuation constants, so no parse observes its result. This overlay
// evaluates it for every Kind from the request's `first` through KindCount
// plus `beyond_count`, so both out-of-range ends are included.
import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
)

func TestPhase1KeywordOrPunctuation(t *testing.T) {
	var input struct {
		First       int `json:"first"`
		BeyondCount int `json:"beyond_count"`
	}
	raw, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(raw, &input); err != nil {
		t.Fatal(err)
	}
	rows := []any{}
	for kind := input.First; kind <= int(ast.KindCount)+input.BeyondCount; kind++ {
		rows = append(rows, map[string]any{"kind": kind, "name": ast.Kind(kind).String(), "keyword_or_punctuation": isKeywordOrPunctuation(ast.Kind(kind))})
	}
	hash := sha256.Sum256(raw)
	out, err := json.Marshal(map[string]any{"observations": rows, "kind_count": int(ast.KindCount), "request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), append(out, '\n'), 0644); err != nil {
		t.Fatal(err)
	}
}
