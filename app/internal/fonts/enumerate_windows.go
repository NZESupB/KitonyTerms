//go:build windows

package fonts

import (
	"context"
	"runtime"
	"unsafe"

	"golang.org/x/sys/windows"
)

var fontGDI = windows.NewLazySystemDLL("gdi32.dll")
var createFontDC = fontGDI.NewProc("CreateCompatibleDC")
var deleteFontDC = fontGDI.NewProc("DeleteDC")
var enumFontFamilies = fontGDI.NewProc("EnumFontFamiliesExW")

// 固定回调只创建一次；GDI同步回调期间由独占令牌保护本轮状态。
var fontEnumerationGate = make(chan struct{}, 1)
var activeFontEnumeration *windowsFontEnumeration
var fontEnumerationCallback = windows.NewCallback(collectWindowsFont)

type logFontW struct {
	Height, Width, Escapement, Orientation, Weight int32
	Italic, Underline, StrikeOut, CharSet          byte
	OutPrecision, ClipPrecision, Quality, Pitch    byte
	FaceName                                       [32]uint16
}

type windowsFontEnumeration struct {
	ctx   context.Context
	names []string
	err   error
}

func collectWindowsFont(font *logFontW, _ uintptr, _ uint32, _ uintptr) uintptr {
	state := activeFontEnumeration
	if err := state.ctx.Err(); err != nil {
		state.err = err
		return 0
	}
	if font == nil || len(state.names) >= maxRecords {
		state.err = errEnumeration
		return 0
	}
	state.names = append(state.names, windows.UTF16ToString(font.FaceName[:]))
	return 1
}

func enumerate(ctx context.Context) ([]string, error) {
	select {
	case fontEnumerationGate <- struct{}{}:
		defer func() { <-fontEnumerationGate }()
	case <-ctx.Done():
		return nil, ctx.Err()
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	dc, _, _ := createFontDC.Call(0)
	if dc == 0 {
		return nil, errEnumeration
	}
	defer deleteFontDC.Call(dc)
	state := &windowsFontEnumeration{ctx: ctx}
	activeFontEnumeration = state
	defer func() { activeFontEnumeration = nil }()
	font := logFontW{CharSet: 1} // DEFAULT_CHARSET；空FaceName、零Pitch枚举全部族。
	result, _, _ := enumFontFamilies.Call(dc, uintptr(unsafe.Pointer(&font)), fontEnumerationCallback, 0, 0)
	runtime.KeepAlive(&font)
	if state.err != nil {
		return nil, state.err
	}
	if result == 0 {
		return nil, errEnumeration
	}
	return state.names, nil
}
