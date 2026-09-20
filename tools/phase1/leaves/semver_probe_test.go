package semver

// Phase 1 F1a, leaves/text: an access-only probe over the pinned version and
// version-range operations.
//
// It is an in-package test file because `Version`'s components are unexported
// and it has no accessors: only from inside `package semver` can a probe record
// the major/minor/patch/prerelease/build a parse actually produced, which is
// what distinguishes a partial version retained alongside an overflow error
// from a zero value. Every existing test file here also declares
// `package semver`; the probe adds no helper those tests could pick up, because
// every identifier it declares is prefixed `phase1`.
//
// Payloads travel as readable text, because every semver input is valid UTF-8
// by construction; `text_hex` is accepted and wins when present, so a malformed
// input can still be frozen exactly.
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
	"os"
	"runtime"
	"testing"
)

type phase1SemverRequest struct {
	Case      string          `json:"case"`
	Operation string          `json:"operation"`
	Subject   string          `json:"subject"`
	Actions   json.RawMessage `json:"actions"`
}

type phase1SemverAction struct {
	Op       string   `json:"op"`
	Text     string   `json:"text"`
	TextHex  string   `json:"text_hex"`
	Right    string   `json:"right"`
	RightHex string   `json:"right_hex"`
	Versions []string `json:"versions"`
}

func phase1SemverInput(plain string, encoded string) (string, error) {
	if encoded == "" {
		return plain, nil
	}
	raw, err := hex.DecodeString(encoded)
	if err != nil {
		return "", fmt.Errorf("malformed hex payload %q: %w", encoded, err)
	}
	return string(raw), nil
}

// phase1SemverStrings renders a []string component as a JSON list. A nil slice
// and an empty slice are both reported as an empty list, which is what the Rust
// side can observe too; the distinction Go draws between them is not part of
// any caller-visible contract here.
func phase1SemverStrings(values []string) []any {
	out := make([]any, 0, len(values))
	for _, value := range values {
		out = append(out, value)
	}
	return out
}

// phase1SemverParse records everything a parse produced, including the partial
// version retained beside an error.
func phase1SemverParse(text string) []any {
	version, err := TryParseVersion(text)
	message := ""
	if err != nil {
		message = err.Error()
	}
	return []any{
		err == nil,
		message,
		version.major,
		version.minor,
		version.patch,
		phase1SemverStrings(version.prerelease),
		phase1SemverStrings(version.build),
		version.String(),
	}
}

// phase1SemverRange records a range's parse result, its rendered form and its
// membership answer for each requested version. A version that does not parse
// is reported as the literal "unparsed" rather than silently tested as zero.
func phase1SemverRange(text string, versions []string) []any {
	parsed, ok := TryParseVersionRange(text)
	tests := make([]any, 0, len(versions))
	for _, candidate := range versions {
		version, err := TryParseVersion(candidate)
		if err != nil {
			tests = append(tests, "unparsed")
			continue
		}
		tests = append(tests, parsed.Test(&version))
	}
	return []any{ok, parsed.String(), tests}
}

func phase1ReplaySemver(actions []phase1SemverAction) ([]any, error) {
	ordered := make([]any, 0, len(actions))
	for _, action := range actions {
		text, err := phase1SemverInput(action.Text, action.TextHex)
		if err != nil {
			return nil, err
		}
		right, err := phase1SemverInput(action.Right, action.RightHex)
		if err != nil {
			return nil, err
		}
		row := map[string]any{"op": action.Op}
		switch action.Op {
		case "parse":
			row["result"] = phase1SemverParse(text)
		case "must_parse_round_trip":
			// MustParse panics on a bad input, so only inputs the same trace has
			// already observed as parseable are sent here.
			if _, err := TryParseVersion(text); err != nil {
				return nil, fmt.Errorf("must_parse_round_trip was given the unparseable input %q", text)
			}
			version := MustParse(text)
			row["result"] = version.String()
		case "compare":
			left, leftErr := TryParseVersion(text)
			other, rightErr := TryParseVersion(right)
			if leftErr != nil || rightErr != nil {
				row["result"] = "unparsed"
				break
			}
			row["result"] = []any{left.Compare(&other), other.Compare(&left)}
		case "compare_nil_left":
			other, err := TryParseVersion(right)
			if err != nil {
				row["result"] = "unparsed"
				break
			}
			var absent *Version
			row["result"] = []any{absent.Compare(&other), other.Compare(absent)}
		case "compare_nil_both":
			var absent *Version
			row["result"] = absent.Compare(absent)
		case "range":
			row["result"] = phase1SemverRange(text, action.Versions)
		case "range_nil_version":
			parsed, ok := TryParseVersionRange(text)
			var absent *Version
			row["result"] = []any{ok, parsed.String(), parsed.Test(absent)}
		default:
			return nil, fmt.Errorf("the text group's semver probe has no action %q", action.Op)
		}
		ordered = append(ordered, row)
	}
	return ordered, nil
}

func TestPhase1LeavesSemver(t *testing.T) {
	input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var document struct {
		Requests []phase1SemverRequest `json:"requests"`
	}
	if err := json.Unmarshal(input, &document); err != nil {
		t.Fatal(err)
	}

	observations := make([]map[string]any, 0, len(document.Requests))
	for _, request := range document.Requests {
		row := map[string]any{"case": request.Case, "operation": request.Operation}
		switch request.Subject {
		case "semver":
			var actions []phase1SemverAction
			if len(request.Actions) > 0 {
				if err := json.Unmarshal(request.Actions, &actions); err != nil {
					row["result"] = "harness_failed"
					row["error"] = fmt.Sprintf("undecodable actions: %v", err)
					break
				}
			}
			ordered, replayErr := phase1ReplaySemver(actions)
			if replayErr != nil {
				row["result"] = "harness_failed"
				row["error"] = replayErr.Error()
				break
			}
			row["result"] = "observed"
			row["observation"] = map[string]any{"ordered": ordered}
		default:
			row["result"] = "native_unavailable"
			row["reason"] = "subject is not served by the semver probe"
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
