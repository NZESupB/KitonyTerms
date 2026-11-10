//go:build windows

package datasnapshot

import (
	"errors"
	"os"
	"runtime"
	"unsafe"

	"golang.org/x/sys/windows"
)

// 先保护空目标，再写入vault/key等内容；不依赖Windows忽略的POSIX权限位。
func protectDestination(root *os.Root) (err error) {
	user, err := windows.GetCurrentProcessToken().GetTokenUser()
	if err != nil {
		return err
	}
	descriptor, err := windows.SecurityDescriptorFromString("D:P(A;OICI;FA;;;" + user.User.Sid.String() + ")")
	if err != nil {
		return err
	}
	dacl, _, err := descriptor.DACL()
	if err != nil {
		return err
	}
	path, err := windows.UTF16PtrFromString(root.Name())
	if err != nil {
		return err
	}
	// 禁止换名/删除，先与os.Root比对身份，再对这个句柄设置权限。
	handle, err := windows.CreateFile(path, windows.WRITE_DAC|windows.READ_CONTROL|windows.FILE_READ_ATTRIBUTES,
		windows.FILE_SHARE_READ|windows.FILE_SHARE_WRITE, nil, windows.OPEN_EXISTING,
		windows.FILE_FLAG_BACKUP_SEMANTICS|windows.FILE_FLAG_OPEN_REPARSE_POINT, 0)
	if err != nil {
		return err
	}
	file := os.NewFile(uintptr(handle), "snapshot-permissions")
	defer func() { err = errors.Join(err, file.Close()) }()
	opened, err := file.Stat()
	if err != nil {
		return err
	}
	original, err := root.Stat(".")
	if err != nil {
		return err
	}
	if !opened.IsDir() || !os.SameFile(opened, original) {
		return errors.New("权限目标身份变化")
	}
	if err = windows.SetSecurityInfo(handle, windows.SE_FILE_OBJECT,
		windows.DACL_SECURITY_INFORMATION|windows.PROTECTED_DACL_SECURITY_INFORMATION, nil, nil, dacl, nil); err != nil {
		return err
	}
	runtime.KeepAlive(descriptor)
	actual, err := windows.GetSecurityInfo(handle, windows.SE_FILE_OBJECT, windows.DACL_SECURITY_INFORMATION)
	if err != nil {
		return err
	}
	return checkPrivateDACL(actual, user.User.Sid)
}

func checkPrivateDACL(descriptor *windows.SECURITY_DESCRIPTOR, user *windows.SID) error {
	invalid := errors.New("目标目录私有DACL未得到确认")
	control, _, err := descriptor.Control()
	if err != nil {
		return err
	}
	if control&windows.SE_DACL_PROTECTED == 0 {
		return invalid
	}
	dacl, _, err := descriptor.DACL()
	if err != nil {
		return err
	}
	if dacl == nil || dacl.AceCount != 1 {
		return invalid
	}
	var ace *windows.ACCESS_ALLOWED_ACE
	if err = windows.GetAce(dacl, 0, &ace); err != nil {
		return err
	}
	if ace == nil || ace.Header.AceType != windows.ACCESS_ALLOWED_ACE_TYPE ||
		ace.Header.AceFlags != windows.OBJECT_INHERIT_ACE|windows.CONTAINER_INHERIT_ACE ||
		ace.Mask != 0x1f01ff || !windows.EqualSid((*windows.SID)(unsafe.Pointer(&ace.SidStart)), user) {
		return invalid
	}
	runtime.KeepAlive(descriptor)
	return nil
}
