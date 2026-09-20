package json

// Access only: replays an ordered action trace against the pinned
// `internal/json` wrapper and records exactly what each action observed.
//
// It is an in-package test file because the subject is the wrapper itself:
// `Marshal`, `Unmarshal` and friends prepend their own options before calling
// the go-json-experiment library, and that prepending is the behavior under
// test. The pinned directory holds no test file at all, so this file collides
// with nothing and deliberately adds no helper the pinned tree could pick up.
//
// Three rules shape the payloads:
//
//   - Every byte string travels as hex. The probe writes its report with
//     encoding/json v1, which silently rewrites invalid UTF-8 as U+FFFD; a
//     case whose whole subject is an invalid byte would then witness nothing.
//     `*_text` is added only when the bytes are valid UTF-8, for readability.
//     An error message is text rather than a byte string and is recorded as
//     text, with the same hex escape hatch if a message ever carries a byte
//     v1 would rewrite. See `putErrorText`.
//   - Every case is an ordered trace and its rows are flat. The comparison
//     canonicalises with sorted keys, so JSON member order survives only
//     inside a list, and a multi-key object nested in a row is refused by the
//     harness for the same reason.
//   - Every error message goes through `stableErrorText`. The library
//     deliberately randomises one word of `*json.SemanticError`'s rendering
//     once per process, and observing it would make the capture flake between
//     otherwise identical runs. See that function for the detail.
//
// Options are only ever built from the wrapper's own exported constructors
// (`AllowDuplicateNames`, `Deterministic`, `WithIndent`), because those three
// are the whole option surface a caller outside this package can reach.

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	stdjson "encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"runtime"
	"strconv"
	"strings"
	"testing"
	"unicode/utf8"

	experiment "github.com/go-json-experiment/json"
	"github.com/go-json-experiment/json/jsontext"
)

// sampleFields is the declared target for the missing/null/empty cases. Its
// four fields cover the distinctions a JSON reader can collapse: an absent
// member, an explicit null, an empty container and a zero scalar.
type sampleFields struct {
	Name  string   `json:"name"`
	Count *int     `json:"count"`
	Items []string `json:"items"`
	Extra string   `json:"extra,omitzero"`
}

type optionSpec struct {
	Option  string `json:"option"`
	Enabled bool   `json:"enabled"`
	Indent  string `json:"indent"`
}

type valueSpec struct {
	Kind    string      `json:"kind"`
	Text    string      `json:"text"`
	Entries [][2]string `json:"entries"`
}

// jsonAction is one step of a trace. The empty fields fall back to the
// request-level defaults, so a single-step case declares its input once.
type jsonAction struct {
	Op        string `json:"op"`
	InputHex  string `json:"input_hex"`
	ValueKind string `json:"value_kind"`
	ValueText string `json:"value_text"`
	Target    string `json:"target"`
	Token     string `json:"token"`
	Prefix    string `json:"prefix"`
	Indent    string `json:"indent"`
	Chunk     int    `json:"chunk"`
	// Preset fills a `new_target` destination before it is read into. It has
	// no request-level fallback: a preset applies to the one action that asks
	// for it.
	Preset string `json:"preset"`
	// Reuse reads into the destination the last `new_target` established
	// instead of a fresh zero one.
	Reuse bool `json:"reuse"`
}

// decodeActions unmarshals a request's actions once its subject has matched.
//
// It reports the decode error rather than returning an empty trace: a request
// this probe cannot express has to reach the report as a harness failure and
// invalidate the capture, not pass as an observation of nothing.
func decodeActions(raw stdjson.RawMessage) ([]jsonAction, error) {
	if len(raw) == 0 {
		return nil, nil
	}
	var out []jsonAction
	if err := stdjson.Unmarshal(raw, &out); err != nil {
		return nil, fmt.Errorf("cannot decode actions: %w", err)
	}
	return out, nil
}

type leafJSONRequest struct {
	Case      string       `json:"case"`
	Operation string       `json:"operation"`
	Subject   string       `json:"subject"`
	InputHex  string       `json:"input_hex"`
	Value     valueSpec    `json:"value"`
	Options   []optionSpec `json:"options"`
	Target    string       `json:"target"`
	// Raw, because every probe decodes the whole shared schedule and another
	// group's actions need not fit this group's action type.
	Actions stdjson.RawMessage `json:"actions"`
}

// subjects the probe serves. Every one of them names a wrapper entry point
// whose Rust home does not exist, so the request schedule can keep them apart.
var servedSubjects = map[string]bool{
	"json.Marshal":            true,
	"json.MarshalIndent":      true,
	"json.MarshalWrite":       true,
	"json.MarshalIndentWrite": true,
	"json.MarshalEncode":      true,
	"json.Unmarshal":          true,
	"json.UnmarshalRead":      true,
	"json.NewDecoder":         true,
	"json.UnmarshalDecode":    true,
}

// chunkReader hands out at most `chunk` bytes per Read, so a streaming case
// can put a read boundary inside a token instead of between values.
type chunkReader struct {
	data  []byte
	chunk int
}

func (r *chunkReader) Read(p []byte) (int, error) {
	if len(r.data) == 0 {
		return 0, io.EOF
	}
	n := len(p)
	if r.chunk > 0 && n > r.chunk {
		n = r.chunk
	}
	if n > len(r.data) {
		n = len(r.data)
	}
	copy(p, r.data[:n])
	r.data = r.data[n:]
	return n, nil
}

// putBytes records a byte string losslessly, plus a readable rendering when
// the bytes happen to be valid UTF-8.
func putBytes(row map[string]any, prefix string, data []byte) {
	row[prefix+"_hex"] = hex.EncodeToString(data)
	row[prefix+"_len"] = len(data)
	if utf8.Valid(data) {
		row[prefix+"_text"] = string(data)
	}
}

// stableErrorText removes the one deliberately unstable part of a library
// error message.
//
// `*json.SemanticError.Error` opens with `errorModalVerb()`, which the library
// picks once per process from Go's randomised map iteration to Hyrum-proof the
// message: the same error renders as "json: cannot marshal ..." in one run and
// "json: unable to marshal ..." in the next. The choice is not a behavior, and
// observing it would make every capture that touches a marshal or unmarshal
// type error irreproducible, so the verb is normalised to the one spelling and
// the raw choice is never recorded. Nothing else in either error type is
// randomised: `*jsontext.SyntacticError.Error` is fully determined by its
// fields.
func stableErrorText(err error) string {
	const randomized = "json: unable to "
	text := err.Error()
	if rest, found := strings.CutPrefix(text, randomized); found {
		return "json: cannot " + rest
	}
	return text
}

// putErrorText records a message losslessly. A library message is normally
// valid UTF-8 and is recorded as text; the hex form appears instead when it is
// not, because the report is written with encoding/json v1, which would rewrite
// an invalid byte to U+FFFD and lose exactly the byte a message quoting the
// offending input exists to show.
func putErrorText(row map[string]any, key, text string) {
	if utf8.ValidString(text) {
		row[key] = text
		return
	}
	row[key+"_hex"] = hex.EncodeToString([]byte(text))
}

// putError flattens an error onto the row. Flat, because a nested multi-key
// object inside an ordered payload is refused by the harness.
func putError(row map[string]any, err error) {
	if err == nil {
		row["ok"] = true
		return
	}
	row["ok"] = false
	putErrorText(row, "error", stableErrorText(err))
	// Truncation is a property of the wrapped sentinel, not of the class: every
	// jsontext error raised by an input that stops mid-value is a
	// *jsontext.SyntacticError, the same class a genuine syntax error carries,
	// so `error_class` alone cannot tell "stopped early" from "invalid
	// character". This flag is the distinction, recorded on every failing row.
	row["error_unexpected_eof"] = errors.Is(err, io.ErrUnexpectedEOF)
	var semantic *experiment.SemanticError
	var syntactic *jsontext.SyntacticError
	switch {
	case errors.As(err, &semantic):
		row["error_class"] = "SemanticError"
		row["error_byte_offset"] = semantic.ByteOffset
		row["error_json_pointer"] = string(semantic.JSONPointer)
		row["error_json_kind"] = semantic.JSONKind.String()
		if semantic.GoType != nil {
			row["error_go_type"] = semantic.GoType.String()
		}
		if semantic.JSONValue != nil {
			row["error_json_value_hex"] = hex.EncodeToString(semantic.JSONValue)
		}
		// The wrapped cause carries the detail ("value out of range",
		// "unsupported value: NaN") without the randomised preamble.
		if cause := semantic.Unwrap(); cause != nil {
			putErrorText(row, "error_cause", cause.Error())
			// A marshal failure a jsontext error caused carries two positions
			// and they are not the same one. The SemanticError's own
			// ByteOffset/JSONPointer describe the *encoder*: where the value
			// would have gone in the output stream. The cause's describe where
			// in the value being written the fault was found, in the same
			// stream coordinates -- so for Marshal, whose encoder starts at
			// offset zero, the cause offset is the offset inside the marshalled
			// value. Recording only the outer pair would witness position 0 for
			// every failure of a value written from the start of a stream.
			var causeSyntactic *jsontext.SyntacticError
			if errors.As(cause, &causeSyntactic) {
				row["error_cause_byte_offset"] = causeSyntactic.ByteOffset
				row["error_cause_json_pointer"] = string(causeSyntactic.JSONPointer)
			}
		}
	case errors.As(err, &syntactic):
		row["error_class"] = "SyntacticError"
		row["error_byte_offset"] = syntactic.ByteOffset
		row["error_json_pointer"] = string(syntactic.JSONPointer)
	// A bare sentinel, for an error that reached here without a library type
	// around it. Every jsontext truncation arrives as a SyntacticError and is
	// caught above, which is why `error_unexpected_eof` carries that
	// distinction instead of this class.
	case errors.Is(err, io.ErrUnexpectedEOF):
		row["error_class"] = "ErrUnexpectedEOF"
	case errors.Is(err, io.EOF):
		row["error_class"] = "EOF"
	default:
		row["error_class"] = fmt.Sprintf("%T", err)
	}
}

// guarded converts a panic into a recorded class. Token.String panics when the
// decoder has already advanced past the token, so a trace that mis-orders its
// own steps must record that rather than take the probe down.
func guarded(row map[string]any, f func()) {
	defer func() {
		if r := recover(); r != nil {
			row["panic"] = fmt.Sprintf("%v", r)
		}
	}()
	f()
}

func hexBytes(text string) ([]byte, error) {
	if text == "" {
		return nil, nil
	}
	return hex.DecodeString(text)
}

// buildOptions turns the declared option list into wrapper options, in the
// order the request gave them: the wrapper appends the caller's options after
// its own, and a later option of the same name wins.
func buildOptions(specs []optionSpec) ([]experiment.Options, error) {
	out := make([]experiment.Options, 0, len(specs))
	for _, spec := range specs {
		switch spec.Option {
		case "AllowDuplicateNames":
			out = append(out, AllowDuplicateNames(spec.Enabled))
		case "Deterministic":
			out = append(out, Deterministic(spec.Enabled))
		case "WithIndent":
			out = append(out, WithIndent(spec.Indent))
		default:
			return nil, fmt.Errorf("unknown option %q", spec.Option)
		}
	}
	return out, nil
}

// buildValue produces the Go value a marshal action hands the wrapper. Each
// kind is a shape a real caller marshals: a raw JSON value, a Go string that
// may hold arbitrary bytes, a float, a string map and a struct.
func buildValue(kind, text, inputHex string, entries [][2]string) (any, error) {
	switch kind {
	case "raw":
		raw, err := hexBytes(inputHex)
		if err != nil {
			return nil, err
		}
		return Value(raw), nil
	case "string":
		raw, err := hexBytes(inputHex)
		if err != nil {
			return nil, err
		}
		return string(raw), nil
	case "float":
		number, err := strconv.ParseFloat(text, 64)
		if err != nil {
			return nil, err
		}
		return number, nil
	case "map":
		out := make(map[string]string, len(entries))
		for _, entry := range entries {
			out[entry[0]] = entry[1]
		}
		return out, nil
	case "sample":
		return sampleValue(text)
	default:
		return nil, fmt.Errorf("unknown value kind %q", kind)
	}
}

// sampleValue returns one of a few declared struct literals, so a marshal case
// can ask for the zero struct, a populated one, or one with an empty slice.
//
// An unknown shape is an error rather than the zero struct: a request with a
// mistyped shape has to reach the report as a harness failure and invalidate
// the capture, not pass as a clean observation of a value nobody asked for.
func sampleValue(shape string) (sampleFields, error) {
	count := 7
	switch shape {
	case "", "zero":
		return sampleFields{}, nil
	case "populated":
		return sampleFields{Name: "a", Count: &count, Items: []string{"x", "y"}, Extra: "e"}, nil
	case "empty_slice":
		return sampleFields{Items: []string{}}, nil
	default:
		return sampleFields{}, fmt.Errorf("unknown sample shape %q", shape)
	}
}

// newTarget returns a pointer for an unmarshal action plus a renderer that
// flattens the decoded value onto the row.
//
// `preset` fills the destination before the read, which is the only way to
// witness what a read leaves alone: into a fresh zero destination an absent
// member, an explicit null and an unknown member all produce the same state,
// so "left untouched" and "cleared" are indistinguishable unless something was
// there first. Only the struct target takes one; a preset on any other kind is
// a request the probe cannot express.
func newTarget(kind, preset string) (any, func(map[string]any), error) {
	if preset != "" && kind != "sample" {
		return nil, nil, fmt.Errorf("target %q takes no preset (%q)", kind, preset)
	}
	switch kind {
	case "raw_value":
		var out Value
		return &out, func(row map[string]any) {
			putBytes(row, "target", out)
			row["target_kind"] = out.Kind().String()
		}, nil
	case "float64":
		var out float64
		return &out, func(row map[string]any) {
			row["target_float"] = strconv.FormatFloat(out, 'g', -1, 64)
			// The bits separate -0 from 0, which the text rendering does not.
			row["target_float_bits"] = math.Float64bits(out)
		}, nil
	case "int64":
		var out int64
		return &out, func(row map[string]any) { row["target_int"] = out }, nil
	case "uint64":
		var out uint64
		return &out, func(row map[string]any) { row["target_uint"] = out }, nil
	case "sample":
		out, presetErr := sampleValue(preset)
		if presetErr != nil {
			return nil, nil, presetErr
		}
		return &out, func(row map[string]any) {
			row["target_name"] = out.Name
			if out.Count == nil {
				row["target_count_nil"] = true
			} else {
				row["target_count_nil"] = false
				row["target_count"] = *out.Count
			}
			row["target_items_nil"] = out.Items == nil
			row["target_items_len"] = len(out.Items)
			row["target_extra"] = out.Extra
			// The round trip shows what a re-serialising consumer would emit;
			// it cannot show nil versus empty, which is why both are recorded.
			encoded, err := Marshal(out)
			if err != nil {
				row["target_render_error"] = stableErrorText(err)
				return
			}
			putBytes(row, "target_render", encoded)
		}, nil
	default:
		return nil, nil, fmt.Errorf("unknown target %q", kind)
	}
}

func tokenNamed(name string) (jsontext.Token, error) {
	switch name {
	case "begin_object":
		return BeginObject, nil
	case "end_object":
		return EndObject, nil
	case "begin_array":
		return BeginArray, nil
	case "end_array":
		return EndArray, nil
	case "null":
		return Null, nil
	default:
		return jsontext.Token{}, fmt.Errorf("unknown token %q", name)
	}
}

// replay runs one request's ordered actions. The state it carries between
// actions -- the write buffer, the encoder and the decoder -- is what makes
// the streaming boundary cases observable at all.
func replay(request leafJSONRequest) ([]any, error) {
	options, err := buildOptions(request.Options)
	if err != nil {
		return nil, err
	}
	actions, err := decodeActions(request.Actions)
	if err != nil {
		return nil, err
	}
	var buffer bytes.Buffer
	var encoder *Encoder
	var decoder *Decoder
	// The destination a `reuse` action reads into, and the kind it was built
	// for. It survives between actions, which is what makes "the read left
	// this field alone" observable at all.
	var held any
	var heldRender func(map[string]any)
	var heldKind string
	ordered := make([]any, 0, len(actions))

	for _, action := range actions {
		row := map[string]any{"op": action.Op}
		inputHex := action.InputHex
		if inputHex == "" {
			inputHex = request.InputHex
		}
		valueKind := action.ValueKind
		if valueKind == "" {
			valueKind = request.Value.Kind
		}
		valueText := action.ValueText
		if valueText == "" {
			valueText = request.Value.Text
		}
		target := action.Target
		if target == "" {
			target = request.Target
		}

		switch action.Op {
		case "marshal", "marshal_write", "marshal_indent", "marshal_indent_write", "marshal_encode":
			input, buildErr := buildValue(valueKind, valueText, inputHex, request.Value.Entries)
			if buildErr != nil {
				return nil, buildErr
			}
			row["value_kind"] = valueKind
			switch action.Op {
			case "marshal":
				out, marshalErr := Marshal(input, options...)
				putError(row, marshalErr)
				putBytes(row, "output", out)
			case "marshal_indent":
				row["prefix"] = action.Prefix
				row["indent"] = action.Indent
				out, marshalErr := MarshalIndent(input, action.Prefix, action.Indent)
				putError(row, marshalErr)
				putBytes(row, "output", out)
			case "marshal_write":
				before := buffer.Len()
				marshalErr := MarshalWrite(&buffer, input, options...)
				putError(row, marshalErr)
				putBytes(row, "output", buffer.Bytes()[before:])
			case "marshal_indent_write":
				row["prefix"] = action.Prefix
				row["indent"] = action.Indent
				before := buffer.Len()
				marshalErr := MarshalIndentWrite(&buffer, input, action.Prefix, action.Indent)
				putError(row, marshalErr)
				putBytes(row, "output", buffer.Bytes()[before:])
			case "marshal_encode":
				if encoder == nil {
					return nil, errors.New("marshal_encode before new_encoder")
				}
				before := buffer.Len()
				marshalErr := MarshalEncode(encoder, input, options...)
				putError(row, marshalErr)
				putBytes(row, "output", buffer.Bytes()[before:])
			}

		case "new_encoder":
			buffer.Reset()
			// No options: a caller outside this package cannot reach the
			// encoder constructor, so the wrapper's own prepended option is
			// the only one in play and this is what it joins onto.
			encoder = jsontext.NewEncoder(&buffer)
			row["ok"] = true

		case "encoder_write_token":
			if encoder == nil {
				return nil, errors.New("encoder_write_token before new_encoder")
			}
			token, tokenErr := tokenNamed(action.Token)
			if tokenErr != nil {
				return nil, tokenErr
			}
			row["token"] = action.Token
			putError(row, encoder.WriteToken(token))

		case "encoder_offset":
			if encoder == nil {
				return nil, errors.New("encoder_offset before new_encoder")
			}
			row["output_offset"] = encoder.OutputOffset()
			row["stack_depth"] = encoder.StackDepth()

		case "written_bytes":
			putBytes(row, "output", buffer.Bytes())

		case "new_target":
			pointer, render, targetErr := newTarget(target, action.Preset)
			if targetErr != nil {
				return nil, targetErr
			}
			held, heldRender, heldKind = pointer, render, target
			row["target"] = target
			row["preset"] = action.Preset
			row["ok"] = true
			// The starting state is recorded too: a later row reading
			// "untouched" means nothing without the state it did not touch.
			render(row)

		case "unmarshal", "unmarshal_read":
			input, decodeErr := hexBytes(inputHex)
			if decodeErr != nil {
				return nil, decodeErr
			}
			pointer, render := held, heldRender
			if action.Reuse {
				if held == nil {
					return nil, errors.New("reuse before new_target")
				}
				if heldKind != target {
					return nil, fmt.Errorf("held target is %q, not %q", heldKind, target)
				}
			} else {
				var targetErr error
				pointer, render, targetErr = newTarget(target, "")
				if targetErr != nil {
					return nil, targetErr
				}
			}
			row["target"] = target
			row["reuse"] = action.Reuse
			putBytes(row, "input", input)
			if action.Op == "unmarshal" {
				putError(row, Unmarshal(input, pointer, options...))
			} else {
				row["chunk"] = action.Chunk
				reader := &chunkReader{data: input, chunk: action.Chunk}
				putError(row, UnmarshalRead(reader, pointer, options...))
				row["unread_len"] = len(reader.data)
			}
			render(row)

		case "new_decoder":
			input, decodeErr := hexBytes(inputHex)
			if decodeErr != nil {
				return nil, decodeErr
			}
			row["chunk"] = action.Chunk
			putBytes(row, "input", input)
			decoder = NewDecoder(&chunkReader{data: input, chunk: action.Chunk})
			row["ok"] = true

		case "peek_kind", "read_token", "read_value", "skip_value", "input_offset",
			"stack_depth", "stack_pointer", "unread_buffer", "unmarshal_decode":
			if decoder == nil {
				return nil, errors.New("decoder action before new_decoder")
			}
			if decoderErr := replayDecoder(row, decoder, action, target); decoderErr != nil {
				return nil, decoderErr
			}

		default:
			return nil, fmt.Errorf("unknown action %q", action.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered, nil
}

func replayDecoder(row map[string]any, decoder *Decoder, action jsonAction, target string) error {
	switch action.Op {
	case "peek_kind":
		kind := decoder.PeekKind()
		row["kind"] = kind.String()
	case "read_token":
		token, err := decoder.ReadToken()
		putError(row, err)
		if err == nil {
			// Token.String panics once the decoder advances, so both readings
			// are taken here, before any other action runs.
			row["token_kind"] = token.Kind().String()
			guarded(row, func() { putBytes(row, "token", []byte(token.String())) })
		}
	case "read_value":
		value, err := decoder.ReadValue()
		putError(row, err)
		if err == nil {
			putBytes(row, "value", value)
			row["value_kind"] = value.Kind().String()
		}
	case "skip_value":
		putError(row, decoder.SkipValue())
	case "input_offset":
		row["input_offset"] = decoder.InputOffset()
	case "stack_depth":
		row["stack_depth"] = decoder.StackDepth()
	case "stack_pointer":
		row["stack_pointer"] = string(decoder.StackPointer())
	case "unread_buffer":
		putBytes(row, "unread", decoder.UnreadBuffer())
	case "unmarshal_decode":
		// Always a fresh destination: the stream cases read a new value per
		// call, and the pre-populated destination the `reuse` flag reaches
		// belongs to the missing/null/empty case on the one-shot entry points.
		pointer, render, err := newTarget(target, "")
		if err != nil {
			// A target this probe cannot build is a request it cannot express,
			// so it invalidates the capture rather than becoming a failed read.
			return err
		}
		row["target"] = target
		putError(row, UnmarshalDecode(decoder, pointer))
		render(row)
	}
	return nil
}

func TestPhase1LeavesJson(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	// Each request is kept raw until its subject is known. The schedule is
	// shared with the other leaf groups, and decoding a sibling group's
	// request into this group's struct would fail the whole probe the first
	// time two groups spelled a field name with different types.
	var document struct {
		Requests []stdjson.RawMessage `json:"requests"`
	}
	if err := stdjson.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, raw := range document.Requests {
		var header struct {
			Case      string `json:"case"`
			Operation string `json:"operation"`
			Subject   string `json:"subject"`
		}
		if err := stdjson.Unmarshal(raw, &header); err != nil {
			t.Fatal(err)
		}
		row := map[string]any{"case": header.Case, "operation": header.Operation}
		if !servedSubjects[header.Subject] {
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the json probe"
			observations = append(observations, row)
			continue
		}
		var request leafJSONRequest
		if err := stdjson.Unmarshal(raw, &request); err != nil {
			row["result"] = "harness_failed"
			row["error"] = err.Error()
			observations = append(observations, row)
			continue
		}
		ordered, replayErr := replay(request)
		if replayErr != nil {
			// A request the probe cannot express is a harness failure, not a
			// semantic result: it must invalidate the capture, not pass as one.
			row["result"] = "harness_failed"
			row["error"] = replayErr.Error()
			observations = append(observations, row)
			continue
		}
		row["result"] = "observed"
		row["observation"] = map[string]any{"ordered": ordered}
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
