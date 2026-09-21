package diagnostics

// Access only: replays diagnostic identity, category, formatting and
// localization requests against the pinned implementation and records what
// each one observed. It calls the pinned functions; it never reimplements one.
//
// It is an in-package test file because most of the surface under test is
// unexported (keyToMessage, getLocalizedMessages, Message.text). The pinned
// diagnostics_test.go is itself `package diagnostics`, so in-package has
// precedent here, but every identifier below is `phase1`-prefixed so nothing
// this file declares can collide with the 2,211 generated message variables or
// be picked up by a pinned test.
//
// Every payload that can carry arbitrary bytes travels as lowercase hex.
// encoding/json rewrites invalid UTF-8 in a Go string to U+FFFD, which would
// silently repair exactly the bytes the invalid-argument cases exist to
// observe, and would make a real difference look like a match.
//
// `fingerprint` is FNV-1a/64 over an explicit record stream, not a
// cryptographic digest: it exists so a 2,211-identity comparison does not have
// to ship 2,211 rows per side. The same four lines are implemented on the Rust
// side, so the two agree by construction; the per-chunk fingerprints localize a
// difference to about 138 identities.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/locale"
	"golang.org/x/text/language"
)

// phase1Arg is one argument for an API that takes `...any`, so StringifyArgs
// sees the Go type the case is about rather than a pre-stringified value.
type phase1Arg struct {
	Kind   string   `json:"kind"`
	Text   string   `json:"text"`
	Hex    string   `json:"hex"`
	Int    int64    `json:"int"`
	Number float64  `json:"number"`
	Bool   bool     `json:"bool"`
	List   []string `json:"list"`
}

type phase1Action struct {
	Op       string      `json:"op"`
	Key      string      `json:"key"`
	KeyHex   string      `json:"key_hex"`
	Text     string      `json:"text"`
	TextHex  string      `json:"text_hex"`
	Locale   string      `json:"locale"`
	Args     []string    `json:"args"`
	ArgsHex  []string    `json:"args_hex"`
	AnyArgs  []phase1Arg `json:"any_args"`
	Category int32       `json:"category"`
	Keys     []string    `json:"keys"`
}

// phase1Request keeps every group-specific field raw and decodes it only once
// the subject has matched. Each probe reads the whole shared schedule, and
// another group is free to give `actions`, `keys`, `category` or `chunks` a
// different JSON type; decoding those eagerly would fail the unmarshal of the
// whole document and take every case in this group down with it. `case`,
// `operation` and `subject` are the schedule's own contract and stay typed.
type phase1Request struct {
	Case      string          `json:"case"`
	Operation string          `json:"operation"`
	Subject   string          `json:"subject"`
	Actions   json.RawMessage `json:"actions"`
	Roster    json.RawMessage `json:"roster"`
	Chunks    json.RawMessage `json:"chunks"`
}

// phase1Decode reads one deferred field of a request this group has claimed. A
// failure here is a malformed request for a case the probe is answering, which
// is a harness fault: it fails loudly rather than degrading to an empty
// observation that would be indistinguishable from a real result.
func phase1Decode(t *testing.T, where string, field string, raw json.RawMessage, into any) {
	t.Helper()
	if len(raw) == 0 {
		return
	}
	if err := json.Unmarshal(raw, into); err != nil {
		t.Fatalf("case %q: %s is not the shape the diagnostics probe requires: %v",
			where, field, err)
	}
}

const (
	phase1FnvOffset uint64 = 0xcbf29ce484222325
	phase1FnvPrime  uint64 = 0x100000001b3
	phase1FieldSep         = "\x1f"
	phase1RecordSep        = "\x1e"
)

func phase1Fingerprint(data string) string {
	hash := phase1FnvOffset
	for index := range len(data) {
		hash ^= uint64(data[index])
		hash *= phase1FnvPrime
	}
	return fmt.Sprintf("%016x", hash)
}

func phase1Hex(text string) string {
	return hex.EncodeToString([]byte(text))
}

func phase1Flag(value bool) string {
	if value {
		return "1"
	}
	return "0"
}

// phase1Guarded runs f and converts a panic into a recorded observation, so a
// case whose subject is the panic boundary observes it instead of failing the
// probe. The panic text is the contract here (it names the unknown key), so it
// is recorded whole rather than bucketed.
func phase1Guarded(f func() string) (result string, panicked bool, text string) {
	defer func() {
		if recovered := recover(); recovered != nil {
			result, panicked, text = "", true, phase1PanicText(recovered)
		}
	}()
	return f(), false, ""
}

func phase1PanicText(recovered any) string {
	switch value := recovered.(type) {
	case error:
		return value.Error()
	case string:
		return value
	default:
		return fmt.Sprintf("%v", value)
	}
}

func phase1DecodeHex(t *testing.T, op string, field string, encoded string) string {
	t.Helper()
	raw, err := hex.DecodeString(encoded)
	if err != nil {
		t.Fatalf("action %q: %s is not hex (%q): %v", op, field, encoded, err)
	}
	return string(raw)
}

// phase1Text prefers text_hex, so a case about invalid bytes can state them.
func phase1Text(t *testing.T, action phase1Action) string {
	if action.TextHex != "" {
		return phase1DecodeHex(t, action.Op, "text_hex", action.TextHex)
	}
	return action.Text
}

func phase1Key(t *testing.T, action phase1Action) Key {
	if action.KeyHex != "" {
		return Key(phase1DecodeHex(t, action.Op, "key_hex", action.KeyHex))
	}
	return Key(action.Key)
}

// phase1Args returns nil when the request supplies no arguments at all, which
// is the distinction Format's `len(args) == 0` short circuit turns on.
func phase1Args(t *testing.T, action phase1Action) []string {
	if action.ArgsHex != nil {
		args := make([]string, len(action.ArgsHex))
		for index, encoded := range action.ArgsHex {
			args[index] = phase1DecodeHex(t, action.Op, "args_hex", encoded)
		}
		return args
	}
	return action.Args
}

func phase1AnyArgs(t *testing.T, action phase1Action) []any {
	if action.AnyArgs == nil {
		return nil
	}
	args := make([]any, len(action.AnyArgs))
	for index, arg := range action.AnyArgs {
		switch arg.Kind {
		case "string":
			args[index] = arg.Text
		case "string_hex":
			args[index] = phase1DecodeHex(t, action.Op, "any_args.hex", arg.Hex)
		case "int":
			args[index] = arg.Int
		case "float":
			args[index] = arg.Number
		case "bool":
			args[index] = arg.Bool
		case "null":
			args[index] = nil
		case "strings":
			args[index] = arg.List
		default:
			t.Fatalf("action %q: unknown any-arg kind %q", action.Op, arg.Kind)
		}
	}
	return args
}

// phase1Locale builds the Locale a case selects. It deliberately calls
// x/text's language.Parse rather than locale.Parse: tag parsing is the locale
// group's operation, and this group only observes what diagnostics do with an
// already selected locale. An empty string means the zero Locale, which is the
// "no locale was requested" state, not the parsed tag "und".
func phase1Locale(text string) (selected locale.Locale, tag string, parsed bool) {
	if text == "" {
		return locale.Default, language.Tag(locale.Default).String(), true
	}
	value, err := language.Parse(text)
	return locale.Locale(value), value.String(), err == nil
}

func phase1LocaleRow(row map[string]any, action phase1Action) locale.Locale {
	selected, tag, parsed := phase1Locale(action.Locale)
	row["locale"] = action.Locale
	row["tag"] = tag
	row["tag_parsed"] = parsed
	row["tag_und"] = language.Tag(selected) == language.Und
	return selected
}

func phase1IdentityRecord(key string, message *Message) string {
	if message == nil {
		return key + phase1FieldSep + "unresolved" + phase1RecordSep
	}
	flags := phase1Flag(message.ReportsUnnecessary()) +
		phase1Flag(message.ReportsDeprecated()) +
		phase1Flag(message.ElidedInCompatibilityPyramid())
	return key +
		phase1FieldSep + strconv.FormatInt(int64(message.Code()), 10) +
		phase1FieldSep + strconv.FormatInt(int64(message.Category()), 10) +
		phase1FieldSep + flags +
		phase1FieldSep + message.String() +
		phase1RecordSep
}

// phase1Roster fingerprints every identity the request names. The roster is an
// input — the list of keys to look up — and the record stream is built from
// what the pinned switch returns for each of them.
func phase1Roster(t *testing.T, request phase1Request) map[string]any {
	t.Helper()
	var roster []string
	var count int
	phase1Decode(t, request.Case, "roster", request.Roster, &roster)
	phase1Decode(t, request.Case, "chunks", request.Chunks, &count)

	records := make([]string, len(roster))
	resolved := 0
	for index, key := range roster {
		message := keyToMessage(Key(key))
		if message != nil {
			resolved++
		}
		records[index] = phase1IdentityRecord(key, message)
	}
	chunks := []any{}
	for chunk := range count {
		start := chunk * len(records) / count
		end := (chunk + 1) * len(records) / count
		chunks = append(chunks, phase1Fingerprint(strings.Join(records[start:end], "")))
	}
	return map[string]any{
		"roster_size": len(roster),
		"resolved":    resolved,
		"unresolved":  len(roster) - resolved,
		"fingerprint": phase1Fingerprint(strings.Join(records, "")),
		"chunks":      chunks,
	}
}

// phase1LookupRow records one resolved identity through the pinned accessors.
//
// The text is recorded once, through Message.String, because String is the
// pinned accessor for it and returns the unexported field verbatim
// (diagnostics.go: `func (m *Message) String() string { return m.text }`). A
// second `string_hex` field alongside `text_hex` was the same bytes for every
// message on both sides and so could never witness a difference; the Rust side
// reaches the same bytes through the public `text` field, which is the
// correspondence leaves/diagnostics/identity-byte-exact-text records.
func phase1LookupRow(row map[string]any, message *Message) {
	row["found"] = message != nil
	if message == nil {
		return
	}
	row["code"] = message.Code()
	row["category"] = int32(message.Category())
	row["reports_unnecessary"] = message.ReportsUnnecessary()
	row["reports_deprecated"] = message.ReportsDeprecated()
	row["elided_in_compatibility_pyramid"] = message.ElidedInCompatibilityPyramid()
	row["key_returned"] = string(message.Key())
	row["text_hex"] = phase1Hex(message.String())
}

func phase1ResultRow(row map[string]any, result string, panicked bool, text string) {
	row["panicked"] = panicked
	if panicked {
		row["panic_hex"] = phase1Hex(text)
		return
	}
	row["result_hex"] = phase1Hex(result)
}

// phase1Table reports one locale's selected translation table: its size, how
// many of its keys the pinned generated switch still resolves, a fingerprint of
// its whole sorted contents, and the value it holds for each probe key. Map
// iteration order is randomised in Go, so the fingerprint sorts first.
func phase1Table(row map[string]any, selected locale.Locale, probes []string) {
	table := getLocalizedMessages(language.Tag(selected))
	row["table_nil"] = table == nil
	row["size"] = len(table)
	keys := make([]string, 0, len(table))
	for key := range table {
		keys = append(keys, string(key))
	}
	slices.Sort(keys)
	resolved := 0
	var stream strings.Builder
	for _, key := range keys {
		if keyToMessage(Key(key)) != nil {
			resolved++
		}
		stream.WriteString(key)
		stream.WriteString(phase1FieldSep)
		stream.WriteString(table[Key(key)])
		stream.WriteString(phase1RecordSep)
	}
	row["resolved_keys"] = resolved
	row["unknown_keys"] = len(keys) - resolved
	row["fingerprint"] = phase1Fingerprint(stream.String())
	samples := []any{}
	for _, probe := range probes {
		value, present := table[Key(probe)]
		samples = append(samples, []any{probe, present, phase1Hex(value)})
	}
	row["samples"] = samples
}

func phase1ReplayAction(t *testing.T, action phase1Action) map[string]any {
	t.Helper()
	row := map[string]any{"op": action.Op}
	switch action.Op {
	case "lookup":
		row["key"] = action.Key
		phase1LookupRow(row, keyToMessage(Key(action.Key)))
	case "lookup_bytes":
		row["key_hex"] = action.KeyHex
		phase1LookupRow(row, keyToMessage(phase1Key(t, action)))
	case "category_name":
		row["category"] = action.Category
		name, panicked, text := phase1Guarded(func() string { return Category(action.Category).Name() })
		row["panicked"] = panicked
		if panicked {
			row["panic_hex"] = phase1Hex(text)
		} else {
			row["name"] = name
		}
	case "category_string":
		row["category"] = action.Category
		row["string"] = Category(action.Category).String()
	case "format":
		text := phase1Text(t, action)
		args := phase1Args(t, action)
		row["text_hex"] = phase1Hex(text)
		row["args_count"] = len(args)
		result, panicked, panicText := phase1Guarded(func() string { return Format(text, args) })
		phase1ResultRow(row, result, panicked, panicText)
	case "format_message":
		row["key"] = action.Key
		message := keyToMessage(Key(action.Key))
		row["found"] = message != nil
		if message != nil {
			args := phase1Args(t, action)
			row["args_count"] = len(args)
			row["text_hex"] = phase1Hex(message.text)
			result, panicked, panicText := phase1Guarded(func() string { return Format(message.text, args) })
			phase1ResultRow(row, result, panicked, panicText)
		}
	case "localize_message":
		row["key"] = action.Key
		selected := phase1LocaleRow(row, action)
		message := keyToMessage(Key(action.Key))
		row["found"] = message != nil
		if message != nil {
			args := phase1Args(t, action)
			row["args_count"] = len(args)
			_, translated := getLocalizedMessages(language.Tag(selected))[message.Key()]
			row["translated"] = translated
			result, panicked, panicText := phase1Guarded(func() string {
				return Localize(selected, message, "", args...)
			})
			phase1ResultRow(row, result, panicked, panicText)
		}
	case "localize_key":
		row["key"] = action.Key
		selected := phase1LocaleRow(row, action)
		args := phase1Args(t, action)
		row["args_count"] = len(args)
		result, panicked, panicText := phase1Guarded(func() string {
			return Localize(selected, nil, Key(action.Key), args...)
		})
		phase1ResultRow(row, result, panicked, panicText)
	case "message_localize":
		row["key"] = action.Key
		selected := phase1LocaleRow(row, action)
		message := keyToMessage(Key(action.Key))
		row["found"] = message != nil
		if message != nil {
			args := phase1AnyArgs(t, action)
			row["args_count"] = len(args)
			result, panicked, panicText := phase1Guarded(func() string {
				return message.Localize(selected, args...)
			})
			phase1ResultRow(row, result, panicked, panicText)
		}
	case "localized_table":
		selected := phase1LocaleRow(row, action)
		phase1Table(row, selected, action.Keys)
	case "stringify":
		values := StringifyArgs(phase1AnyArgs(t, action))
		row["nil_result"] = values == nil
		row["count"] = len(values)
		encoded := []any{}
		for _, value := range values {
			encoded = append(encoded, phase1Hex(value))
		}
		row["values_hex"] = encoded
	case "adhoc":
		message := NewAdHocMessage(phase1Text(t, action))
		row["code"] = message.Code()
		row["category"] = int32(message.Category())
		row["key_returned"] = string(message.Key())
		// One text field, for the reason phase1LookupRow gives: String
		// returns the text field verbatim, so a second copy of it is not a
		// second observation.
		row["text_hex"] = phase1Hex(message.String())
	case "adhoc_localize":
		selected := phase1LocaleRow(row, action)
		message := NewAdHocMessage(phase1Text(t, action))
		args := phase1Args(t, action)
		row["args_count"] = len(args)
		result, panicked, panicText := phase1Guarded(func() string {
			return Localize(selected, message, "", args...)
		})
		phase1ResultRow(row, result, panicked, panicText)
	default:
		row["unsupported_action"] = action.Op
	}
	return row
}

func phase1Replay(t *testing.T, request phase1Request) []any {
	t.Helper()
	var actions []phase1Action
	phase1Decode(t, request.Case, "actions", request.Actions, &actions)
	ordered := []any{}
	for _, action := range actions {
		ordered = append(ordered, phase1ReplayAction(t, action))
	}
	return ordered
}

func TestPhase1LeavesDiagnostics(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []phase1Request `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		switch {
		case request.Subject == "diagnostics.roster":
			row["result"] = "observed"
			row["observation"] = phase1Roster(t, request)
		case strings.HasPrefix(request.Subject, "diagnostics."):
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": phase1Replay(t, request)}
		default:
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the diagnostics probe"
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
