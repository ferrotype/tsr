package locale_test

// Access only: replays a locale request schedule against the pinned
// internal/locale package and against the pinned localisation selection in
// internal/diagnostics, recording what each action observed. It adds no
// canonicalisation, no matcher and no table of its own, and it declines every
// request it does not serve so the comparator sees an explicit native row for
// each case rather than an absent one.
//
// This is an *external* test package (`locale_test`, not `locale`). The
// translation tables and the `language.NewMatcher` that selects them live in
// internal/diagnostics, which imports internal/locale; an in-package test file
// importing diagnostics would be an import cycle. An external test package may
// import it, and doing so is what lets the probe observe the real pinned
// matcher and the real embedded tables instead of a reconstruction. The
// directory has no other test file at the pin, so the choice of package
// collides with nothing.
//
// Two observations are recorded for every parsed tag, because the pinned code
// distinguishes them and a port can collapse them:
//
//   - `locale_string` is `Locale.String()`, which returns "" for the `Default`
//     sentinel (locale.go:15-20), and
//   - `tag_string` is the underlying `language.Tag.String()`, which returns
//     "und" for that same value.
//
// `parse_error` / `error_subtag` come from calling `language.Parse` directly on
// the same input. `locale.Parse` discards the error, so this is the only way to
// record *why* `ok` is false, which is the discriminating input for choosing a
// Rust BCP 47 dependency in F1b. It is an observation of the pinned dependency
// at the pinned version, never a claim about it.
//
// The trace shape is the contract the Rust driver answers: one ordered result
// per action, under `ordered`, so sequence survives canonicalisation.

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

type action struct {
	Op   string   `json:"op"`
	Tag  string   `json:"tag"`
	Key  string   `json:"key"`
	Args []string `json:"args"`
}

// decodeAction unmarshals a request's actions once its subject has matched.
func decodeAction(raw json.RawMessage) []action {
	if len(raw) == 0 {
		return nil
	}
	var out []action
	if err := json.Unmarshal(raw, &out); err != nil {
		return nil
	}
	return out
}

type leafRequest struct {
	Case      string `json:"case"`
	Operation string `json:"operation"`
	Subject   string `json:"subject"`
	// Raw, because every probe decodes the whole shared schedule and another
	// group's actions need not fit this group's action type.
	Actions json.RawMessage `json:"actions"`
}

// The exported message vars this schedule formats. `Localize` resolves a key
// through the unexported keyToMessage, but `Message.Localize` needs the value
// itself, so the pairing is written out and checked against `Message.Key()`
// before any row is emitted.
var messagesByKey = map[string]*diagnostics.Message{
	"Modules_6244": diagnostics.Modules,
	"Abstract_method_0_in_class_1_cannot_be_accessed_via_super_expression_2513":                          diagnostics.Abstract_method_0_in_class_1_cannot_be_accessed_via_super_expression,
	"using_declarations_are_not_allowed_in_case_or_default_clauses_unless_contained_within_a_block_1547": diagnostics.X_using_declarations_are_not_allowed_in_case_or_default_clauses_unless_contained_within_a_block,
}

// guarded runs f and converts a panic into a recorded class, so a case whose
// subject is the panic boundary observes it instead of failing the probe.
func guarded(f func() string) (result string, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			result = ""
			panicked = classify(r)
		}
	}()
	return f(), ""
}

func classify(r any) string {
	switch value := r.(type) {
	case error:
		return "error:" + value.Error()
	case string:
		return "string:" + value
	default:
		return "non-error panic"
	}
}

// parseFailure reports the error `locale.Parse` throws away. A well-formed tag
// carrying an unrecognised subtag yields a language.ValueError and a *stripped*
// tag, which is a different outcome from a syntax error even though both leave
// `ok` false.
func parseFailure(text string) (message string, subtag string) {
	_, err := language.Parse(text)
	if err == nil {
		return "", ""
	}
	if value, ok := err.(language.ValueError); ok {
		return err.Error(), value.Subtag()
	}
	return err.Error(), ""
}

func replay(request leafRequest) ([]any, string) {
	ctx := context.Background()
	ordered := make([]any, 0, len(decodeAction(request.Actions)))
	for _, a := range decodeAction(request.Actions) {
		row := map[string]any{"op": a.Op}
		switch a.Op {
		case "parse":
			parsed, ok := locale.Parse(a.Tag)
			failure, subtag := parseFailure(a.Tag)
			row["input"] = a.Tag
			row["ok"] = ok
			row["locale_string"] = parsed.String()
			row["tag_string"] = language.Tag(parsed).String()
			row["is_default"] = parsed == locale.Default
			row["parse_error"] = failure
			row["error_subtag"] = subtag
		case "ctx_root":
			ctx = context.Background()
		case "ctx_with":
			// Production callers discard `ok` and keep the returned tag; see
			// tsc/internal/tsoptions/parsedcommandline.go:491 and
			// tsc/internal/execute/tsc/compile.go:93. The probe does the same,
			// so a failed parse is observed exactly as the compiler stores it.
			parsed, ok := locale.Parse(a.Tag)
			ctx = locale.WithLocale(ctx, parsed)
			row["input"] = a.Tag
			row["parse_ok"] = ok
			row["stored_locale_string"] = parsed.String()
			row["stored_tag_string"] = language.Tag(parsed).String()
		case "ctx_with_default":
			ctx = locale.WithLocale(ctx, locale.Default)
		case "ctx_read":
			current := locale.FromContext(ctx)
			row["has_locale"] = locale.HasLocale(ctx)
			row["locale_string"] = current.String()
			row["tag_string"] = language.Tag(current).String()
			row["is_default"] = current == locale.Default
		case "localize":
			parsed, ok := locale.Parse(a.Tag)
			text, panicked := guarded(func() string {
				return diagnostics.Localize(parsed, nil, diagnostics.Key(a.Key), a.Args...)
			})
			row["input"] = a.Tag
			row["parse_ok"] = ok
			row["locale_string"] = parsed.String()
			row["key"] = a.Key
			row["text"] = text
			row["panic"] = panicked
		case "localize_message":
			message, known := messagesByKey[a.Key]
			if !known {
				return nil, "the probe has no exported Message var for key " + a.Key
			}
			args := make([]any, len(a.Args))
			for i, value := range a.Args {
				args[i] = value
			}
			parsed, ok := locale.Parse(a.Tag)
			text, panicked := guarded(func() string {
				return message.Localize(parsed, args...)
			})
			row["input"] = a.Tag
			row["parse_ok"] = ok
			row["locale_string"] = parsed.String()
			row["key"] = a.Key
			row["code"] = message.Code()
			row["text"] = text
			row["panic"] = panicked
		default:
			row["unsupported_action"] = a.Op
		}
		ordered = append(ordered, row)
	}
	return ordered, ""
}

// served names the subjects this probe answers. `locale.default` is served by
// the sibling probe, which needs a process in which nothing has been localized
// yet; answering it here would observe an already-warm localisation cache.
var served = map[string]bool{
	"locale.parse":             true,
	"locale.string":            true,
	"locale.context":           true,
	"locale.translation":       true,
	"locale.translation-args":  true,
	"locale.translation-cache": true,
}

func TestPhase1LeavesLocale(t *testing.T) {
	for key, message := range messagesByKey {
		if string(message.Key()) != key {
			t.Fatalf("message table pairs key %q with message %q", key, message.Key())
		}
	}

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
		switch {
		case served[request.Subject]:
			rows, problem := replay(request)
			if problem != "" {
				row["result"] = "harness_failed"
				row["error"] = problem
			} else {
				row["result"] = "observed"
				row["observation"] = map[string]any{"ordered": rows}
			}
		case request.Subject == "locale.default":
			row["result"] = "native_unavailable"
			row["reason"] = "served by the sibling locale-default probe, which needs a process " +
				"in which no localisation has been requested yet"
		default:
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the locale probe"
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
