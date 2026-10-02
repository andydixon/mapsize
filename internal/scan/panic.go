package scan

import "runtime/debug"

// OnPanic is called if a scanner goroutine panics. The default re-panics
// (crashing the process); the TUI replaces it so it can restore the terminal
// before reporting. Programmer bugs are never silently swallowed.
var OnPanic = func(v any, stack []byte) { panic(v) }

func guard() {
	if v := recover(); v != nil {
		OnPanic(v, debug.Stack())
	}
}
