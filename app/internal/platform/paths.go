package platform

import (
	"os"
	"path/filepath"
)

type Paths struct {
	Config string
	Data   string
}

func PathsForRoot(root string) Paths {
	return Paths{Config: filepath.Join(root, "config"), Data: filepath.Join(root, "data")}
}
func ensurePaths(p Paths) (Paths, error) {
	for _, dir := range []string{p.Config, p.Data} {
		if err := os.MkdirAll(dir, 0700); err != nil {
			return Paths{}, err
		}
	}
	return p, nil
}
