package astnav

// Access-only witness for the otherwise unproven JSX rescan branch. Ordinary
// parsed JSX/shift sources do not establish the scanner's containing-node kind.
import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/scanner"
	"os"
	"runtime"
	"testing"
)

func TestPhase1NavigationRescan(t *testing.T) {
	var input struct {
		Requests []struct {
			Case, Text string
			JSX        bool
		} `json:"requests"`
	}
	raw, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(raw, &input); err != nil {
		t.Fatal(err)
	}
	rows := []any{}
	for _, request := range input.Requests {
		s := scanner.NewScanner()
		s.SetText(request.Text)
		s.Scan()
		kind := ast.KindSourceFile
		if request.JSX {
			kind = ast.KindJsxExpression
		}
		parent := &ast.Node{Kind: kind}
		before := s.Token()
		rescan := shouldRescanLessThanLessThanToken(s, parent, before)
		after := scanNavigationToken(s, parent)
		rows = append(rows, map[string]any{"case": request.Case, "before": int(before), "rescan": rescan, "after": int(after), "start": s.TokenStart(), "end": s.TokenEnd(), "flags": int(s.TokenFlags())})
	}
	hash := sha256.Sum256(raw)
	out, err := json.Marshal(map[string]any{"observations": rows, "request_sha256": hex.EncodeToString(hash[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), append(out, '\n'), 0644); err != nil {
		t.Fatal(err)
	}
}
