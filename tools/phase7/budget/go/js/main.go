//go:build js && wasm

package main

import (
	"encoding/json"
	"syscall/js"

	budget "github.com/microsoft/TypeScript/tsc/cmd/phase7-budget/body"
)

func main() {
	engine := &budget.Engine{}
	failed := false
	execute := js.FuncOf(func(_ js.Value, args []js.Value) (result any) {
		defer func() {
			if recover() != nil {
				failed = true
				result = `{"version":1,"id":"","kind":"failure","value":"Go panic; discard instance"}`
			}
		}()
		if failed {
			return `{"version":1,"id":"","kind":"failure","value":"terminal instance"}`
		}
		if len(args) != 1 || args[0].Type() != js.TypeString {
			failed = true
			return `{"version":1,"id":"","kind":"failure","value":"expected one JSON command string"}`
		}
		response, err := engine.ExecuteCommand([]byte(args[0].String()))
		if err != nil {
			failed = true
			response.Kind = "failure"
			response.Value = err.Error()
		}
		raw, err := json.Marshal(response)
		if err != nil {
			failed = true
			return `{"version":1,"id":"","kind":"failure","value":"response serialization failed"}`
		}
		return string(raw)
	})
	js.Global().Set("phase7BudgetGo", execute)
	select {}
}
