package language

// Access-only export of the pinned x/text registry and CLDR tables. No parser
// or matcher is implemented here; Rust consumes data, not fixture answers.
import (
	"bytes"
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
	case reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64, reflect.Int:
		return v.Int()
	default:
		return v.Uint()
	}
}
func TestPhase1LocaleTables(t *testing.T) {
	langs := map[string]uint16{"und": 0}
	for a := byte('a'); a <= 'z'; a++ {
		for b := byte('a'); b <= 'z'; b++ {
			key := []byte{a, b}
			if id, err := getLangID(key); err == nil {
				langs[string(key)] = uint16(id)
			}
			for c := byte('a'); c <= 'z'; c++ {
				key := []byte{a, b, c}
				if id, err := getLangID(key); err == nil {
					langs[string(key)] = uint16(id)
				}
			}
		}
	}
	scripts := map[string]uint16{}
	for i := 1; i < (NumScripts + 1); i++ {
		scripts[Script(i).String()] = uint16(i)
	}
	regions := map[string]uint16{}
	for i := 1; i < len(regionTypes); i++ {
		r := Region(i)
		regions[r.String()] = uint16(i)
	}
	// Numeric region spellings are accepted even when the canonical spelling is alpha.
	for i := 0; i < 1000; i++ {
		if r, e := getRegionM49(i); e == nil {
			var s [3]byte
			s[0] = byte(i/100) + '0'
			s[1] = byte(i/10%10) + '0'
			s[2] = byte(i%10) + '0'
			regions[string(s[:])] = uint16(r)
		}
	}
	grandfather := map[string]string{}
	for key := range grandfatheredMap {
		v, _ := grandfathered(key)
		grandfather[string(bytes.TrimRight(key[:], "\x00"))] = v.String()
	}
	languageNames := map[uint16]string{}
	for _, id := range langs {
		languageNames[id] = Language(id).String()
	}
	regionNames := make([]string, len(regionTypes))
	for i := range regionNames {
		regionNames[i] = Region(i).String()
	}
	scriptNames := make([]string, (NumScripts + 1))
	for i := range scriptNames {
		scriptNames[i] = Script(i).String()
	}
	out := map[string]any{"languages": langs, "language_names": languageNames, "scripts": scripts, "script_names": scriptNames, "regions": regions, "region_names": regionNames, "variants": variantIndex, "grandfathered": grandfather, "lang_no_index_offset": langNoIndexOffset, "private_start": langPrivateStart, "private_end": langPrivateEnd}
	for name, value := range map[string]any{"aliases": AliasMap, "alias_types": AliasTypes, "region_aliases": regionOldMap, "suppress_script": suppressScript, "likely_script": likelyScript, "likely_lang": likelyLang, "likely_lang_list": likelyLangList, "likely_region": likelyRegion, "likely_region_list": likelyRegionList, "likely_region_group": likelyRegionGroup, "region_containment": regionContainment, "region_inclusion": regionInclusion, "region_inclusion_bits": regionInclusionBits, "region_inclusion_next": regionInclusionNext} {
		out[name] = numeric(reflect.ValueOf(value))
	}
	data, e := json.Marshal(out)
	if e != nil {
		t.Fatal(e)
	}
	if e = os.WriteFile(os.Getenv("PHASE1_LOCALE_INTERNAL"), data, 0600); e != nil {
		t.Fatal(e)
	}
}
