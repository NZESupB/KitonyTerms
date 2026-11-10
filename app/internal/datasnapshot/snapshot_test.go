package datasnapshot

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"testing"

	"github.com/gofrs/flock"
	"kitonyterms/internal/model"
	"kitonyterms/internal/storage"
)

func write(t *testing.T, path string, value []byte) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, value, 0600); err != nil {
		t.Fatal(err)
	}
}
func fixture(t *testing.T) (string, string) {
	t.Helper()
	root := t.TempDir()
	config, data := filepath.Join(root, "config"), filepath.Join(root, "data")
	for source, target := range map[string]string{"rust-config.toml": "config/config.toml", "rust-v1.vault": "data/secrets.vault"} {
		value, err := os.ReadFile(filepath.Join("../storage/testdata", source))
		if err != nil {
			t.Fatal(err)
		}
		write(t, filepath.Join(root, target), value)
	}
	write(t, filepath.Join(data, "secrets.vault.key"), []byte("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n"))
	for _, name := range lockNames {
		write(t, filepath.Join(data, filepath.FromSlash(name)), nil)
	}
	write(t, filepath.Join(data, "sftp-edit/edit-fixture/file📄.txt"), []byte("尚未上传，必须保留\n"))
	write(t, filepath.Join(data, "updates/preferences.json"), []byte(`{"proxy":{"kind":"direct"}}`))
	write(t, filepath.Join(config, "future.bin"), []byte{0, 1, 2, 255})
	if err := os.MkdirAll(filepath.Join(data, "empty"), 0700); err != nil {
		t.Fatal(err)
	}
	repo, err := storage.OpenRepository(config, data)
	if err != nil {
		t.Fatal(err)
	}
	if err = repo.TrustHost("test.invalid", 22, "SHA256:public-fixture"); err != nil {
		t.Fatal(err)
	}
	return config, data
}
func inventory(t *testing.T, config, data string) []Entry {
	t.Helper()
	roots, _, err := openSources(config, data)
	if err != nil {
		t.Fatal(err)
	}
	defer closeRoots(roots)
	entries, err := scan(context.Background(), roots, true)
	if err != nil {
		t.Fatal(err)
	}
	return entries
}
func TestCompleteDirectoryRustGoRoundtrip(t *testing.T) {
	config, data := fixture(t)
	ctx := context.Background()
	before := inventory(t, config, data)
	write(t, filepath.Join(data, "updates/journal.json"), []byte(`{"foreignPath":"/isolated/original","stage":"prepared"}`))
	before = inventory(t, config, data)
	backup := filepath.Join(t.TempDir(), "snapshot")
	result, err := Create(ctx, config, data, backup)
	if err != nil {
		t.Fatal(err)
	}
	if !result.PendingUpdate || result.Files < 7 {
		t.Fatal("未记录完整数据及未完成更新")
	}
	verified, err := Verify(ctx, backup)
	if err != nil || verified != result {
		t.Fatal("快照回读不一致", err)
	}
	restored := filepath.Join(t.TempDir(), "restored")
	if _, err = Restore(ctx, backup, restored); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(before, inventory(t, filepath.Join(restored, "config"), filepath.Join(restored, "data"))) {
		t.Fatal("恢复内容不完整")
	}
	if !reflect.DeepEqual(before, inventory(t, config, data)) {
		t.Fatal("源目录被修改")
	}
	for _, name := range lockNames {
		if _, err = os.Stat(filepath.Join(restored, "data", filepath.FromSlash(name))); !errors.Is(err, os.ErrNotExist) {
			t.Fatal("恢复不应复用运行时锁")
		}
	}
	repo, err := storage.OpenRepository(filepath.Join(restored, "config"), filepath.Join(restored, "data"))
	if err != nil {
		t.Fatal(err)
	}
	snapshot := repo.Snapshot()
	snapshot.Config.Settings.FontSize = 15
	if _, err = repo.SaveSettings(ctx, model.SaveSettingsRequest{ExpectedRevision: snapshot.Revision, Settings: snapshot.Config.Settings}); err != nil {
		t.Fatal(err)
	}
	if _, trusted := repo.CheckHost("test.invalid", 22, "SHA256:public-fixture"); !trusted {
		t.Fatal("恢复后信任丢失")
	}
	vault, err := storage.OpenVault(filepath.Join(restored, "data"))
	if err != nil {
		t.Fatal(err)
	}
	if value, ok := vault.Get("tester@test.invalid:22"); !ok || value != "interop-test-value" {
		t.Fatal("恢复后vault不可读")
	}
	if err = vault.Set("go-write", "go-roundtrip"); err != nil {
		t.Fatal(err)
	}
	vault.Close()
	if runtime.GOOS != "windows" {
		for _, path := range []string{backup, filepath.Join(backup, "manifest.json"), filepath.Join(restored, "data/secrets.vault.key")} {
			info, err := os.Stat(path)
			if err != nil {
				t.Fatal(err)
			}
			want := os.FileMode(0600)
			if info.IsDir() {
				want = 0700
			}
			if info.Mode().Perm() != want {
				t.Fatal("私有权限不正确")
			}
		}
	}
}
func TestSnapshotRejectsHeldLocksAndMissingLock(t *testing.T) {
	for _, name := range lockNames {
		t.Run(name, func(t *testing.T) {
			config, data := fixture(t)
			lock := flock.New(filepath.Join(data, filepath.FromSlash(name)), flock.SetFlag(os.O_RDWR))
			ok, err := lock.TryLock()
			if err != nil || !ok {
				t.Fatal("夹具锁失败")
			}
			defer lock.Close()
			out := filepath.Join(t.TempDir(), "not-created")
			if _, err = Create(context.Background(), config, data, out); err == nil {
				t.Fatal("活跃锁应拒绝")
			}
			if _, err = os.Stat(out); !errors.Is(err, os.ErrNotExist) {
				t.Fatal("锁竞争时不应创建目标")
			}
		})
	}
	config, data := fixture(t)
	if err := os.Remove(filepath.Join(data, "kitonyterms.lock")); err != nil {
		t.Fatal(err)
	}
	if _, err := Create(context.Background(), config, data, filepath.Join(t.TempDir(), "missing")); err == nil {
		t.Fatal("缺失应用锁应拒绝")
	}
	if _, err := os.Stat(filepath.Join(data, "kitonyterms.lock")); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("不得创建源锁")
	}
}
func TestSnapshotRefusesExistingAndOverlappingTargets(t *testing.T) {
	config, data := fixture(t)
	ctx := context.Background()
	existing := t.TempDir()
	write(t, filepath.Join(existing, "keep"), []byte("keep"))
	for _, target := range []string{existing, config, filepath.Join(data, "nested"), filepath.Dir(data)} {
		if _, err := Create(ctx, config, data, target); err == nil {
			t.Fatal("应拒绝覆盖或嵌套目标")
		}
	}
	backup := filepath.Join(t.TempDir(), "snapshot")
	if _, err := Create(ctx, config, data, backup); err != nil {
		t.Fatal(err)
	}
	if _, err := Restore(ctx, backup, existing); err == nil {
		t.Fatal("恢复不能覆盖已有新数据")
	}
	if value, err := os.ReadFile(filepath.Join(existing, "keep")); err != nil || string(value) != "keep" {
		t.Fatal("原目标改变")
	}
}
func TestSnapshotRejectsTamperExtraAndIncomplete(t *testing.T) {
	for _, kind := range []string{"bytes", "extra", "missing", "manifest", "trailing", "unknown"} {
		t.Run(kind, func(t *testing.T) {
			config, data := fixture(t)
			ctx := context.Background()
			backup := filepath.Join(t.TempDir(), "snapshot")
			if _, err := Create(ctx, config, data, backup); err != nil {
				t.Fatal(err)
			}
			switch kind {
			case "bytes":
				write(t, filepath.Join(backup, "data/secrets.vault"), []byte("damaged"))
			case "extra":
				write(t, filepath.Join(backup, "extra"), nil)
			case "missing":
				os.Remove(filepath.Join(backup, "config/future.bin"))
			case "manifest":
				os.Remove(filepath.Join(backup, manifestName))
			case "trailing":
				file, err := os.OpenFile(filepath.Join(backup, manifestName), os.O_APPEND|os.O_WRONLY, 0600)
				if err != nil {
					t.Fatal(err)
				}
				file.WriteString("{}")
				file.Close()
			case "unknown":
				write(t, filepath.Join(backup, manifestName), []byte(`{"version":1,"entries":[],"unknown":true}`))
			}
			if _, err := Verify(ctx, backup); err == nil {
				t.Fatal("损坏应拒绝")
			}
			out := filepath.Join(t.TempDir(), "not-created")
			if _, err := Restore(ctx, backup, out); err == nil {
				t.Fatal("损坏快照不能恢复")
			}
			if _, err := os.Stat(out); !errors.Is(err, os.ErrNotExist) {
				t.Fatal("预检失败不应创建目标")
			}
		})
	}
}

type checkingContext struct {
	context.Context
	check func()
}

func (c checkingContext) Err() error { c.check(); return c.Context.Err() }
func TestSnapshotCancellationAndMutationKeepIncompleteMaterial(t *testing.T) {
	for _, mode := range []string{"cancel", "mutation"} {
		t.Run(mode, func(t *testing.T) {
			config, data := fixture(t)
			out := filepath.Join(t.TempDir(), "incomplete")
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			changed := false
			checked := checkingContext{Context: ctx, check: func() {
				if changed {
					return
				}
				if _, err := os.Stat(out); err == nil {
					changed = true
					if mode == "cancel" {
						cancel()
					} else {
						write(t, filepath.Join(config, "future.bin"), []byte("changed"))
					}
				}
			}}
			_, err := Create(checked, config, data, out)
			if err == nil || !changed {
				t.Fatal("故障未发生")
			}
			if mode == "cancel" && !errors.Is(err, context.Canceled) {
				t.Fatal("取消原因丢失")
			}
			if _, err = os.Stat(out); err != nil {
				t.Fatal("应保留未完成目标")
			}
			if _, err = os.Stat(filepath.Join(out, manifestName)); !errors.Is(err, os.ErrNotExist) {
				t.Fatal("失败不得发布完成清单")
			}
			if _, err = Verify(context.Background(), out); err == nil {
				t.Fatal("不完整目标不得校验成功")
			}
		})
	}
}
func TestSnapshotRejectsLinksNamesAndBounds(t *testing.T) {
	for _, kind := range []string{"symlink", "unsafe", "size", "depth"} {
		t.Run(kind, func(t *testing.T) {
			config, data := fixture(t)
			switch kind {
			case "symlink":
				if runtime.GOOS == "windows" {
					t.Skip("符号链接需Windows专用权限")
				}
				if err := os.Symlink(filepath.Join(config, "config.toml"), filepath.Join(config, "alias")); err != nil {
					t.Fatal(err)
				}
			case "unsafe":
				write(t, filepath.Join(config, "CON.txt"), nil)
			case "size":
				file, err := os.Create(filepath.Join(data, "huge"))
				if err != nil {
					t.Fatal(err)
				}
				err = file.Truncate(MaxBytes + 1)
				file.Close()
				if err != nil {
					t.Fatal(err)
				}
			case "depth":
				if err := os.MkdirAll(filepath.Join(config, strings.Repeat("x/", MaxDepth+1)), 0700); err != nil {
					t.Fatal(err)
				}
			}
			if _, err := Create(context.Background(), config, data, filepath.Join(t.TempDir(), "refused")); err == nil {
				t.Fatal("无效源应拒绝")
			}
		})
	}
}
func TestSnapshotErrorsDoNotExposeSource(t *testing.T) {
	config, data := fixture(t)
	_, err := Create(context.Background(), config, data, "relative-private-name")
	if err == nil || strings.Contains(err.Error(), config) || strings.Contains(err.Error(), "relative-private-name") {
		t.Fatal("错误泄漏原路径")
	}
	backup := filepath.Join(t.TempDir(), "snapshot")
	if _, err = Create(context.Background(), config, data, backup); err != nil {
		t.Fatal(err)
	}
	write(t, filepath.Join(backup, manifestName), []byte(`{"version":1,"entries":[],"private-field-marker":true}`))
	_, err = Verify(context.Background(), backup)
	if err == nil || strings.Contains(err.Error(), "private-field-marker") {
		t.Fatal("清单字段名不得进入错误输出")
	}
}

func TestRestoreCancellationKeepsExistingSourceAndIncompleteTarget(t *testing.T) {
	config, data := fixture(t)
	backup := filepath.Join(t.TempDir(), "snapshot")
	if _, err := Create(context.Background(), config, data, backup); err != nil {
		t.Fatal(err)
	}
	out := filepath.Join(t.TempDir(), "incomplete")
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	checked := checkingContext{Context: ctx, check: func() {
		if _, err := os.Stat(out); err == nil {
			cancel()
		}
	}}
	if _, err := Restore(checked, backup, out); !errors.Is(err, context.Canceled) {
		t.Fatal("恢复取消未传播")
	}
	if _, err := os.Stat(filepath.Join(out, ".kitonyterms-restored.json")); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("取消不得标记恢复完成")
	}
	if _, err := Verify(context.Background(), backup); err != nil {
		t.Fatal("恢复取消不能损坏原备份")
	}
}

func TestSnapshotDetectsLockReplacement(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("Windows锁文件占用限制另需原生验证")
	}
	config, data := fixture(t)
	out := filepath.Join(t.TempDir(), "incomplete")
	changed := false
	checked := checkingContext{Context: context.Background(), check: func() {
		if changed {
			return
		}
		if _, err := os.Stat(out); err == nil {
			changed = true
			name := filepath.Join(data, "kitonyterms.lock")
			if err = os.Rename(name, name+".retained"); err != nil {
				t.Fatal(err)
			}
			write(t, name, nil)
			if err = os.Remove(name + ".retained"); err != nil {
				t.Fatal(err)
			}
		}
	}}
	if _, err := Create(checked, config, data, out); err == nil || !changed {
		t.Fatal("锁替换未拒绝")
	}
	if _, err := os.Stat(filepath.Join(out, manifestName)); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("锁替换不得发布清单")
	}
}

func TestManifestRejectsNonregularBeforeOpening(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("FIFO是Unix原生类型")
	}
	root := t.TempDir()
	if err := exec.Command("mkfifo", filepath.Join(root, manifestName)).Run(); err != nil {
		t.Fatal(err)
	}
	if _, err := Verify(context.Background(), root); err == nil {
		t.Fatal("FIFO完成清单应在打开前拒绝")
	}
}

func TestCompletionPublicationCancellationAndExistingMarker(t *testing.T) {
	for _, existing := range []bool{false, true} {
		root, err := os.OpenRoot(t.TempDir())
		if err != nil {
			t.Fatal(err)
		}
		ctx, cancel := context.WithCancel(context.Background())
		checked := checkingContext{Context: ctx, check: func() {
			if _, err := root.Lstat(manifestName + ".partial"); err == nil {
				cancel()
			}
		}}
		if existing {
			write(t, filepath.Join(root.Name(), manifestName), []byte("existing"))
		}
		err = publish(checked, root, manifestName, Manifest{Version: 1})
		if existing {
			if value, readErr := root.ReadFile(manifestName); err == nil || readErr != nil || string(value) != "existing" {
				t.Fatal("已有完成文件不得覆盖")
			}
		} else {
			if !errors.Is(err, context.Canceled) {
				t.Fatal("提交前取消未生效")
			}
			if _, err = root.Lstat(manifestName); !errors.Is(err, os.ErrNotExist) {
				t.Fatal("取消时不得发布完成文件")
			}
			if _, err = root.Lstat(manifestName + ".partial"); err != nil {
				t.Fatal("未完成清单应保留")
			}
		}
		cancel()
		if err = root.Close(); err != nil {
			t.Fatal(err)
		}
	}
}
