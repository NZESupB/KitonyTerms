package main

import (
	"context"
	"debug/macho"
	"encoding/binary"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

func actualCandidate(t *testing.T) (string, string) {
	t.Helper()
	if runtime.GOOS != "darwin" || runtime.GOARCH != "arm64" && runtime.GOARCH != "amd64" {
		t.Skip("实际候选回读需要macOS")
	}
	candidate := os.Getenv("KITONYTERMS_MACOS_CANDIDATE")
	if candidate == "" {
		t.Skip("未提供实际候选；不计作原生包验证")
	}
	project, err := filepath.Abs("../..")
	if err != nil {
		t.Fatal(err)
	}
	project, err = filepath.EvalSymlinks(project)
	if err != nil {
		t.Fatal(err)
	}
	return candidate, project
}

func TestActualCandidateIdentityAndTamper(t *testing.T) {
	candidate, project := actualCandidate(t)
	identityJSON, err := readBounded(filepath.Join(candidate, "identity.json"), 4096)
	if err != nil {
		t.Fatal(err)
	}
	info, err := parseIdentity(identityJSON)
	if err != nil {
		t.Fatal(err)
	}
	record, err := verifyCandidate(context.Background(), project, candidate, info)
	if err != nil || !record.RuntimeIdentityVerified || !record.DMGReadbackVerified || record.MinimumOSExecuted || record.FormalSigningVerified {
		t.Fatal("实际候选完整验证失败或越界声明", err)
	}
	if _, err = checkBinary(filepath.Join(candidate, "payload", bundleName, executableName)); err != nil {
		t.Fatal(err)
	}
	wrong := info
	wrong.Build++
	if _, err = verifyCandidate(context.Background(), project, candidate, wrong); err == nil {
		t.Fatal("完整构建身份不符应拒绝")
	}
	copy := t.TempDir()
	if err = os.CopyFS(copy, os.DirFS(candidate)); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"payload/" + bundleName + "/" + iconName, "payload/" + bundleName + "/" + plistName, dmgName, "build-inputs/main.go"} {
		t.Run(name, func(t *testing.T) {
			path := filepath.Join(copy, name)
			original, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			changed := append([]byte(nil), original...)
			changed[len(changed)-1] ^= 1
			if err = os.WriteFile(path, changed, 0600); err != nil {
				t.Fatal(err)
			}
			defer func() {
				if err := os.WriteFile(path, original, 0600); err != nil {
					t.Fatal(err)
				}
			}()
			if _, err = verifyCandidate(context.Background(), project, copy, info); err == nil {
				t.Fatal("实际成品或输入篡改应拒绝")
			}
		})
	}
	t.Run("signature_and_plist_beyond_digest", func(t *testing.T) {
		path := filepath.Join(copy, "payload", bundleName, plistName)
		original, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(path, append(original, '\n'), 0600); err != nil {
			t.Fatal(err)
		}
		defer os.WriteFile(path, original, 0600)
		recordPath := filepath.Join(copy, recordName)
		originalRecord, err := os.ReadFile(recordPath)
		if err != nil {
			t.Fatal(err)
		}
		defer os.WriteFile(recordPath, originalRecord, 0600)
		var altered Record
		if err = decodeJSON(originalRecord, &altered); err != nil {
			t.Fatal(err)
		}
		altered.Files["payload/"+bundleName+"/"+plistName], err = digestFile(path)
		if err != nil {
			t.Fatal(err)
		}
		encoded, _ := json.Marshal(altered)
		if err = os.WriteFile(recordPath, encoded, 0600); err != nil {
			t.Fatal(err)
		}
		if _, err = verifyCandidate(context.Background(), project, copy, info); err == nil {
			t.Fatal("修改摘要也不能绕过plist及签名验证")
		}
	})
	t.Run("additional_file", func(t *testing.T) {
		path := filepath.Join(copy, "payload", bundleName, "Contents/Resources/extra")
		if err := os.WriteFile(path, []byte("extra"), 0600); err != nil {
			t.Fatal(err)
		}
		defer os.Remove(path)
		if _, err := verifyCandidate(context.Background(), project, copy, info); err == nil {
			t.Fatal("额外文件应拒绝")
		}
	})
	t.Run("macho_target_and_minimum", func(t *testing.T) {
		path := filepath.Join(copy, "payload", bundleName, executableName)
		original, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		defer os.WriteFile(path, original, 0700)
		changed := append([]byte(nil), original...)
		wrongCPU := macho.CpuAmd64
		if binary.LittleEndian.Uint32(original[4:8]) == uint32(wrongCPU) {
			wrongCPU = macho.CpuArm64
		}
		binary.LittleEndian.PutUint32(changed[4:8], uint32(wrongCPU))
		if err = os.WriteFile(path, changed, 0700); err != nil {
			t.Fatal(err)
		}
		if _, err = checkBinary(path); err == nil {
			t.Fatal("错误Mach-O架构应拒绝")
		}
		changed = append([]byte(nil), original...)
		ncommands := binary.LittleEndian.Uint32(changed[16:20])
		offset := 32
		found := false
		for i := uint32(0); i < ncommands; i++ {
			command := binary.LittleEndian.Uint32(changed[offset : offset+4])
			size := int(binary.LittleEndian.Uint32(changed[offset+4 : offset+8]))
			if command == 0x32 {
				binary.LittleEndian.PutUint32(changed[offset+12:offset+16], 13<<16)
				found = true
				break
			}
			offset += size
		}
		if !found {
			t.Fatal("实际候选应包含LC_BUILD_VERSION")
		}
		if err = os.WriteFile(path, changed, 0700); err != nil {
			t.Fatal(err)
		}
		if _, err = checkBinary(path); err == nil {
			t.Fatal("错误最低系统声明应拒绝")
		}
	})
}

func TestNativeDMGFailureAndCancellationCleanup(t *testing.T) {
	candidate, _ := actualCandidate(t)
	for _, scenario := range []string{"failure", "cancel", "pre_cancel"} {
		t.Run(scenario, func(t *testing.T) {
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			if scenario == "pre_cancel" {
				cancel()
			}
			sentinel := errors.New("有意拒绝本次包回读")
			visited := false
			err := withDMG(ctx, candidate, func(mount string) error {
				visited = true
				active, readonly, err := mountedReadOnly(mount, candidate)
				if err != nil || !active || !readonly {
					t.Fatal("必须实际只读挂载", err)
				}
				file, err := os.OpenFile(filepath.Join(mount, "write-must-fail"), os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
				if err == nil {
					file.Close()
					t.Fatal("只读镜像竟允许创建文件")
				}
				if scenario == "cancel" {
					cancel()
					return ctx.Err()
				}
				return sentinel
			})
			if scenario == "failure" && !errors.Is(err, sentinel) || scenario != "failure" && !errors.Is(err, context.Canceled) {
				t.Fatal("失败或取消根因不符", err)
			}
			if visited != (scenario != "pre_cancel") {
				t.Fatal("预取消不应进入镜像回读")
			}
			if _, err = os.Lstat(filepath.Join(candidate, ".dmg-mount")); !errors.Is(err, os.ErrNotExist) {
				t.Fatal("失败或取消必须卸载并回收本次挂载目录", err)
			}
		})
	}
	foreign := filepath.Join(candidate, ".dmg-mount")
	if err := os.Mkdir(foreign, 0700); err != nil {
		t.Fatal(err)
	}
	defer os.Remove(foreign)
	marker := filepath.Join(foreign, "foreign")
	if err := os.WriteFile(marker, []byte("保留"), 0600); err != nil {
		t.Fatal(err)
	}
	defer os.Remove(marker)
	if err := withDMG(context.Background(), candidate, func(string) error { return nil }); err == nil {
		t.Fatal("已有挂载路径应拒绝接管")
	}
	if data, err := os.ReadFile(marker); err != nil || string(data) != "保留" {
		t.Fatal("不能清理外来挂载路径", err)
	}
}
