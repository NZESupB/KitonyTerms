//go:build !android && !ios

package platform

import (
	"os"
	"path/filepath"
	"testing"
)

func TestLegacyDirectoryContract(t *testing.T) {
	home := t.TempDir()
	base := filepath.Join(home, "config-base")
	empty := func(string) string { return "" }
	mac := filepath.Join(home, "Library", "Application Support", "com.kitony.KitonyTerms")
	for _, row := range []struct {
		system string
		want   Paths
	}{
		{"darwin", Paths{Config: mac, Data: mac}},
		{"windows", PathsForRoot(filepath.Join(base, "kitony", "KitonyTerms"))},
		{"linux", Paths{Config: filepath.Join(base, "kitonyterms"), Data: filepath.Join(home, ".local", "share", "kitonyterms")}},
	} {
		got, err := legacyPaths(row.system, home, base, empty)
		if err != nil || got != row.want {
			t.Fatalf("%s: %#v, %v", row.system, got, err)
		}
	}
	for _, value := range []string{"relative", filepath.Join(home, "xdg-data")} {
		got, err := legacyPaths("linux", home, base, func(string) string { return value })
		want := filepath.Join(home, ".local", "share", "kitonyterms")
		if filepath.IsAbs(value) {
			want = filepath.Join(value, "kitonyterms")
		}
		if err != nil || got.Data != want {
			t.Fatalf("XDG: %#v %v", got, err)
		}
	}
	if _, err := legacyPaths("unknown", home, base, empty); err == nil {
		t.Fatal("接受未知平台")
	}
	entries, err := os.ReadDir(home)
	if err != nil || len(entries) != 0 {
		t.Fatal("路径计算创建了目录", err)
	}
}

func TestExplicitDirectoryIsolation(t *testing.T) {
	root := filepath.Join(t.TempDir(), "isolated")
	t.Setenv("KITONYTERMS_DATA_DIR", root)
	got, err := Discover()
	if err != nil || got != PathsForRoot(root) {
		t.Fatal(got, err)
	}
	for _, dir := range []string{got.Config, got.Data} {
		if info, err := os.Stat(dir); err != nil || !info.IsDir() {
			t.Fatal("未创建隔离目录", err)
		}
	}
}
