package locale_test

// Access only: the locale group's second probe. It exists for one reason: the
// localisation cache in internal/diagnostics is a process-global `sync.Map`
// (diagnostics.go:87) and `locale.Default` is a process-global var
// (locale.go:13), so a process that has already answered the main schedule can
// no longer observe either from cold.
//
// The harness runs one `go test` invocation per probe entry, so this file's
// test function gets a process of its own. It serves exactly one case, the one
// whose subject is `locale.default`, and declines every other request. Because
// it declines them without calling into diagnostics, its first `Localize` is
// genuinely the first in the process no matter where that case sits in the
// schedule.
//
// The two probes never compile together: each capture run overlays exactly one
// file at the same virtual path. The small amount of shared shape below is
// therefore duplicated rather than factored out, which is also why this file
// adds no helper the pinned package could pick up.
//
// What this case establishes, and the main probe's warm trace cannot: the very
// first localisation of a tag returns the same bytes as a later repeat, so the
// cache is transparent, and the default locale of a fresh process is the `und`
// sentinel whose `Locale.String()` is "" while its underlying tag prints "und".

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"runtime"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/diagnostics"
	"github.com/microsoft/TypeScript/tsc/internal/locale"
	"golang.org/x/text/language"
)

type defaultAction struct {
	Op   string   `json:"op"`
	Tag  string   `json:"tag"`
	Key  string   `json:"key"`
	Args []string `json:"args"`
}

// decodeDefaultAction unmarshals a request's actions once its subject has matched.
func decodeDefaultAction(raw json.RawMessage) []defaultAction {
	if len(raw) == 0 {
		return nil
	}
	var out []defaultAction
	if err := json.Unmarshal(raw, &out); err != nil {
		return nil
	}
	return out
}

type defaultRequest struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared schedule and another
	// group's actions need not fit this group's action type.
	Actions json.RawMessage `json:"actions"`
}

func guardedDefault(f func() string) (result string, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			result = ""
			switch value := r.(type) {
			case error:
				panicked = "error:" + value.Error()
			case string:
				panicked = "string:" + value
			default:
				panicked = "non-error panic"
			}
		}
	}()
	return f(), ""
}

func replayDefault(request defaultRequest) []any {
	ctx := context.Background()
	ordered := make([]any, 0, len(decodeDefaultAction(request.Actions)))
	for _, a := range decodeDefaultAction(request.Actions) {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "default_read":
			// The pinned package never assigns Default; every caller reads it as
			// "no locale requested" (for example
			// tsc/internal/project/session.go:203).
			row["locale_string"] = locale.Default.String()
			row["tag_string"] = language.Tag(locale.Default).String()
			row["is_zero_tag"] = locale.Default == locale.Locale(language.Tag{})
		case "ctx_read":
			current := locale.FromContext(ctx)
			row["has_locale"] = locale.HasLocale(ctx)
			row["locale_string"] = current.String()
			row["tag_string"] = language.Tag(current).String()
			row["is_default"] = current == locale.Default
		case "localize":
			parsed, ok := locale.Parse(a.Tag)
			text, panicked := guardedDefault(func() string {
				return diagnostics.Localize(parsed, nil, diagnostics.Key(a.Key), a.Args...)
			})
			row["input"] = a.Tag
			row["parse_ok"] = ok
			row["locale_string"] = parsed.String()
			row["key"] = a.Key
			row["text"] = text
			row["panic"] = panicked
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered
}

func TestPhase1LeavesLocaleDefault(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []defaultRequest `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		if request.Subject == "locale.default" {
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": replayDefault(request)}
		} else {
			row["result"] = "native_unavailable"
			row["reason"] = "the locale-default probe serves only the cold-process default case; " +
				"answering anything else would warm the localisation cache first"
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
