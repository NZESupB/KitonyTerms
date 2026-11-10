//go:build darwin && !ios && cgo

package fonts

/*
#cgo LDFLAGS: -framework CoreText -framework CoreFoundation
#include <CoreText/CoreText.h>
#include <CoreFoundation/CoreFoundation.h>

static Boolean KTFontFamilyAt(CFArrayRef fonts, CFIndex index, char *buffer, CFIndex size) {
    CFTypeRef value = CFArrayGetValueAtIndex(fonts, index);
    if (value == NULL || CFGetTypeID(value) != CFStringGetTypeID()) return false;
    return CFStringGetCString((CFStringRef)value, buffer, size, kCFStringEncodingUTF8);
}
*/
import "C"

import (
	"context"
	"unsafe"
)

func enumerate(ctx context.Context) ([]string, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	fonts := C.CTFontManagerCopyAvailableFontFamilyNames()
	if fonts == 0 {
		return nil, errEnumeration
	}
	defer C.CFRelease(C.CFTypeRef(fonts))
	count := C.CFArrayGetCount(fonts)
	if count < 0 || count > maxRecords {
		return nil, errEnumeration
	}
	names := make([]string, 0, int(count))
	for index := C.CFIndex(0); index < count; index++ {
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		var buffer [maxFamilyBytes + 1]byte
		if C.KTFontFamilyAt(fonts, index, (*C.char)(unsafe.Pointer(&buffer[0])), C.CFIndex(len(buffer))) != 0 {
			names = append(names, C.GoString((*C.char)(unsafe.Pointer(&buffer[0]))))
		}
	}
	return names, nil
}
