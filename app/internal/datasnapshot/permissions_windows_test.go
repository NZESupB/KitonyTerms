//go:build windows

package datasnapshot

import (
	"os"
	"path/filepath"
	"runtime"
	"testing"
	"unsafe"

	"golang.org/x/sys/windows"
)

func TestWindowsPrivateDACLRejectsBroadOrUnprotected(t *testing.T) {
	user, err := windows.GetCurrentProcessToken().GetTokenUser()
	if err != nil {
		t.Fatal(err)
	}
	sid := user.User.Sid.String()
	for _, item := range []struct {
		name, sddl string
		valid      bool
	}{
		{"private", "D:P(A;OICI;FA;;;" + sid + ")", true},
		{"inherited-parent", "D:(A;OICI;FA;;;" + sid + ")", false},
		{"everyone", "D:P(A;OICI;FA;;;WD)", false},
		{"extra-allow", "D:P(A;OICI;FA;;;" + sid + ")(A;;FR;;;WD)", false},
		{"no-child-inheritance", "D:P(A;;FA;;;" + sid + ")", false},
		{"read-only", "D:P(A;OICI;FR;;;" + sid + ")", false},
	} {
		t.Run(item.name, func(t *testing.T) {
			descriptor, err := windows.SecurityDescriptorFromString(item.sddl)
			if err != nil {
				t.Fatal(err)
			}
			if (checkPrivateDACL(descriptor, user.User.Sid) == nil) != item.valid {
				t.Fatal("DACL验证与预期不同")
			}
		})
	}
}

func TestWindowsPrivateDestinationAndChildInheritance(t *testing.T) {
	parent := t.TempDir()
	broad, err := windows.SecurityDescriptorFromString("D:P(A;OICI;FA;;;WD)")
	if err != nil {
		t.Fatal(err)
	}
	dacl, _, err := broad.DACL()
	if err != nil {
		t.Fatal(err)
	}
	if err = windows.SetNamedSecurityInfo(parent, windows.SE_FILE_OBJECT,
		windows.DACL_SECURITY_INFORMATION|windows.PROTECTED_DACL_SECURITY_INFORMATION, nil, nil, dacl, nil); err != nil {
		t.Fatal(err)
	}
	runtime.KeepAlive(broad)
	root, err := newDestination(filepath.Join(parent, "private"), nil)
	if err != nil {
		t.Fatal(err)
	}
	defer root.Close()
	user, err := windows.GetCurrentProcessToken().GetTokenUser()
	if err != nil {
		t.Fatal(err)
	}
	actual, err := windows.GetNamedSecurityInfo(root.Name(), windows.SE_FILE_OBJECT, windows.DACL_SECURITY_INFORMATION)
	if err != nil {
		t.Fatal(err)
	}
	if err = checkPrivateDACL(actual, user.User.Sid); err != nil {
		t.Fatal(err)
	}
	if err = root.Mkdir("config", 0700); err != nil {
		t.Fatal(err)
	}
	file, err := root.OpenFile("config/key", os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
	if err != nil {
		t.Fatal(err)
	}
	if err = file.Close(); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"config", "config/key"} {
		descriptor, err := windows.GetNamedSecurityInfo(filepath.Join(root.Name(), name), windows.SE_FILE_OBJECT, windows.DACL_SECURITY_INFORMATION)
		if err != nil {
			t.Fatal(err)
		}
		dacl, _, err := descriptor.DACL()
		if err != nil || dacl == nil || dacl.AceCount != 1 {
			t.Fatal("子项必须只继承当前用户", err)
		}
		var ace *windows.ACCESS_ALLOWED_ACE
		if err = windows.GetAce(dacl, 0, &ace); err != nil {
			t.Fatal(err)
		}
		if ace.Header.AceType != windows.ACCESS_ALLOWED_ACE_TYPE || ace.Header.AceFlags&windows.INHERITED_ACE == 0 ||
			ace.Mask != 0x1f01ff || !windows.EqualSid((*windows.SID)(unsafe.Pointer(&ace.SidStart)), user.User.Sid) {
			t.Fatal("子项继承了额外或不正确的访问权限")
		}
		runtime.KeepAlive(descriptor)
	}
}
