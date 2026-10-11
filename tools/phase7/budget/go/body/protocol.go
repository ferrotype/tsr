package budget

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"strconv"
)

// Bytes deliberately encodes integer arrays, not encoding/json's []byte base64.
type Bytes []byte

func (b Bytes) MarshalJSON() ([]byte, error) {
	values := make([]int, len(b))
	for i, v := range b {
		values[i] = int(v)
	}
	return json.Marshal(values)
}
func (b *Bytes) UnmarshalJSON(raw []byte) error {
	var values []json.RawMessage
	if len(raw) == 0 || raw[0] != '[' {
		return fmt.Errorf("bytes must be integer array")
	}
	if err := json.Unmarshal(raw, &values); err != nil {
		return err
	}
	result := make(Bytes, len(values))
	for i, rawValue := range values {
		if bytes.Equal(bytes.TrimSpace(rawValue), []byte("null")) {
			return fmt.Errorf("null byte")
		}
		var v int
		if err := json.Unmarshal(rawValue, &v); err != nil {
			return err
		}
		if v < 0 || v > 255 {
			return fmt.Errorf("invalid byte")
		}
		result[i] = byte(v)
	}
	*b = result
	return nil
}

type Entry struct {
	Kind   string `json:"kind"`
	Path   Bytes  `json:"path"`
	Bytes  *Bytes `json:"bytes,omitempty"`
	Target *Bytes `json:"target,omitempty"`
}
type Configuration struct {
	ConfigPath Bytes   `json:"config_path"`
	Arguments  []Bytes `json:"arguments"`
}
type Input struct {
	CurrentDirectory Bytes         `json:"current_directory"`
	CaseSensitive    bool          `json:"case_sensitive"`
	Entries          []Entry       `json:"entries"`
	Configuration    Configuration `json:"configuration"`
	Operation        string        `json:"operation"`
}
type Command struct {
	Version int    `json:"version"`
	Command string `json:"command"`
	ID      string `json:"id"`
	Input   *Input `json:"input,omitempty"`
}
type Response struct {
	Version int    `json:"version"`
	ID      string `json:"id"`
	Kind    string `json:"kind"`
	Value   any    `json:"value"`
}

// Scan separately so encoding/json's otherwise accepted duplicate fields fail.
func uniqueJSON(d *json.Decoder) error {
	t, err := d.Token()
	if err != nil {
		return err
	}
	if delim, ok := t.(json.Delim); ok {
		switch delim {
		case '{':
			seen := map[string]bool{}
			for d.More() {
				key, err := d.Token()
				if err != nil {
					return err
				}
				s := key.(string)
				if seen[s] {
					return fmt.Errorf("duplicate field %q", s)
				}
				seen[s] = true
				if err := uniqueJSON(d); err != nil {
					return err
				}
			}
		case '[':
			for d.More() {
				if err := uniqueJSON(d); err != nil {
					return err
				}
			}
		default:
			return fmt.Errorf("unexpected JSON delimiter")
		}
		_, err = d.Token()
	}
	return err
}
func Decode(raw []byte) (Command, error) {
	var c Command
	scan := json.NewDecoder(bytes.NewReader(raw))
	scan.UseNumber()
	if err := uniqueJSON(scan); err != nil {
		return c, err
	}
	if _, err := scan.Token(); err != io.EOF {
		return c, fmt.Errorf("trailing JSON")
	}
	d := json.NewDecoder(bytes.NewReader(raw))
	d.DisallowUnknownFields()
	if err := d.Decode(&c); err != nil {
		return c, err
	}
	fields, err := required(raw, "version", "command", "id")
	if err != nil {
		return c, err
	}
	if err := keys(fields, "version", "command", "id", "input"); err != nil {
		return c, err
	}
	if c.Command == "prepare" {
		input, err := required(fields["input"], "current_directory", "case_sensitive", "entries", "configuration", "operation")
		if err != nil {
			return c, err
		}
		if err := keys(input, "current_directory", "case_sensitive", "entries", "configuration", "operation"); err != nil {
			return c, err
		}
		configuration, err := required(input["configuration"], "config_path", "arguments")
		if err != nil {
			return c, err
		}
		if err := keys(configuration, "config_path", "arguments"); err != nil {
			return c, err
		}
		for _, key := range []string{"entries"} {
			if bytes.Equal(bytes.TrimSpace(input[key]), []byte("null")) {
				return c, fmt.Errorf("null %s", key)
			}
		}
		if bytes.Equal(bytes.TrimSpace(configuration["arguments"]), []byte("null")) {
			return c, fmt.Errorf("null arguments")
		}
		var entries []json.RawMessage
		if err := json.Unmarshal(input["entries"], &entries); err != nil {
			return c, err
		}
		for _, entry := range entries {
			fields, err := required(entry, "kind", "path")
			if err != nil {
				return c, err
			}
			var kind string
			if err := json.Unmarshal(fields["kind"], &kind); err != nil {
				return c, err
			}
			switch kind {
			case "directory":
				if err := keys(fields, "kind", "path"); err != nil {
					return c, err
				}
			case "physical_file":
				if _, err := required(entry, "bytes"); err != nil {
					return c, err
				}
				if err := keys(fields, "kind", "path", "bytes"); err != nil {
					return c, err
				}
			case "symlink":
				if _, err := required(entry, "target"); err != nil {
					return c, err
				}
				if err := keys(fields, "kind", "path", "target"); err != nil {
					return c, err
				}
			default:
				return c, fmt.Errorf("unsupported snapshot entry")
			}
		}
	}
	if c.Version != 1 {
		return c, fmt.Errorf("unsupported budget protocol version")
	}
	n, err := strconv.ParseUint(c.ID, 10, 64)
	if err != nil || strconv.FormatUint(n, 10) != c.ID {
		return c, fmt.Errorf("invalid operation id")
	}
	if c.Command != "prepare" && c.Input != nil {
		return c, fmt.Errorf("input only valid for prepare")
	}
	return c, nil
}
func keys(fields map[string]json.RawMessage, names ...string) error {
	for key := range fields {
		found := false
		for _, name := range names {
			if key == name {
				found = true
				break
			}
		}
		if !found {
			return fmt.Errorf("unknown field %q", key)
		}
	}
	return nil
}
func required(raw []byte, names ...string) (map[string]json.RawMessage, error) {
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(raw, &fields); err != nil {
		return nil, err
	}
	if fields == nil {
		return nil, fmt.Errorf("expected object")
	}
	for _, name := range names {
		v, ok := fields[name]
		if !ok || bytes.Equal(bytes.TrimSpace(v), []byte("null")) {
			return nil, fmt.Errorf("missing or null %s", name)
		}
	}
	return fields, nil
}
