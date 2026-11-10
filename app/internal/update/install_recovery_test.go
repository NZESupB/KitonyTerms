package update

import (
	"context"
	"encoding/json"
	"errors"
	"io/fs"
	"path/filepath"
	"testing"
)

func TestInstallLegacyJournalConfirmation(t *testing.T) {
	fixture := newInstallFixture(t)
	if err := fixture.installer.applyPrepared(context.Background(), fixture.task, fixture.prepared); err != nil {
		t.Fatal(err)
	}
	data, err := fixture.root.ReadFile("journal.json")
	if err != nil {
		t.Fatal(err)
	}
	var fields map[string]json.RawMessage
	if err = json.Unmarshal(data, &fields); err != nil {
		t.Fatal(err)
	}
	// Windows 的 user_directory 不删 storage；其余平台验证缺省 legacy。
	var storage string
	if err = json.Unmarshal(fields["storage"], &storage); err != nil {
		t.Fatal(err)
	}
	if storage == "legacy" {
		delete(fields, "storage")
	}
	delete(fields, "staged_sha256")
	legacy, err := json.Marshal(fields)
	if err != nil {
		t.Fatal(err)
	}
	if err = fixture.root.WriteFile("journal.json", legacy, 0600); err != nil {
		t.Fatal(err)
	}
	if err = fixture.installer.recover(context.Background(), fixture.task.Next, fixture.task.Installation.Executable, false); err != nil {
		t.Fatal("旧字段记录无法探测", err)
	}
	after, err := fixture.root.ReadFile("journal.json")
	if err != nil || string(after) != string(legacy) {
		t.Fatal("探测改写了旧记录", err)
	}
	if err = fixture.installer.recover(context.Background(), fixture.task.Next, fixture.task.Installation.Executable, true); err != nil {
		t.Fatal("旧字段记录无法确认", err)
	}
}

func TestInstallMissingTransactionParentIsNotMissingJournal(t *testing.T) {
	root := archiveTestRoot(t)
	journal := testInstallJournal(t, root)
	missing := filepath.Join(t.TempDir(), "removed")
	journal.Task.Installation.Target = filepath.Join(missing, "kitonyterms")
	journal.Task.Installation.Executable = journal.Task.Installation.Target
	journal.Staging = filepath.Join(missing, ".kt-update-stage-missing")
	if err := atomicJSON(root, "journal.json", journal); err != nil {
		t.Fatal(err)
	}
	original, err := root.ReadFile("journal.json")
	if err != nil {
		t.Fatal(err)
	}
	engine, err := openInstaller(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	defer engine.Close()
	for _, recover := range []func() error{
		func() error { return engine.rollback(context.Background()) },
		func() error {
			return engine.recover(context.Background(), journal.Task.Current, journal.Task.Installation.Executable, false)
		},
	} {
		if err = recover(); err == nil {
			t.Fatal("把暂存父目录缺失当作没有journal")
		}
		after, err := root.ReadFile("journal.json")
		if err != nil || string(after) != string(original) {
			t.Fatal("损坏记录被删除或改写", err)
		}
	}
}

func TestInstallRollbackWithoutBackupRequiresUnchangedOriginal(t *testing.T) {
	for _, unchanged := range []bool{true, false} {
		t.Run(map[bool]string{true: "unchanged", false: "changed"}[unchanged], func(t *testing.T) {
			fixture := newInstallFixture(t)
			stop := errors.New("在备份后中断")
			fixture.installer.checkpoint = func(stage string) error {
				if stage == "backed_up" {
					return stop
				}
				return nil
			}
			if err := fixture.installer.applyPrepared(context.Background(), fixture.task, fixture.prepared); !errors.Is(err, stop) {
				t.Fatal(err)
			}
			journal, _, err := readInstallJournal(fixture.root)
			if err != nil {
				t.Fatal(err)
			}
			backupRoot, backup := fixture.installer.backup(journal, fixture.parent)
			if err = backupRoot.Remove(backup); err != nil {
				t.Fatal(err)
			}
			if !unchanged {
				if err = fixture.parent.WriteFile("kitonyterms", []byte("另一个程序"), 0700); err != nil {
					t.Fatal(err)
				}
			}
			err = fixture.installer.rollback(context.Background())
			if unchanged {
				if err != nil {
					t.Fatal("原程序未变但无法回收旧事务", err)
				}
				assertInstallOriginal(t, fixture)
				if _, err = fixture.root.Lstat("journal.json"); !errors.Is(err, fs.ErrNotExist) {
					t.Fatal(err)
				}
			} else {
				if err == nil {
					t.Fatal("缺失备份且目标变化仍宣称恢复")
				}
				if _, err = fixture.root.Lstat("journal.json"); err != nil {
					t.Fatal("恢复失败丢失记录", err)
				}
			}
		})
	}
}

func TestUpdateHelperDoesNotRecoverForeignJournal(t *testing.T) {
	fixture := helperInstallFixture(t)
	if err := fixture.installer.installHelper(context.Background(), fixture.task); err != nil {
		t.Fatal(err)
	}
	fixture.task.ParentPID = 1
	if err := writeInstallRecord(fixture.root, "task.json", fixture.task); err != nil {
		t.Fatal(err)
	}
	foreign := []byte(`{"stage":"属于其他中断的事务"}`)
	if err := fixture.root.WriteFile("journal.json", foreign, 0600); err != nil {
		t.Fatal(err)
	}
	if _, err := validateApplyHelper(context.Background(), fixture.root, fixture.task.Current, filepath.Join(fixture.root.Name(), updateHelperName())); err == nil {
		t.Fatal("helper接受了已有journal")
	}
	after, err := fixture.root.ReadFile("journal.json")
	if err != nil || string(after) != string(foreign) {
		t.Fatal("helper改写了其他journal", err)
	}
	assertInstallOriginal(t, fixture)
}

func TestInstallRestoredIdentityBeforeRelaunch(t *testing.T) {
	fixture := newInstallFixture(t)
	if err := verifyRestoredInstall(context.Background(), fixture.root, fixture.task); err != nil {
		t.Fatal(err)
	}
	if err := fixture.parent.WriteFile("kitonyterms", []byte("被其他程序改变的目标"), 0700); err != nil {
		t.Fatal(err)
	}
	if err := verifyRestoredInstall(context.Background(), fixture.root, fixture.task); err == nil {
		t.Fatal("未知目标被当作已恢复程序")
	}
}
