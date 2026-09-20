package stringutil

// Phase 1 F1a, leaves/text: an access-only probe over the pinned string
// comparer, classifier, casing and conversion operations.
//
// It is an in-package test file because several operations under test are only
// reachable from inside the package's own identifier space once the probe needs
// both the exported API and the package's constants. Every existing test file
// in this directory also declares `package stringutil`, so in-package has
// precedent here; the probe still adds no helper those tests could pick up,
// because every identifier it declares is prefixed `phase1`.
//
// Byte discipline. Go's encoding/json replaces malformed UTF-8 and lone
// surrogate escapes with U+FFFD in both directions, which would silently repair
// exactly the inputs these cases exist to test. So every string payload travels
// as lowercase hex: a request carries `<field>_hex` and the probe reports each
// byte result as hex too. A readable `<field>` is accepted for inputs that are
// valid UTF-8 and need no byte-level control; `<field>_hex` wins when both are
// present.
//
// Order. Each case is an ordered action trace and reports one row per action
// under `ordered`, so the comparison's canonicalisation cannot reorder it.
//
// Schedule sharing. The probe answers every request in the family schedule and
// declines the ones it does not own. Other groups' actions are left as raw JSON
// and never decoded, so their field shapes cannot break this probe.
//
// An action this probe cannot replay is a harness failure for its whole
// request, not a row with a missing result: the Rust side answers the same
// condition with a failed outcome, and a schedule defect must invalidate the
// capture on both sides rather than read as a mismatch attributable to Rust.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"runtime"
	"testing"
)

type phase1Request struct {
	Case      string          `json:"case"`
	Operation string          `json:"operation"`
	Subject   string          `json:"subject"`
	Actions   json.RawMessage `json:"actions"`
}

type phase1Action struct {
	Op       string   `json:"op"`
	Left     string   `json:"left"`
	LeftHex  string   `json:"left_hex"`
	Right    string   `json:"right"`
	RightHex string   `json:"right_hex"`
	Extra    string   `json:"extra"`
	ExtraHex string   `json:"extra_hex"`
	Count    int      `json:"count"`
	Enabled  bool     `json:"enabled"`
	Rune     int32    `json:"rune"`
	Rune2    int32    `json:"rune2"`
	LinesHex []string `json:"lines_hex"`
}

// phase1Input resolves one payload field. The hex form is authoritative; the
// readable form exists so a reviewer can read the frozen request.
func phase1Input(plain string, encoded string) (string, error) {
	if encoded == "" {
		return plain, nil
	}
	raw, err := hex.DecodeString(encoded)
	if err != nil {
		return "", fmt.Errorf("malformed hex payload %q: %w", encoded, err)
	}
	return string(raw), nil
}

func phase1Hex(s string) string {
	return hex.EncodeToString([]byte(s))
}

func phase1HexAll(values []string) []any {
	out := make([]any, 0, len(values))
	for _, value := range values {
		out = append(out, phase1Hex(value))
	}
	return out
}

func phase1ReplayText(actions []phase1Action) ([]any, error) {
	ordered := make([]any, 0, len(actions))
	for _, action := range actions {
		left, err := phase1Input(action.Left, action.LeftHex)
		if err != nil {
			return nil, err
		}
		right, err := phase1Input(action.Right, action.RightHex)
		if err != nil {
			return nil, err
		}
		extra, err := phase1Input(action.Extra, action.ExtraHex)
		if err != nil {
			return nil, err
		}
		row := map[string]any{"op": action.Op}
		switch action.Op {
		// Equality and comparison: three separate contracts.
		case "equate_ci":
			row["result"] = EquateStringCaseInsensitive(left, right)
		case "equate_cs":
			row["result"] = EquateStringCaseSensitive(left, right)
		case "equality_comparer_ci":
			row["result"] = GetStringEqualityComparer(true)(left, right)
		case "equality_comparer_cs":
			row["result"] = GetStringEqualityComparer(false)(left, right)
		case "compare_ci":
			row["result"] = CompareStringsCaseInsensitive(left, right)
		case "compare_cs":
			row["result"] = CompareStringsCaseSensitive(left, right)
		case "compare_ci_then_cs":
			row["result"] = CompareStringsCaseInsensitiveThenSensitive(left, right)
		case "compare_eslint":
			row["result"] = CompareStringsCaseInsensitiveEslintCompatible(left, right)
		case "comparer_ci":
			row["result"] = GetStringComparer(true)(left, right)
		case "comparer_cs":
			row["result"] = GetStringComparer(false)(left, right)

		// Affix tests. `enabled` is the caseSensitive argument.
		case "has_prefix":
			row["result"] = HasPrefix(left, right, action.Enabled)
		case "has_suffix":
			row["result"] = HasSuffix(left, right, action.Enabled)
		case "has_prefix_and_suffix":
			row["result"] = HasPrefixAndSuffixWithoutOverlap(left, right, extra, action.Enabled)

		// Rune classifiers.
		case "identifier_start":
			row["result"] = IsUnicodeIdentifierStart(action.Rune)
		case "identifier_part":
			row["result"] = IsUnicodeIdentifierPart(action.Rune)
		case "white_space_like":
			row["result"] = IsWhiteSpaceLike(action.Rune)
		case "white_space_single_line":
			row["result"] = IsWhiteSpaceSingleLine(action.Rune)
		case "line_break":
			row["result"] = IsLineBreak(action.Rune)
		case "digit":
			row["result"] = IsDigit(action.Rune)
		case "octal_digit":
			row["result"] = IsOctalDigit(action.Rune)
		case "hex_digit":
			row["result"] = IsHexDigit(action.Rune)
		case "ascii_letter":
			row["result"] = IsASCIILetter(action.Rune)

		// Casing and truncation.
		case "to_lower_js":
			row["result"] = phase1Hex(ToLowerJS(left))
		case "to_upper_js":
			row["result"] = phase1Hex(ToUpperJS(left))
		case "lower_first_char":
			row["result"] = phase1Hex(LowerFirstChar(left))
		case "truncate_by_runes":
			row["result"] = phase1Hex(TruncateByRunes(left, action.Count))

		// Byte-level conversions.
		case "encode_uri":
			row["result"] = phase1Hex(EncodeURI(left))
		case "remove_bom":
			row["result"] = phase1Hex(RemoveByteOrderMark(left))
		case "add_bom":
			row["result"] = phase1Hex(AddUTF8ByteOrderMark(left))
		case "split_lines":
			row["result"] = phase1HexAll(SplitLines(left))
		case "guess_indentation":
			lines := make([]string, 0, len(action.LinesHex))
			for _, encoded := range action.LinesHex {
				line, err := phase1Input("", encoded)
				if err != nil {
					return nil, err
				}
				lines = append(lines, line)
			}
			row["result"] = GuessIndentation(lines)
		case "strip_quotes":
			row["result"] = phase1Hex(StripQuotes(left))
		case "unquote_string":
			row["result"] = phase1Hex(UnquoteString(left))

		// JS-string rune codec and the surrogate sentinels.
		case "decode_js_rune":
			decoded, size := DecodeJSStringRune(left)
			row["result"] = []any{decoded, size}
		case "encode_js_rune":
			row["result"] = phase1Hex(EncodeJSStringRune(action.Rune))
		case "is_surrogate":
			row["result"] = IsSurrogate(action.Rune)
		case "is_high_surrogate":
			row["result"] = IsHighSurrogate(action.Rune)
		case "is_low_surrogate":
			row["result"] = IsLowSurrogate(action.Rune)
		case "surrogate_pair_to_code_point":
			row["result"] = SurrogatePairToCodePoint(action.Rune, action.Rune2)
		case "code_point_to_surrogate_pair":
			high, low := CodePointToSurrogatePair(action.Rune)
			row["result"] = []any{high, low}
		case "combine_surrogate_pairs":
			row["result"] = phase1Hex(CombineSurrogatePairs(left))

		default:
			return nil, fmt.Errorf("the text group's stringutil probe has no action %q", action.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered, nil
}

func TestPhase1LeavesText(t *testing.T) {
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
		switch request.Subject {
		case "stringutil":
			var actions []phase1Action
			if len(request.Actions) > 0 {
				if err := json.Unmarshal(request.Actions, &actions); err != nil {
					row["result"] = "harness_failed"
					row["error"] = fmt.Sprintf("undecodable actions: %v", err)
					break
				}
			}
			ordered, replayErr := phase1ReplayText(actions)
			if replayErr != nil {
				row["result"] = "harness_failed"
				row["error"] = replayErr.Error()
				break
			}
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": ordered}
		default:
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the stringutil probe"
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
