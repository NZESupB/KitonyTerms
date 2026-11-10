//go:build windows && !server

package osauth

import (
	"context"
	"fmt"
	"runtime"
	"syscall"
	"time"
	"unsafe"

	"golang.org/x/sys/windows"
)

var combase = windows.NewLazySystemDLL("combase.dll")
var roInitialize = combase.NewProc("RoInitialize")
var roUninitialize = combase.NewProc("RoUninitialize")
var createString = combase.NewProc("WindowsCreateString")
var deleteString = combase.NewProc("WindowsDeleteString")
var activationFactory = combase.NewProc("RoGetActivationFactory")

var verifierIID = windows.GUID{Data1: 0xaf4f3f91, Data2: 0x564c, Data3: 0x4ddc, Data4: [8]byte{0xb8, 0xb5, 0x97, 0x34, 0x47, 0x62, 0x7c, 0x65}}
var interopIID = windows.GUID{Data1: 0x39e050c3, Data2: 0x4e74, Data3: 0x441a, Data4: [8]byte{0x8d, 0xc0, 0xb8, 0x11, 0x04, 0xdf, 0x94, 0x9c}}
var asyncInfoIID = windows.GUID{Data1: 0x00000036, Data2: 0x0000, Data3: 0x0000, Data4: [8]byte{0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46}}

// WinRT参数化接口IID，由IAsyncOperation<UserConsentVerificationResult>的官方签名生成。
var verificationIID = windows.GUID{Data1: 0xfd596ffd, Data2: 0x2318, Data3: 0x558f, Data4: [8]byte{0x9d, 0xbe, 0xd2, 0x1d, 0xf4, 0x37, 0x64, 0xa5}}

type hresult uint32

func (h hresult) Error() string {
	return fmt.Sprintf("系统认证接口失败（0x%08x）", uint32(h))
}
func hrError(code uint32) error {
	if int32(code) < 0 {
		return hresult(code)
	}
	return nil
}

type winObject struct{ table *[11]uintptr }

//go:uintptrescapes
func (o *winObject) call(index int, args ...uintptr) uint32 {
	values := append([]uintptr{uintptr(unsafe.Pointer(o))}, args...)
	result, _, _ := syscall.SyscallN(o.table[index], values...)
	runtime.KeepAlive(o)
	return uint32(result)
}
func (o *winObject) release() {
	if o != nil {
		o.call(2)
	}
}
func hstring(value string) (uintptr, error) {
	wide, err := windows.UTF16FromString(value)
	if err != nil {
		return 0, ErrFailed
	}
	var result uintptr
	code, _, _ := createString.Call(uintptr(unsafe.Pointer(&wide[0])), uintptr(len(wide)-1), uintptr(unsafe.Pointer(&result)))
	runtime.KeepAlive(wide)
	return result, hrError(uint32(code))
}
func factory(iid *windows.GUID) (*winObject, error) {
	name, err := hstring("Windows.Security.Credentials.UI.UserConsentVerifier")
	if err != nil {
		return nil, err
	}
	defer deleteString.Call(name)
	var result *winObject
	code, _, _ := activationFactory.Call(name, uintptr(unsafe.Pointer(iid)), uintptr(unsafe.Pointer(&result)))
	return result, hrError(uint32(code))
}

func awaitEnum(ctx context.Context, operation *winObject) (int32, error) {
	if operation == nil {
		return 0, ErrFailed
	}
	defer operation.release()
	var info *winObject
	if err := hrError(operation.call(0, uintptr(unsafe.Pointer(&asyncInfoIID)), uintptr(unsafe.Pointer(&info)))); err != nil {
		return 0, err
	}
	if info == nil {
		return 0, ErrFailed
	}
	defer info.release()
	defer info.call(10)
	timer := time.NewTicker(20 * time.Millisecond)
	defer timer.Stop()
	for {
		if ctx.Err() != nil {
			info.call(9)
			return 0, ErrCancelled
		}
		var status int32
		if err := hrError(info.call(7, uintptr(unsafe.Pointer(&status)))); err != nil {
			return 0, err
		}
		switch status {
		case 1:
			var result int32
			err := hrError(operation.call(8, uintptr(unsafe.Pointer(&result))))
			return result, err
		case 2:
			return 0, ErrCancelled
		case 3:
			var code uint32
			if err := hrError(info.call(8, uintptr(unsafe.Pointer(&code)))); err != nil {
				return 0, err
			}
			if err := hrError(code); err != nil {
				return 0, err
			}
			return 0, ErrFailed
		case 0:
		default:
			return 0, ErrFailed
		}
		select {
		case <-ctx.Done():
			info.call(9)
			return 0, ErrCancelled
		case <-timer.C:
		}
	}
}

func verifyHello(ctx context.Context, reason string, owner uintptr) (bool, error) {
	code, _, _ := roInitialize.Call(1)
	if err := hrError(uint32(code)); err != nil {
		return false, err
	}
	defer roUninitialize.Call()
	statics, err := factory(&verifierIID)
	if err != nil {
		return false, err
	}
	if statics == nil {
		return false, ErrFailed
	}
	defer statics.release()
	var operation *winObject
	if err = hrError(statics.call(6, uintptr(unsafe.Pointer(&operation)))); err != nil {
		return false, err
	}
	availability, err := awaitEnum(ctx, operation)
	if err != nil {
		return false, err
	}
	switch availability {
	case 0:
	case 1, 2, 3:
		return true, nil
	case 4:
		return false, ErrBusy
	default:
		return false, ErrFailed
	}
	interop, err := factory(&interopIID)
	if err != nil {
		return false, err
	}
	if interop == nil {
		return false, ErrFailed
	}
	defer interop.release()
	message, err := hstring(reason)
	if err != nil {
		return false, err
	}
	defer deleteString.Call(message)
	operation = nil
	// IUserConsentVerifierInterop继承IInspectable，窗口方法位于第6槽。
	if err = hrError(interop.call(6, owner, message, uintptr(unsafe.Pointer(&verificationIID)), uintptr(unsafe.Pointer(&operation)))); err != nil {
		return false, err
	}
	result, err := awaitEnum(ctx, operation)
	if err != nil {
		return false, err
	}
	return false, helloResult(result)
}

func Available() bool { return true }
func authenticatePlatform(ctx context.Context, reason string, owner uintptr, method string) error {
	if owner == 0 {
		return ErrUnavailable
	}
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	if method == "auto" {
		password, err := verifyHello(ctx, reason, owner)
		if err != nil {
			if code, ok := err.(hresult); !ok || !helloUnavailable(uint32(code)) {
				return err
			}
		} else if !password {
			return nil
		}
	}
	if ctx.Err() != nil {
		return ErrCancelled
	}
	return verifyCredentials(ctx, reason, owner)
}
