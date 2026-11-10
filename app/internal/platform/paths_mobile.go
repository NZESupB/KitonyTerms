//go:build android || ios

package platform

import (
	"fmt"
	"github.com/wailsapp/wails/v3/pkg/application"
	"path/filepath"
)

func Discover() (Paths, error) {
	root := application.Mobile.StoragePath()
	if root == "" || !filepath.IsAbs(root) {
		return Paths{}, fmt.Errorf("移动宿主私有存储未就绪")
	}
	return ensurePaths(PathsForRoot(root))
}
