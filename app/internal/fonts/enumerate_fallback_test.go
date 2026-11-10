//go:build ios || android || (darwin && !cgo) || (!darwin && !linux && !windows)

package fonts

import (
	"context"
	"testing"
)

func TestPlatformFallback(t *testing.T) {
	catalog, err := List(context.Background(), false)
	if err != nil || catalog.Source != "fallback" || catalog.Reason != "unavailable" || len(catalog.Families) != 7 {
		t.Fatalf("%+v %v", catalog, err)
	}
}
