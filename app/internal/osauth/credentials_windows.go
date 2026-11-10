//go:build windows && !server

package osauth

import (
	"context"
	"runtime"
	"unsafe"

	"golang.org/x/sys/windows"
)

var credUI = windows.NewLazySystemDLL("credui.dll")
var packAuthentication = credUI.NewProc("CredPackAuthenticationBufferW")
var unpackAuthentication = credUI.NewProc("CredUnPackAuthenticationBufferW")
var promptCredentials = credUI.NewProc("CredUIPromptForWindowsCredentialsW")
var logonUser = windows.NewLazySystemDLL("advapi32.dll").NewProc("LogonUserW")

type credentialInfo struct {
	Size    uint32
	Owner   uintptr
	Message *uint16
	Title   *uint16
	Banner  uintptr
}

func windowsFailure(err error) error {
	switch err {
	case windows.ERROR_CANCELLED:
		return ErrCancelled
	case windows.ERROR_LOGON_FAILURE:
		return ErrWrongCredentials
	case windows.ERROR_ACCOUNT_LOCKED_OUT:
		return ErrLocked
	case windows.ERROR_ACCOUNT_RESTRICTION, windows.ERROR_LOGON_TYPE_NOT_GRANTED:
		return ErrRestricted
	default:
		return ErrFailed
	}
}
func wipeWide(value []uint16) { clear(value); runtime.KeepAlive(value) }

func verifyCredentials(ctx context.Context, reason string, owner uintptr) error {
	current, err := windows.OpenCurrentProcessToken()
	if err != nil {
		return windowsFailure(err)
	}
	defer current.Close()
	currentUser, err := current.GetTokenUser()
	if err != nil {
		return windowsFailure(err)
	}
	name, domain, _, err := currentUser.User.Sid.LookupAccount("")
	if err != nil {
		return windowsFailure(err)
	}
	fullName := name
	if domain != "" {
		fullName = domain + `\` + name
	}
	account, err := windows.UTF16FromString(fullName)
	if err != nil {
		return ErrFailed
	}
	defer wipeWide(account)
	empty := []uint16{0}
	var size uint32
	ok, _, callErr := packAuthentication.Call(4, uintptr(unsafe.Pointer(&account[0])), uintptr(unsafe.Pointer(&empty[0])), 0, uintptr(unsafe.Pointer(&size)))
	if ok != 0 || callErr != windows.ERROR_INSUFFICIENT_BUFFER || size == 0 || size > 65536 {
		return ErrFailed
	}
	input := make([]byte, size)
	defer func() { clear(input); runtime.KeepAlive(input) }()
	ok, _, callErr = packAuthentication.Call(4, uintptr(unsafe.Pointer(&account[0])), uintptr(unsafe.Pointer(&empty[0])), uintptr(unsafe.Pointer(&input[0])), uintptr(unsafe.Pointer(&size)))
	if ok == 0 {
		return windowsFailure(callErr)
	}
	title, _ := windows.UTF16FromString("KitonyTerms")
	message, err := windows.UTF16FromString(reason)
	if err != nil {
		return ErrFailed
	}
	info := credentialInfo{Owner: owner, Message: &message[0], Title: &title[0]}
	info.Size = uint32(unsafe.Sizeof(info))
	var authenticationPackage uint32
	var output unsafe.Pointer
	var outputSize uint32
	var save int32
	status, _, _ := promptCredentials.Call(uintptr(unsafe.Pointer(&info)), 0, uintptr(unsafe.Pointer(&authenticationPackage)), uintptr(unsafe.Pointer(&input[0])), uintptr(size), uintptr(unsafe.Pointer(&output)), uintptr(unsafe.Pointer(&outputSize)), uintptr(unsafe.Pointer(&save)), 1)
	runtime.KeepAlive(info)
	runtime.KeepAlive(message)
	runtime.KeepAlive(title)
	runtime.KeepAlive(input)
	if output != nil {
		defer func() { clear(unsafe.Slice((*byte)(output), int(outputSize))); windows.CoTaskMemFree(output) }()
	}
	if status != 0 {
		return windowsFailure(windows.Errno(status))
	}
	if ctx.Err() != nil {
		return ErrCancelled
	}
	if output == nil || outputSize == 0 || outputSize > 1<<20 {
		return ErrFailed
	}
	var userSize, domainSize, passwordSize uint32
	ok, _, callErr = unpackAuthentication.Call(0, uintptr(output), uintptr(outputSize), 0, uintptr(unsafe.Pointer(&userSize)), 0, uintptr(unsafe.Pointer(&domainSize)), 0, uintptr(unsafe.Pointer(&passwordSize)))
	if ok != 0 || callErr != windows.ERROR_INSUFFICIENT_BUFFER || userSize == 0 || userSize > 4096 || domainSize > 4096 || passwordSize > 4096 {
		return ErrFailed
	}
	user := make([]uint16, userSize)
	userDomain := make([]uint16, max(1, domainSize))
	password := make([]uint16, max(1, passwordSize))
	defer wipeWide(user)
	defer wipeWide(userDomain)
	defer wipeWide(password)
	domainSize, passwordSize = uint32(len(userDomain)), uint32(len(password))
	ok, _, callErr = unpackAuthentication.Call(0, uintptr(output), uintptr(outputSize), uintptr(unsafe.Pointer(&user[0])), uintptr(unsafe.Pointer(&userSize)), uintptr(unsafe.Pointer(&userDomain[0])), uintptr(unsafe.Pointer(&domainSize)), uintptr(unsafe.Pointer(&password[0])), uintptr(unsafe.Pointer(&passwordSize)))
	if ok == 0 {
		return windowsFailure(callErr)
	}
	if user[len(user)-1] != 0 || userDomain[len(userDomain)-1] != 0 || password[len(password)-1] != 0 {
		return ErrFailed
	}
	username, domainName, hasDomain := accountParts(windows.UTF16ToString(user), windows.UTF16ToString(userDomain), name, domain)
	if username == "" {
		return ErrFailed
	}
	login, err := windows.UTF16FromString(username)
	if err != nil {
		return ErrFailed
	}
	defer wipeWide(login)
	var loginDomain *uint16
	if hasDomain {
		wide, err := windows.UTF16FromString(domainName)
		if err != nil {
			return ErrFailed
		}
		defer wipeWide(wide)
		loginDomain = &wide[0]
	}
	if ctx.Err() != nil {
		return ErrCancelled
	}
	// 即使密码为空也必须调用系统；只有登录成功且SID一致才通过。
	var token windows.Token
	ok, _, callErr = logonUser.Call(uintptr(unsafe.Pointer(&login[0])), uintptr(unsafe.Pointer(loginDomain)), uintptr(unsafe.Pointer(&password[0])), 2, 0, uintptr(unsafe.Pointer(&token)))
	runtime.KeepAlive(login)
	runtime.KeepAlive(loginDomain)
	runtime.KeepAlive(password)
	if ok == 0 {
		return windowsFailure(callErr)
	}
	defer token.Close()
	verified, err := token.GetTokenUser()
	if err != nil {
		return windowsFailure(err)
	}
	if !windows.EqualSid(currentUser.User.Sid, verified.User.Sid) {
		return ErrWrongCredentials
	}
	if ctx.Err() != nil {
		return ErrCancelled
	}
	return nil
}
