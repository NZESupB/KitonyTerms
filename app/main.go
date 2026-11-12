package main

import (
	"embed"
	"errors"
	"github.com/wailsapp/wails/v3/pkg/application"
	"github.com/wailsapp/wails/v3/pkg/events"
	"kitonyterms/internal/ipc"
	"kitonyterms/internal/lifecycle"
	"log"
	"os"
)

//go:embed all:frontend/dist
var assets embed.FS

// 对外应用、窗口和版本资源统一使用该显示名，避免任务管理器出现中文描述或迁移版后缀。
const appDisplayName = "KitonyTerms"

func main() {
	if err := run(); err != nil {
		log.Print(err)
		os.Exit(1)
	}
}

func run() (err error) {
	handled, err := runPlatformCommand()
	if err != nil {
		return err
	}
	if handled {
		return nil
	}
	paths, desktop, restarting, err := preparePlatformStartup()
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, desktop.Close()) }()
	if restarting {
		return nil
	}
	var quit *lifecycle.Controller
	app := application.New(application.Options{Name: appDisplayName, Description: appDisplayName, DisableDefaultSignalHandler: application.System.IsPlatform(application.PlatformIOS), ShouldQuit: func() bool { return quit == nil || quit.ShouldQuit() }, Assets: application.AssetOptions{Handler: application.AssetFileServerFS(assets)}, Mac: application.MacOptions{ApplicationShouldTerminateAfterLastWindowClosed: true}})
	quit = lifecycle.New(func() { app.Event.Emit("app:close-requested") })
	window := app.Window.NewWithOptions(application.WebviewWindowOptions{Name: "main", Title: appDisplayName, Width: 1200, Height: 800, Frameless: !application.System.IsMobile(), URL: "/"})
	if !application.System.IsMobile() {
		window.RegisterHook(events.Common.WindowClosing, func(event *application.WindowEvent) {
			if !quit.ShouldQuit() {
				event.Cancel()
			}
		})
	}
	app.RegisterService(application.NewService(ipc.NewBackend(app, window, quit, paths, desktop)))
	installDesktopUI(app, window)
	return app.Run()
}
