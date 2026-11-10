//go:build darwin && !ios && cgo

package fonts

import (
	"context"
	"sync"
	"testing"
)

func TestCoreTextCatalog(t *testing.T) {
	var wg sync.WaitGroup
	for range 4 {
		wg.Go(func() {
			catalog, err := List(context.Background(), false)
			if err != nil || catalog.Source != "system" || len(catalog.Families) <= len(fallbackFamilies) {
				t.Errorf("count=%d source=%s err=%v", len(catalog.Families), catalog.Source, err)
				return
			}
			t.Logf("实际CoreText目录：%d族（含兼容回退）", len(catalog.Families))
		})
	}
	wg.Wait()
}
