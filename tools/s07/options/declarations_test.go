package tsoptions

import (
	"encoding/json"
	"os"
	"reflect"
	"slices"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/diagnostics"
)

type s07Declaration struct {
	Name              string                `json:"name"`
	ShortName         string                `json:"short_name"`
	Kind              CommandLineOptionKind `json:"kind"`
	IsFilePath        bool                  `json:"is_file_path"`
	IsTSConfigOnly    bool                  `json:"is_tsconfig_only"`
	IsCommandLineOnly bool                  `json:"is_command_line_only"`
	ExtraValidation   string                `json:"extra_validation"`
	MinValue          int                   `json:"min_value"`
	Template          bool                  `json:"template"`
	PreserveFalsy     bool                  `json:"preserve_falsy"`
	Enum              []s07Enum             `json:"enum"`
	Deprecated        []string              `json:"deprecated"`
	Element           *s07Declaration       `json:"element"`
	Category          *int32                `json:"category"`
	Description       *int32                `json:"description"`
	Default           s07Default            `json:"default"`
	Simplified        bool                  `json:"simplified"`
}
type s07Default struct {
	Kind  string `json:"kind"`
	Value any    `json:"value"`
}

func s07Message(message *diagnostics.Message) *int32 {
	if message == nil {
		return nil
	}
	code := message.Code()
	return &code
}

func s07DefaultValue(value any) s07Default {
	switch value := value.(type) {
	case nil:
		return s07Default{Kind: "nil"}
	case *diagnostics.Message:
		if value == nil {
			return s07Default{Kind: "nil"}
		}
		return s07Default{Kind: "message", Value: value.Code()}
	case bool:
		return s07Default{Kind: "boolean", Value: value}
	case string:
		return s07Default{Kind: "string", Value: value}
	case core.Tristate:
		if value == core.TSUnknown {
			return s07Default{Kind: "unknown"}
		}
	}
	r := reflect.ValueOf(value)
	if r.Kind() >= reflect.Int && r.Kind() <= reflect.Int64 {
		return s07Default{Kind: "number", Value: r.Int()}
	}
	panic("unsupported option default type: " + r.Type().String())
}

type s07Enum struct {
	Key   string `json:"key"`
	Value any    `json:"value"`
}

func s07Decl(option *CommandLineOption) *s07Declaration {
	if option == nil {
		return nil
	}
	result := &s07Declaration{Name: option.Name, ShortName: option.ShortName, Kind: option.Kind, IsFilePath: option.IsFilePath, IsTSConfigOnly: option.IsTSConfigOnly, IsCommandLineOnly: option.IsCommandLineOnly, ExtraValidation: string(option.extraValidation), MinValue: option.minValue, Template: option.allowConfigDirTemplateSubstitution, PreserveFalsy: option.listPreserveFalsyValues, Element: s07Decl(option.Elements())}
	result.Category = s07Message(option.Category)
	result.Description = s07Message(option.Description)
	result.Default = s07DefaultValue(option.DefaultValueDescription)
	result.Simplified = option.ShowInSimplifiedHelpView
	if values := option.EnumMap(); values != nil {
		for key, value := range values.Entries() {
			result.Enum = append(result.Enum, s07Enum{key, value})
		}
	}
	if values := option.DeprecatedKeys(); values != nil {
		for key := range values.Keys() {
			result.Deprecated = append(result.Deprecated, key)
		}
		slices.Sort(result.Deprecated)
	}
	return result
}
func TestS07Declarations(t *testing.T) {
	result := map[string][]*s07Declaration{}
	for _, option := range OptionsDeclarations {
		result["compiler"] = append(result["compiler"], s07Decl(option))
	}
	for _, option := range BuildOpts {
		result["build"] = append(result["build"], s07Decl(option))
	}
	for _, option := range OptionsForBuild {
		result["build_help"] = append(result["build_help"], s07Decl(option))
	}
	for _, option := range typeAcquisitionDecls {
		result["type_acquisition"] = append(result["type_acquisition"], s07Decl(option))
	}
	for _, option := range OptionsForWatch {
		result["watch"] = append(result["watch"], s07Decl(option))
	}
	for _, option := range tsconfigRootOptionsMap.ElementOptions {
		result["root"] = append(result["root"], s07Decl(option))
	}
	slices.SortFunc(result["root"], func(a, b *s07Declaration) int {
		if a.Name < b.Name {
			return -1
		}
		if a.Name > b.Name {
			return 1
		}
		return 0
	})
	data, err := json.Marshal(result)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S07_DECLARATIONS_OUTPUT"), append(data, '\n'), 0600); err != nil {
		t.Fatal(err)
	}
}
