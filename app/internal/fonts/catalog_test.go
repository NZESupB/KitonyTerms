package fonts

import (
	"context"
	"errors"
	"fmt"
	"io"
	"reflect"
	"strings"
	"testing"
)

func TestNormalizeFamilies(t *testing.T) {
	got, err := normalize([]string{" Arial ", "Menlo", "Arial", "@Vertical", "", "bad\nname", string([]byte{255}), strings.Repeat("x", 513), "中文,字体 \"A\"", "Source Code Pro"})
	want := []string{"Menlo", "Source Code Pro", "Arial", "中文,字体 \"A\""}
	if err != nil || !reflect.DeepEqual(got, want) {
		t.Fatalf("families=%q err=%v", got, err)
	}
}

func TestCatalogSourcesAndCancellation(t *testing.T) {
	for _, test := range []struct {
		name, reason string
		names        []string
		err          error
	}{
		{"system", "", []string{"Arial", "Menlo"}, nil},
		{"missing", "unavailable", nil, errUnavailable},
		{"failed", "enumeration_failed", nil, errors.New("私有原始错误")},
		{"empty", "empty", []string{"", "\x00"}, nil},
	} {
		t.Run(test.name, func(t *testing.T) {
			catalog, err := list(context.Background(), false, func(context.Context) ([]string, error) { return test.names, test.err })
			if err != nil || catalog.Reason != test.reason || len(catalog.Families) < 7 {
				t.Fatalf("catalog=%+v err=%v", catalog, err)
			}
			if (catalog.Source == "system") != (test.reason == "") {
				t.Fatalf("来源错误：%s", catalog.Source)
			}
		})
	}
	ctx, cancel := context.WithCancel(context.Background())
	_, err := list(ctx, false, func(context.Context) ([]string, error) { cancel(); return []string{"Arial"}, nil })
	if !errors.Is(err, context.Canceled) {
		t.Fatal(err)
	}
	_, err = list(ctx, true, func(context.Context) ([]string, error) { t.Fatal("取消后不能枚举"); return nil, nil })
	if !errors.Is(err, context.Canceled) {
		t.Fatal(err)
	}
}

func TestMobileFallbackDoesNotScanOrShareStorage(t *testing.T) {
	catalog, err := list(context.Background(), true, func(context.Context) ([]string, error) { t.Fatal("移动端不枚举"); return nil, nil })
	if err != nil || catalog.Source != "fallback" || catalog.Reason != "mobile" || !reflect.DeepEqual(catalog.Families, fallbackFamilies) {
		t.Fatalf("%+v %v", catalog, err)
	}
	catalog.Families[0] = "changed"
	if fallbackFamilies[0] != "Menlo" {
		t.Fatal("回退共享了可变切片")
	}
}

func TestCatalogLimitsIncludeFallbacks(t *testing.T) {
	names := make([]string, maxFamilies)
	for i := range names {
		names[i] = fmt.Sprintf("Font%05d", i)
	}
	catalog, err := list(context.Background(), false, func(context.Context) ([]string, error) { return names, nil })
	if err != nil || catalog.Source != "fallback" || catalog.Reason != "enumeration_failed" {
		t.Fatalf("%+v %v", catalog, err)
	}
	if _, err := normalize(make([]string, maxRecords+1)); !errors.Is(err, errEnumeration) {
		t.Fatal(err)
	}
}

func TestLimitedOutputCannotBypassWrite(t *testing.T) {
	out := &limitedOutput{limit: 16}
	_, err := io.Copy(out, strings.NewReader(strings.Repeat("x", 17)))
	if !errors.Is(err, errEnumeration) || len(out.Bytes()) > 16 {
		t.Fatalf("len=%d err=%v", len(out.Bytes()), err)
	}
}
