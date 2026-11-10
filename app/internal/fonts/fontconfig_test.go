//go:build !windows

package fonts

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestFontconfigHelper(t *testing.T) {
	mode := os.Getenv("KITONYTERMS_FONT_TEST_MODE")
	if mode == "" {
		return
	}
	if os.Args[len(os.Args)-1] != "--format=%{family[0]}\\n" {
		os.Exit(9)
	}
	switch mode {
	case "normal":
		fmt.Print("中文,族名\nSource Code Pro\nArial\n")
	case "failure":
		fmt.Fprint(os.Stderr, "私有字体路径")
		os.Exit(7)
	case "stdout":
		_, _ = os.Stdout.Write(bytes.Repeat([]byte("x"), 5<<20))
	case "stderr":
		_, _ = os.Stderr.Write(bytes.Repeat([]byte("x"), 17<<10))
	case "invalid":
		_, _ = os.Stdout.Write([]byte{255})
	case "cancel":
		time.Sleep(30 * time.Second)
	}
	os.Exit(0)
}

func TestFontconfigBoundaries(t *testing.T) {
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	program := filepath.Join(t.TempDir(), "font fixture")
	script := "#!/bin/sh\nexec '" + strings.ReplaceAll(executable, "'", "'\\''") + "' -test.run=^TestFontconfigHelper$ -- \"$@\"\n"
	if err := os.WriteFile(program, []byte(script), 0700); err != nil {
		t.Fatal(err)
	}
	for _, mode := range []string{"normal", "failure", "stdout", "stderr", "invalid", "cancel"} {
		t.Run(mode, func(t *testing.T) {
			t.Setenv("KITONYTERMS_FONT_TEST_MODE", mode)
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			if mode == "cancel" {
				cancel()
				ctx, cancel = context.WithTimeout(context.Background(), 50*time.Millisecond)
			}
			defer cancel()
			start := time.Now()
			names, err := fontconfigFamilies(ctx, program)
			if mode == "normal" {
				if err != nil || len(names) != 4 || names[0] != "中文,族名" {
					t.Fatalf("%q %v", names, err)
				}
			} else if mode == "cancel" {
				if !errors.Is(err, context.DeadlineExceeded) || time.Since(start) > time.Second {
					t.Fatal(err)
				}
			} else if !errors.Is(err, errEnumeration) {
				t.Fatalf("错误未分类：%v", err)
			}
		})
	}
	t.Setenv("PATH", t.TempDir())
	if _, err := fontconfigFamilies(context.Background(), "kitonyterms-no-fc-list"); !errors.Is(err, errUnavailable) {
		t.Fatal(err)
	}
}

func TestInstalledFontconfigAdapter(t *testing.T) {
	program, err := exec.LookPath("fc-list")
	if err != nil {
		t.Skip("本机没有Fontconfig")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	names, err := fontconfigFamilies(ctx, program)
	if err != nil || len(names) < 2 || strings.Contains(names[0], "\\n") {
		t.Fatalf("count=%d err=%v", len(names), err)
	}
	t.Logf("实际Fontconfig记录数：%d；不是Linux原生运行证据", len(names))
}
