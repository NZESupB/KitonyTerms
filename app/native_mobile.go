//go:build android || ios

package main

import (
	"github.com/wailsapp/wails/v3/pkg/application"
	"kitonyterms/internal/platform"
	"kitonyterms/internal/update"
)

func installDesktopUI(_ *application.App, _ *application.WebviewWindow) {}

func runPlatformCommand() (bool, error) { return false, nil }

func preparePlatformStartup() (platform.Paths, *update.Desktop, bool, error) {
	return platform.Paths{}, nil, false, nil
}
