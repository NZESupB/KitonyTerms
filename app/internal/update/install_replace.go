package update

import (
	"errors"
	"fmt"
	"os"
	"syscall"
)

type replaceFailure struct {
	Target, Kind string
	Err          error
}

func (e *replaceFailure) Error() string {
	return fmt.Sprintf("更新替换失败（%s）：%s", e.Kind, e.Target)
}
func (e *replaceFailure) Unwrap() error { return e.Err }

func replaceErrorKind(err error) string {
	var code syscall.Errno
	if errors.As(err, &code) {
		switch uint32(code) {
		case 5:
			return "access_denied"
		case 32, 33, 1224:
			return "in_use"
		}
	}
	return "other"
}

func replacementNeedsElevation(err error, target string) bool {
	var failure *replaceFailure
	if !errors.As(err, &failure) || !sameInstallPath(failure.Target, target) || failure.Kind != "access_denied" {
		return false
	}
	return installTargetAllowsElevation(target)
}

func installTargetAllowsElevation(target string) bool {
	info, err := os.Lstat(target)
	return err == nil && info.Mode().IsRegular() && info.Mode().Perm()&0200 != 0
}
