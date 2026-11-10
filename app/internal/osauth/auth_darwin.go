//go:build darwin && !ios && cgo && !server

package osauth

/*
#cgo CFLAGS: -x objective-c -fobjc-arc
#cgo LDFLAGS: -framework Foundation -framework LocalAuthentication
#include <stdlib.h>
void *KTAuthBegin(const char *reason);
int KTAuthStatus(void *task);
void KTAuthEnd(void *task);
*/
import "C"

import (
	"context"
	"time"
	"unsafe"
)

func Available() bool { return true }
func authenticatePlatform(ctx context.Context, reason string, _ uintptr, _ string) error {
	text := C.CString(reason)
	defer C.free(unsafe.Pointer(text))
	task := C.KTAuthBegin(text)
	if task == nil {
		return ErrUnavailable
	}
	defer C.KTAuthEnd(task)
	ticker := time.NewTicker(20 * time.Millisecond)
	defer ticker.Stop()
	for {
		switch C.KTAuthStatus(task) {
		case 0:
			return nil
		case 1:
			return ErrCancelled
		case 2:
			return ErrUnavailable
		case 3:
			return ErrFailed
		}
		select {
		case <-ctx.Done():
			return ErrCancelled
		case <-ticker.C:
		}
	}
}
