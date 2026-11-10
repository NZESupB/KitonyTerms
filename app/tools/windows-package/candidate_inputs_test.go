package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func inputFixture(t *testing.T) string {
	t.Helper()
	project := resourceProject(t)
	for name, value := range map[string]string{
		"go.mod": "module kitonyterms\n\ngo 1.26.0\n", "go.sum": "",
		"main.go": "package main\nfunc main() {}\n", "native_desktop.go": "package main\n",
		"internal/fixture/fixture.go": "package fixture\n", "internal/fixture/native.c": "// public fixture\n",
		"frontend/dist/index.html": "public fixture", "frontend/dist/.embedded": "embedded hidden asset",
		"main_test.go": "exclude", "internal/fixture/fixture_test.go": "exclude",
		".local-data/data/value": "exclude", ".git/config": "exclude", "frontend/node_modules/pkg/value": "exclude", "tools/fixture.go": "exclude",
	} {
		fixtureFile(t, project, name, []byte(value))
	}
	return project
}

func fixtureFile(t *testing.T, project, name string, value []byte) {
	t.Helper()
	path := filepath.Join(project, name)
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, value, 0600); err != nil {
		t.Fatal(err)
	}
}

func TestBuildInputsIsolationAndRevalidation(t *testing.T) {
	project := inputFixture(t)
	target := filepath.Join(t.TempDir(), "private")
	inputs, err := copyBuildInputs(context.Background(), project, target)
	if err != nil {
		t.Fatal(err)
	}
	if len(inputs) != 10 {
		t.Fatal("构建输入集合不符", len(inputs))
	}
	for name := range inputs {
		if filepath.IsAbs(name) || strings.Contains(name, "_test.go") || strings.Contains(name, "node_modules") || strings.Contains(name, ".local-data") {
			t.Fatal("仅允许相对构建输入")
		}
		original, err := os.ReadFile(filepath.Join(project, name))
		if err != nil {
			t.Fatal(err)
		}
		copied, err := os.ReadFile(filepath.Join(target, name))
		if err != nil || !bytes.Equal(original, copied) {
			t.Fatal("副本字节不符", err)
		}
	}
	for _, name := range []string{".local-data", ".git", "tools", "frontend/node_modules", "main_test.go", "internal/fixture/fixture_test.go"} {
		if _, err := os.Lstat(filepath.Join(target, name)); !errors.Is(err, os.ErrNotExist) {
			t.Fatal("无关数据不能复制", name, err)
		}
	}
	if runtime.GOOS != "windows" {
		err := filepath.WalkDir(target, func(path string, entry os.DirEntry, err error) error {
			if err != nil {
				return err
			}
			info, err := entry.Info()
			if err != nil {
				return err
			}
			want := os.FileMode(0600)
			if entry.IsDir() {
				want = 0700
			}
			if info.Mode().Perm() != want {
				return errors.New("副本权限不私有")
			}
			return nil
		})
		if err != nil {
			t.Fatal(err)
		}
	}
	if err = verifyBuildInputs(context.Background(), project, inputs); err != nil {
		t.Fatal(err)
	}
	if err = verifyBuildInputs(context.Background(), target, inputs); err != nil {
		t.Fatal(err)
	}
	fixtureFile(t, project, "main.go", []byte("changed source"))
	if err = verifyBuildInputs(context.Background(), project, inputs); err == nil {
		t.Fatal("源内容变化不能发布成功")
	}
	fixtureFile(t, target, "internal/fixture/added.go", []byte("package fixture\n"))
	if err = verifyBuildInputs(context.Background(), target, inputs); err == nil {
		t.Fatal("副本文件集合变化应拒绝")
	}
	if _, err = copyBuildInputs(context.Background(), project, target); err == nil {
		t.Fatal("已有副本不能覆盖")
	}
}

func TestBuildInputsRejectLinksAndOverlap(t *testing.T) {
	for _, name := range []string{"main.go", "internal", "frontend", "build/windows"} {
		t.Run(name, func(t *testing.T) {
			project := inputFixture(t)
			path := filepath.Join(project, name)
			moved := path + "-original"
			if err := os.Rename(path, moved); err != nil {
				t.Fatal(err)
			}
			if err := os.Symlink(moved, path); err != nil {
				if runtime.GOOS == "windows" {
					t.Skip("Windows创建链接需要原生权限；本机不计此项通过")
				}
				t.Fatal(err)
			}
			if _, err := copyBuildInputs(context.Background(), project, filepath.Join(t.TempDir(), "private")); err == nil {
				t.Fatal("输入及中间目录链接应拒绝")
			}
		})
	}
	project := inputFixture(t)
	target := filepath.Join(project, "internal/private")
	if _, err := copyBuildInputs(context.Background(), project, target); err == nil {
		t.Fatal("重叠目标应拒绝")
	}
	if _, err := os.Stat(target); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("重叠拒绝不能创建目标")
	}
}

func TestBuildInputsBounds(t *testing.T) {
	for _, scenario := range []string{"file", "total", "nodes", "depth"} {
		t.Run(scenario, func(t *testing.T) {
			project := inputFixture(t)
			switch scenario {
			case "file", "total":
				count, size := 1, int64(maxBuildInputFileBytes+1)
				if scenario == "total" {
					count, size = 4, maxBuildInputFileBytes
				}
				for i := 0; i < count; i++ {
					file, err := os.Create(filepath.Join(project, "internal", fmt.Sprintf("large-%d", i)))
					if err != nil {
						t.Fatal(err)
					}
					if err = errors.Join(file.Truncate(size), file.Close()); err != nil {
						t.Fatal(err)
					}
				}
			case "nodes":
				for i := 0; i <= maxBuildInputNodes; i++ {
					if err := os.Mkdir(filepath.Join(project, "internal", fmt.Sprintf("empty-%d", i)), 0700); err != nil {
						t.Fatal(err)
					}
				}
			case "depth":
				if err := os.MkdirAll(filepath.Join(project, "internal", strings.Repeat("d/", maxBuildInputDepth+1)), 0700); err != nil {
					t.Fatal(err)
				}
			}
			if _, err := scanBuildInputs(context.Background(), project, nil); err == nil {
				t.Fatal("构建输入超限应拒绝")
			}
		})
	}
}

func TestBuildInputsCancellationAndMutation(t *testing.T) {
	project := inputFixture(t)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	target := filepath.Join(t.TempDir(), "private")
	if _, err := copyBuildInputs(ctx, project, target); !errors.Is(err, context.Canceled) {
		t.Fatal("已取消请求应拒绝", err)
	}
	if _, err := os.Stat(target); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("预取消不能创建目标")
	}
	ctx, cancel = context.WithCancel(context.Background())
	defer cancel()
	if _, err := scanBuildInputs(ctx, project, func(string, []byte) error { cancel(); return nil }); !errors.Is(err, context.Canceled) {
		t.Fatal("实际扫描应观察取消", err)
	}
	if _, err := scanBuildInputs(context.Background(), project, func(name string, _ []byte) error {
		if name == "main.go" {
			fixtureFile(t, project, "added.go", []byte("package main\n"))
		}
		return nil
	}); err == nil {
		t.Fatal("目录扫描期间新增文件应拒绝")
	}
}
