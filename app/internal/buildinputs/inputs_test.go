package buildinputs

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"testing"
)

func fixture(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	for name, value := range map[string]string{"go.mod": "module kitonyterms\n", "go.sum": "", "main.go": "package main\n", "internal/a/a.go": "package a\n", "frontend/dist/index.html": "public", "build/darwin/candidate.icns": "public icon", "build/windows/candidate.ico": "excluded"} {
		path := filepath.Join(root, name)
		if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(value), 0600); err != nil {
			t.Fatal(err)
		}
	}
	return root
}

func TestCallerResourcesAndValidation(t *testing.T) {
	project := fixture(t)
	resource := []string{"build/darwin/candidate.icns"}
	target := filepath.Join(t.TempDir(), "copy")
	inputs, err := Copy(context.Background(), project, target, resource)
	if err != nil || len(inputs) != 6 {
		t.Fatal("macOS固定资源复制失败", len(inputs), err)
	}
	if _, exists := inputs["build/windows/candidate.ico"]; exists {
		t.Fatal("不能自动复制其它平台资源")
	}
	if err = Verify(context.Background(), target, resource, inputs); err != nil {
		t.Fatal(err)
	}
	for _, invalid := range [][]string{{"../outside"}, {"main.go"}, {"/build/absolute"}, {"build/a\\b"}, {"build/darwin/candidate.icns", "build/darwin/candidate.icns"}} {
		output := filepath.Join(t.TempDir(), "refused")
		if _, err = Copy(context.Background(), project, output, invalid); err == nil {
			t.Fatal("非法调用方资源应拒绝")
		}
		if _, err = os.Lstat(output); !errors.Is(err, os.ErrNotExist) {
			t.Fatal("非法资源不应创建目标", err)
		}
	}
}

func TestAliasedOutputOverlap(t *testing.T) {
	project := fixture(t)
	alias := filepath.Join(t.TempDir(), "alias")
	if err := os.Symlink(project, alias); err != nil {
		t.Skip("创建链接权限不可用；不计为已执行")
	}
	if _, err := Copy(context.Background(), project, filepath.Join(alias, "private"), []string{"build/darwin/candidate.icns"}); err == nil {
		t.Fatal("别名父目录指回工程时应拒绝")
	}
	if _, err := os.Lstat(filepath.Join(project, "private")); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("重叠拒绝不能写入工程")
	}
}
