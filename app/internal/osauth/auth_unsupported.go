//go:build (!darwin && !windows) || ios || server || (darwin && !cgo)

package osauth

import "context"

func Available() bool                                                     { return false }
func authenticatePlatform(context.Context, string, uintptr, string) error { return ErrUnavailable }
