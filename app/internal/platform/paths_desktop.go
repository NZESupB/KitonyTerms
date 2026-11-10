//go:build !android && !ios

package platform

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"

	"kitonyterms/internal/update"
)

func Discover() (Paths, error) {
	if root := os.Getenv("KITONYTERMS_DATA_DIR"); root != "" {
		absolute, err := filepath.Abs(root)
		if err != nil {
			return Paths{}, err
		}
		return ensurePaths(PathsForRoot(absolute))
	}
	build, err := update.CurrentBuild()
	if err != nil {
		return Paths{}, err
	}
	base, err := os.UserConfigDir()
	if err != nil {
		return Paths{}, err
	}
	if build.Channel == "development" {
		return ensurePaths(PathsForRoot(filepath.Join(base, "KitonyTerms-Wails-Development")))
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return Paths{}, err
	}
	paths, err := legacyPaths(runtime.GOOS, home, base, os.Getenv)
	if err != nil {
		return Paths{}, err
	}
	return ensurePaths(paths)
}

// 与旧 directories::ProjectDirs::from("com", "kitony", "KitonyTerms") 保持一致。
// 仅计算路径；测试使用临时home，不创建或读取当前用户的数据。
func legacyPaths(system, home, configBase string, getenv func(string) string) (Paths, error) {
	switch system {
	case "darwin":
		root := filepath.Join(home, "Library", "Application Support", "com.kitony.KitonyTerms")
		return Paths{Config: root, Data: root}, nil
	case "windows":
		root := filepath.Join(configBase, "kitony", "KitonyTerms")
		return PathsForRoot(root), nil
	case "linux":
		data := getenv("XDG_DATA_HOME")
		if !filepath.IsAbs(data) {
			data = filepath.Join(home, ".local", "share")
		}
		return Paths{Config: filepath.Join(configBase, "kitonyterms"), Data: filepath.Join(data, "kitonyterms")}, nil
	default:
		return Paths{}, fmt.Errorf("此桌面平台没有兼容的数据目录")
	}
}
