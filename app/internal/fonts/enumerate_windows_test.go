//go:build windows

package fonts

import (
	"context"
	"testing"
	"unsafe"
)

func TestWindowsFontLayout(t *testing.T) {
	var font logFontW
	if unsafe.Sizeof(font) != 92 || unsafe.Offsetof(font.FaceName) != 28 {
		t.Fatal("LOGFONTW布局不匹配")
	}
}

func TestWindowsFontCatalog(t *testing.T) {
	catalog, err := List(context.Background(), false)
	if err != nil || catalog.Source != "system" || len(catalog.Families) < 7 {
		t.Fatalf("count=%d source=%s err=%v", len(catalog.Families), catalog.Source, err)
	}
}
