package jsnum

// Phase 1 F1a, leaves/text: an access-only probe over the pinned JavaScript
// number operations.
//
// It is an in-package test file because `toInt32`, `toUint32`, `toShiftCount`,
// `trunc` and `isNonFinite` are unexported: only from inside `package jsnum`
// can a probe record the conversion each bitwise operator is built on rather
// than inferring it from the operator's result. Every existing test file here
// also declares `package jsnum`; the probe adds no helper those tests could
// pick up, because every identifier it declares is prefixed `phase1`.
//
// Bit discipline. A JSON number cannot carry NaN, an infinity or the sign of
// negative zero, and re-parsing a decimal rendering would not round-trip a
// subnormal. So every Number travels as its IEEE-754 bit pattern in 16 lowercase
// hex digits, and every Number result is reported as the pair
// [bits, String()] - the bit pattern for the exact value and the pinned
// rendering beside it. Text payloads use the same `text` / `text_hex` rule as
// the other leaves/text probes: hex wins when present.
//
// Byte discipline. Every string a bigint operation produces is reported as hex
// as well. ParsePseudoBigInt's default branch passes its input through
// verbatim, so a malformed byte can reach the result; reported as a JSON string
// it would be repaired to U+FFFD by encoding/json here and by from_utf8_lossy
// on the Rust side, and the two sides would agree while hiding the bytes the
// case exists to pin.
//
// The probe answers every request in the family schedule and declines the ones
// it does not own. Other groups' actions stay raw JSON and are never decoded.
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
	"math"
	"os"
	"runtime"
	"strconv"
	"testing"
)

type phase1JsnumRequest struct {
	Case      string          `json:"case"`
	Operation string          `json:"operation"`
	Subject   string          `json:"subject"`
	Actions   json.RawMessage `json:"actions"`
}

type phase1JsnumAction struct {
	Op      string `json:"op"`
	Bits    string `json:"bits"`
	Bits2   string `json:"bits2"`
	Text    string `json:"text"`
	TextHex string `json:"text_hex"`
	Count   int    `json:"count"`
	Enabled bool   `json:"enabled"`
}

func phase1JsnumInput(plain string, encoded string) (string, error) {
	if encoded == "" {
		return plain, nil
	}
	raw, err := hex.DecodeString(encoded)
	if err != nil {
		return "", fmt.Errorf("malformed hex payload %q: %w", encoded, err)
	}
	return string(raw), nil
}

func phase1JsnumHex(value string) string {
	return hex.EncodeToString([]byte(value))
}

func phase1JsnumNumber(bits string) (Number, error) {
	if len(bits) != 16 {
		return 0, fmt.Errorf("a Number payload needs 16 hex digits, got %q", bits)
	}
	pattern, err := strconv.ParseUint(bits, 16, 64)
	if err != nil {
		return 0, fmt.Errorf("malformed Number payload %q: %w", bits, err)
	}
	return Number(math.Float64frombits(pattern)), nil
}

// phase1JsnumValue reports a Number as its bit pattern plus its rendering, so
// an infinity and the sign of a zero survive and a subnormal is exact.
//
// A NaN reports the literal "nan" instead of its bits. ECMAScript has exactly
// one NaN value and no operation here can observe the payload, but Go's
// math.NaN() is 7ff8000000000001 while an IEEE default quiet NaN is
// 7ff8000000000000. Freezing the payload would turn an unobservable runtime
// detail into a permanent parity failure, so the category is frozen and the
// payload is not. Every other bit pattern is recorded exactly.
func phase1JsnumValue(n Number) []any {
	if n.IsNaN() {
		return []any{"nan", n.String()}
	}
	bits := math.Float64bits(float64(n))
	return []any{fmt.Sprintf("%016x", bits), n.String()}
}

func phase1JsnumBigInt(value PseudoBigInt) []any {
	return []any{value.Negative, phase1JsnumHex(value.Base10Value), phase1JsnumHex(value.String())}
}

func phase1ReplayJsnum(actions []phase1JsnumAction) ([]any, error) {
	ordered := make([]any, 0, len(actions))
	for _, action := range actions {
		text, err := phase1JsnumInput(action.Text, action.TextHex)
		if err != nil {
			return nil, err
		}
		// A malformed Number payload is recorded once and checked after the
		// switch, so the operand decoding stays inline in each case.
		var failure error
		number := func(bits string) Number {
			value, err := phase1JsnumNumber(bits)
			if err != nil && failure == nil {
				failure = err
			}
			return value
		}
		row := map[string]any{"op": action.Op}
		switch action.Op {
		// Text conversions.
		case "to_text":
			row["result"] = number(action.Bits).String()
		case "from_text":
			row["result"] = phase1JsnumValue(FromString(text))

		// Integer conversions.
		case "to_int32":
			row["result"] = number(action.Bits).toInt32()
		case "to_uint32":
			row["result"] = number(action.Bits).toUint32()
		case "to_shift_count":
			row["result"] = number(action.Bits).toShiftCount()

		// Bitwise operators and shifts.
		case "signed_right_shift":
			row["result"] = phase1JsnumValue(number(action.Bits).SignedRightShift(number(action.Bits2)))
		case "unsigned_right_shift":
			row["result"] = phase1JsnumValue(number(action.Bits).UnsignedRightShift(number(action.Bits2)))
		case "left_shift":
			row["result"] = phase1JsnumValue(number(action.Bits).LeftShift(number(action.Bits2)))
		case "bitwise_not":
			row["result"] = phase1JsnumValue(number(action.Bits).BitwiseNOT())
		case "bitwise_or":
			row["result"] = phase1JsnumValue(number(action.Bits).BitwiseOR(number(action.Bits2)))
		case "bitwise_and":
			row["result"] = phase1JsnumValue(number(action.Bits).BitwiseAND(number(action.Bits2)))
		case "bitwise_xor":
			row["result"] = phase1JsnumValue(number(action.Bits).BitwiseXOR(number(action.Bits2)))

		// Arithmetic with JavaScript exceptional values.
		case "remainder":
			row["result"] = phase1JsnumValue(number(action.Bits).Remainder(number(action.Bits2)))
		case "exponentiate":
			row["result"] = phase1JsnumValue(number(action.Bits).Exponentiate(number(action.Bits2)))

		// Classification and rounding.
		case "is_nan":
			row["result"] = number(action.Bits).IsNaN()
		case "is_inf":
			row["result"] = number(action.Bits).IsInf()
		case "is_non_finite":
			row["result"] = isNonFinite(float64(number(action.Bits)))
		case "floor":
			row["result"] = phase1JsnumValue(number(action.Bits).Floor())
		case "abs":
			row["result"] = phase1JsnumValue(number(action.Bits).Abs())
		case "trunc":
			row["result"] = phase1JsnumValue(number(action.Bits).trunc())
		case "nan_constructor":
			row["result"] = phase1JsnumValue(NaN())
		case "inf_constructor":
			row["result"] = phase1JsnumValue(Inf(action.Count))

		// Bigint literals.
		case "parse_pseudo_big_int":
			row["result"] = phase1JsnumHex(ParsePseudoBigInt(text))
		case "new_pseudo_big_int":
			row["result"] = phase1JsnumBigInt(NewPseudoBigInt(text, action.Enabled))
		case "parse_valid_big_int":
			row["result"] = phase1JsnumBigInt(ParseValidBigInt(text))
		case "pseudo_big_int_sign":
			row["result"] = NewPseudoBigInt(text, action.Enabled).Sign()

		default:
			return nil, fmt.Errorf("the text group's jsnum probe has no action %q", action.Op)
		}
		if failure != nil {
			return nil, failure
		}
		ordered = append(ordered, row)
	}
	return ordered, nil
}

func TestPhase1LeavesJsnum(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []phase1JsnumRequest `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		switch request.Subject {
		case "jsnum":
			var actions []phase1JsnumAction
			if len(request.Actions) > 0 {
				if err := json.Unmarshal(request.Actions, &actions); err != nil {
					row["result"] = "harness_failed"
					row["error"] = fmt.Sprintf("undecodable actions: %v", err)
					break
				}
			}
			ordered, replayErr := phase1ReplayJsnum(actions)
			if replayErr != nil {
				row["result"] = "harness_failed"
				row["error"] = replayErr.Error()
				break
			}
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": ordered}
		default:
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the jsnum probe"
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
