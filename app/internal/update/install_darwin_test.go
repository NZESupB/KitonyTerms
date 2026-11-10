//go:build darwin && !ios

package update

import (
	"context"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestInstallDMGTransactionRollbackAndConfirmation(t *testing.T) {
	t.Setenv("KITONYTERMS_UPDATE_TEST_HELPER", "1")
	// 使用真实临时签名应用与DMG；没有触碰已安装应用或正式数据目录。
	source := makeTestBundle(t)
	image := makeTestDMG(t, source)
	for _, finish := range []string{"rollback", "confirm"} {
		t.Run(finish, func(t *testing.T) {
			ctx := context.Background()
			root := archiveTestRoot(t)
			parent := archiveTestRoot(t)
			sourceRoot, err := os.OpenRoot(source)
			if err != nil {
				t.Fatal(err)
			}
			defer sourceRoot.Close()
			if err = copyInstallPayload(ctx, sourceRoot, "KitonyTerms.app", parent, "KitonyTerms.app", true); err != nil {
				t.Fatal(err)
			}
			if err = parent.WriteFile("KitonyTerms.app/Contents/Resources/payload.txt", []byte("旧版本资源"), 0600); err != nil {
				t.Fatal(err)
			}
			if _, err = runUpdateTool(ctx, 20*time.Second, 64<<10, "/usr/bin/codesign", "--force", "--deep", "--sign", "-", filepath.Join(parent.Name(), "KitonyTerms.app")); err != nil {
				t.Fatal(err)
			}
			journal := testInstallJournal(t, root)
			target := filepath.Join(parent.Name(), "KitonyTerms.app")
			journal.Task.Installation = installation{Executable: filepath.Join(target, "Contents/MacOS/kitonyterms"), Target: target, Bundle: true}
			journal.Task.SourceSHA256, err = hashInstallFile(ctx, parent, "KitonyTerms.app/Contents/MacOS/kitonyterms")
			if err != nil {
				t.Fatal(err)
			}
			journal.Task.Package = cacheTestPackage(t, root, "dmg", image)
			if err = writeInstallRecord(root, "task.json", journal.Task); err != nil {
				t.Fatal(err)
			}
			engine, err := openInstaller(ctx, root)
			if err != nil {
				t.Fatal(err)
			}
			defer engine.Close()
			if err = engine.apply(ctx, journal.Task); err != nil {
				t.Fatal("真实DMG事务失败", err)
			}
			resource, err := parent.ReadFile("KitonyTerms.app/Contents/Resources/current")
			if err != nil || string(resource) != "原始资源" {
				t.Fatal("更新资源/链接不正确", string(resource), err)
			}
			stored, _, err := readInstallJournal(root)
			if err != nil || stored.Stage != "committed" {
				t.Fatal("没有提交安装事务", err)
			}
			backupRoot, backup := engine.backup(stored, parent)
			if err = engine.recover(ctx, journal.Task.Next, journal.Task.Installation.Executable, false); err != nil {
				t.Fatal(err)
			}
			if _, err = backupRoot.Lstat(backup); err != nil {
				t.Fatal("确认前丢失备份", err)
			}
			if finish == "rollback" {
				if err = engine.rollback(ctx); err != nil {
					t.Fatal("真实应用包回滚失败", err)
				}
				resource, err = parent.ReadFile("KitonyTerms.app/Contents/Resources/current")
				if err != nil || string(resource) != "旧版本资源" {
					t.Fatal("未恢复旧资源/链接", string(resource), err)
				}
				if err = verifyBundle(ctx, target); err != nil {
					t.Fatal("恢复后签名失效", err)
				}
			} else {
				if err = engine.recover(ctx, journal.Task.Next, journal.Task.Installation.Executable, true); err != nil {
					t.Fatal("应用包启动确认失败", err)
				}
			}
			if _, err = root.Lstat("journal.json"); !errors.Is(err, fs.ErrNotExist) {
				t.Fatal("完成后事务未清理", err)
			}
			if _, err = backupRoot.Lstat(backup); !errors.Is(err, fs.ErrNotExist) {
				t.Fatal("完成后备份未清理", err)
			}
			assertNoPreparationLeftovers(t, root)
		})
	}
}
