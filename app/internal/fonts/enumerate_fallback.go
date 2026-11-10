//go:build (!darwin && !linux && !windows) || ios || android || (darwin && !cgo)

package fonts

import "context"

func enumerate(context.Context) ([]string, error) { return nil, errUnavailable }
