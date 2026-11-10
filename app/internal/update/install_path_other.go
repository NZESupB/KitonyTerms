//go:build !windows

package update

import "path/filepath"

func canonicalUpdatePath(path string) (string, error) { return filepath.EvalSymlinks(path) }
