//go:build linux && !android

package fonts

import "context"

func enumerate(ctx context.Context) ([]string, error) { return fontconfigFamilies(ctx, "fc-list") }
