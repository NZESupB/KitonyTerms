// Package fonts提供有界的本机字体族发现，不读取字体文件内容。
package fonts

import (
	"context"
	"errors"
	"sort"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	"kitonyterms/internal/model"
)

const maxFamilies = 16384
const maxRecords = 65536
const maxFamilyBytes = 512

var errUnavailable = errors.New("系统字体枚举不可用")
var errEnumeration = errors.New("系统字体枚举失败")
var fallbackFamilies = []string{"Menlo", "SF Mono", "Consolas", "DejaVu Sans Mono", "Courier New", "Monaco", "monospace"}

func List(ctx context.Context, mobile bool) (model.FontCatalog, error) {
	return list(ctx, mobile, enumerate)
}

func list(parent context.Context, mobile bool, scan func(context.Context) ([]string, error)) (model.FontCatalog, error) {
	if err := parent.Err(); err != nil {
		return model.FontCatalog{}, err
	}
	fallback := func(reason string) model.FontCatalog {
		return model.FontCatalog{Families: append([]string(nil), fallbackFamilies...), Source: "fallback", Reason: reason}
	}
	if mobile {
		return fallback("mobile"), nil
	}
	ctx, cancel := context.WithTimeout(parent, 5*time.Second)
	defer cancel()
	names, err := scan(ctx)
	if parent.Err() != nil {
		return model.FontCatalog{}, parent.Err()
	}
	if err == nil {
		err = ctx.Err()
	}
	if errors.Is(err, errUnavailable) {
		return fallback("unavailable"), nil
	}
	if err != nil {
		return fallback("enumeration_failed"), nil
	}
	names, err = normalize(names)
	if err != nil {
		return fallback("enumeration_failed"), nil
	}
	if len(names) == 0 {
		return fallback("empty"), nil
	}
	// 等宽判断只用于兼容原排序，回退名不代表设备上已安装。
	names, err = normalize(append(names, fallbackFamilies...))
	if err != nil {
		return fallback("enumeration_failed"), nil
	}
	return model.FontCatalog{Families: names, Source: "system"}, nil
}

func normalize(names []string) ([]string, error) {
	if len(names) > maxRecords {
		return nil, errEnumeration
	}
	seen := make(map[string]struct{})
	result := make([]string, 0, len(names))
	for _, name := range names {
		name = strings.TrimSpace(name)
		if name == "" || strings.HasPrefix(name, "@") || len(name) > maxFamilyBytes || !utf8.ValidString(name) || strings.ContainsFunc(name, unicode.IsControl) {
			continue
		}
		if _, exists := seen[name]; exists {
			continue
		}
		seen[name] = struct{}{}
		result = append(result, name)
		if len(result) > maxFamilies {
			return nil, errEnumeration
		}
	}
	sort.Slice(result, func(i, j int) bool {
		a, b := strings.ToLower(result[i]), strings.ToLower(result[j])
		am, bm := looksMonospace(a), looksMonospace(b)
		if am != bm {
			return am
		}
		if a == b {
			return result[i] < result[j]
		}
		return a < b
	})
	return result, nil
}

func looksMonospace(name string) bool {
	for _, part := range []string{"mono", "code", "courier", "console", "menlo", "monaco"} {
		if strings.Contains(name, part) {
			return true
		}
	}
	return false
}
