//go:build !android && !ios

package main

import (
	"context"
	"errors"
	"os"

	"github.com/wailsapp/wails/v3/pkg/application"
	"kitonyterms/internal/platform"
	"kitonyterms/internal/update"
)

func installDesktopUI(app *application.App, window *application.WebviewWindow) {
	menu := app.Menu.New()
	if application.System.IsPlatform(application.PlatformMacOS) {
		menu.AddRole(application.AppMenu)
	}
	menu.AddRole(application.EditMenu)
	app.Menu.Set(menu)
}

func runPlatformCommand() (bool, error) {
	return update.HandleDesktopCommand(context.Background(), os.Args[1:], os.Stdout, func() (string, error) {
		paths, err := platform.Discover()
		return paths.Data, err
	})
}

func preparePlatformStartup() (platform.Paths, *update.Desktop, bool, error) {
	paths, err := platform.Discover()
	if err != nil {
		return paths, nil, false, err
	}
	self, err := os.Executable()
	if err != nil {
		return paths, nil, false, err
	}
	desktop, err := update.OpenDesktop(context.Background(), paths.Data, self)
	if err != nil {
		return paths, nil, false, err
	}
	if desktop.NeedsRecovery() {
		if err = desktop.RelaunchRecovery(context.Background()); err != nil {
			return paths, nil, false, errors.Join(err, desktop.Close())
		}
		return paths, desktop, true, nil
	}
	return paths, desktop, false, nil
}
