package language

// Export the candidate index built by the pinned matcher, plus its tie-break
// tables. This is independent of the Phase 1 request corpus.
import (
	"encoding/json"
	"os"
	"reflect"
	"testing"
)

func numeric(v reflect.Value) any {
	switch v.Kind() {
	case reflect.Array, reflect.Slice:
		out := make([]any, v.Len())
		for i := range out {
			out[i] = numeric(v.Index(i))
		}
		return out
	case reflect.Struct:
		out := make([]any, v.NumField())
		for i := range out {
			out[i] = numeric(v.Field(i))
		}
		return out
	default:
		return v.Uint()
	}
}
func TestPhase1LocaleMatcher(t *testing.T) {
	names := []string{"en", "zh-CN", "zh-TW", "cs-CZ", "de-DE", "es-ES", "fr-FR", "it-IT", "ja-JP", "ko-KR", "pl-PL", "pt-BR", "ru-RU", "tr-TR"}
	tags := make([]Tag, len(names))
	for i, s := range names {
		tags[i] = MustParse(s)
	}
	m := newMatcher(tags, nil)
	index := map[uint16][][]uint16{}
	for base, head := range m.index {
		for _, h := range head.haveTags {
			index[uint16(base)] = append(index[uint16(base)], []uint16{uint16(h.index), uint16(h.tag.LangID), uint16(h.tag.ScriptID), uint16(h.tag.RegionID), uint16(h.maxScript), uint16(h.maxRegion), uint16(h.altScript), uint16(h.conf), uint16(h.nextMax)})
		}
	}
	out := map[string]any{"supported": names, "candidates": index, "region_groups": numeric(reflect.ValueOf(regionToGroups)), "paradigms": paradigmLocales, "match_region": numeric(reflect.ValueOf(matchRegion))}
	data, e := json.Marshal(out)
	if e != nil {
		t.Fatal(e)
	}
	if e = os.WriteFile(os.Getenv("PHASE1_LOCALE_MATCHER"), data, 0600); e != nil {
		t.Fatal(e)
	}
}

func TestPhase1LocaleConformance(t *testing.T) {
	path := os.Getenv("PHASE1_LOCALE_CASES")
	if path == "" {
		return
	}
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var inputs []string
	if err = json.Unmarshal(raw, &inputs); err != nil {
		t.Fatal(err)
	}
	names := []string{"en", "zh-CN", "zh-TW", "cs-CZ", "de-DE", "es-ES", "fr-FR", "it-IT", "ja-JP", "ko-KR", "pl-PL", "pt-BR", "ru-RU", "tr-TR"}
	tags := make([]Tag, len(names))
	for i, s := range names {
		tags[i] = MustParse(s)
	}
	matcher := NewMatcher(tags)
	rows := make([]any, 0, len(inputs))
	for _, input := range inputs {
		tag, e := Parse(input)
		message := ""
		if e != nil {
			message = e.Error()
		}
		_, index, confidence := matcher.Match(tag)
		if confidence < Low {
			index = -1
		}
		rows = append(rows, []any{input, tag.String(), message, index})
	}
	data, err := json.Marshal(rows)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(os.Getenv("PHASE1_LOCALE_CONFORMANCE"), data, 0600); err != nil {
		t.Fatal(err)
	}
}
