package update

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func testInstallJournal(t *testing.T, root *os.Root) installJournal {
	t.Helper()
	target := filepath.Join(t.TempDir(), "kitonyterms")
	current := archiveTestBuild()
	current.Version = "1.2.2"
	current.Build = 122
	next := archiveTestBuild()
	kind, suffix := "tar_gz", ".tar.gz"
	if strings.Contains(current.Target, "apple") {
		kind, suffix = "dmg", ".dmg"
	}
	if strings.Contains(current.Target, "windows") {
		kind, suffix = "zip", ".zip"
	}
	return installJournal{Task: installTask{Installation: installation{Executable: target, Target: target}, Current: current, Next: next, SourceSHA256: strings.Repeat("1", 64), Package: Package{Name: "kitonyterms-test" + suffix, Kind: kind, Size: 100, SHA256: strings.Repeat("2", 64)}, ParentPID: 1}, Stage: "prepared", Storage: "legacy", Staging: filepath.Join(filepath.Dir(target), ".kt-update-stage-fixture")}
}

func TestInstallJournalCompatibilityAndReadOnlyValidation(t *testing.T) {
	root := archiveTestRoot(t)
	journal := testInstallJournal(t, root)
	data, err := json.Marshal(journal)
	if err != nil {
		t.Fatal(err)
	}
	var fields map[string]json.RawMessage
	if err = json.Unmarshal(data, &fields); err != nil {
		t.Fatal(err)
	}
	delete(fields, "storage")
	delete(fields, "staged_sha256")
	var task map[string]json.RawMessage
	if err = json.Unmarshal(fields["task"], &task); err != nil {
		t.Fatal(err)
	}
	delete(task, "elevate")
	fields["task"], err = json.Marshal(task)
	if err != nil {
		t.Fatal(err)
	}
	data, err = json.Marshal(fields)
	if err != nil {
		t.Fatal(err)
	}
	if err = root.WriteFile("journal.json", data, 0600); err != nil {
		t.Fatal(err)
	}
	decoded, digest, err := readInstallJournal(root)
	if err != nil || decoded.Storage != "legacy" || decoded.Task.Elevate || !validHex(digest, 64) {
		t.Fatal("旧默认字段不兼容", decoded, digest, err)
	}
	unchanged, err := root.ReadFile("journal.json")
	if err != nil || !bytes.Equal(data, unchanged) {
		t.Fatal("读取改写了旧事务", err)
	}
	if err = pendingInstallation(root); err == nil {
		t.Fatal("未识别旧事务")
	}
	if _, err = preparePayload(context.Background(), root, journal.Task.Package, journal.Task.Next); err == nil || !strings.Contains(err.Error(), "未完成") {
		t.Fatal("准备覆盖了旧事务", err)
	}
	if err = root.Remove("journal.json"); err != nil {
		t.Fatal(err)
	}
	if err = root.WriteFile("task.json", fields["task"], 0600); err != nil {
		t.Fatal(err)
	}
	if _, err = readInstallTask(root); err != nil {
		t.Fatal("旧准备任务不兼容", err)
	}
	if err = pendingInstallation(root); err == nil {
		t.Fatal("未识别旧准备任务")
	}
	if err = root.Remove("task.json"); err != nil {
		t.Fatal(err)
	}
	journal.Storage = "user_directory"
	journal.Staging = filepath.Join(root.Name(), ".kt-update-stage-fixture")
	if err = atomicJSON(root, "journal.json", journal); err != nil {
		t.Fatal(err)
	}
	if _, _, err = readInstallJournal(root); err != nil {
		t.Fatal("用户目录布局被拒绝", err)
	}
}

func TestInstallJournalRejectsTamperingAndPreservesFiles(t *testing.T) {
	for _, change := range []struct {
		name string
		edit func(*installJournal)
	}{
		{"foreign_directory", func(j *installJournal) { j.Staging = filepath.Join(t.TempDir(), ".kt-update-stage-foreign") }},
		{"bad_stage", func(j *installJournal) { j.Stage = "done" }},
		{"bad_storage", func(j *installJournal) { j.Storage = "automatic" }},
		{"empty_stage_suffix", func(j *installJournal) { j.Staging = filepath.Join(filepath.Dir(j.Staging), ".kt-update-stage-") }},
		{"relative_target", func(j *installJournal) { j.Task.Installation.Target = "kitonyterms" }},
		{"different_executable", func(j *installJournal) { j.Task.Installation.Executable += "-other" }},
		{"not_newer", func(j *installJournal) { j.Task.Next = j.Task.Current }},
		{"wrong_channel", func(j *installJournal) { j.Task.Next.Channel = "alpha" }},
		{"invalid_source_hash", func(j *installJournal) { j.Task.SourceSHA256 = "wrong" }},
		{"invalid_staged_hash", func(j *installJournal) { j.StagedSHA256 = "wrong" }},
		{"missing_pid", func(j *installJournal) { j.Task.ParentPID = 0 }},
	} {
		t.Run(change.name, func(t *testing.T) {
			root := archiveTestRoot(t)
			journal := testInstallJournal(t, root)
			change.edit(&journal)
			data, err := json.Marshal(journal)
			if err != nil {
				t.Fatal(err)
			}
			if err = root.WriteFile("journal.json", data, 0600); err != nil {
				t.Fatal(err)
			}
			if _, _, err = readInstallJournal(root); err == nil {
				t.Fatal("接受损坏事务")
			}
			actual, err := root.ReadFile("journal.json")
			if err != nil || !bytes.Equal(data, actual) {
				t.Fatal("拒绝后改写了原记录", err)
			}
		})
	}
	root := archiveTestRoot(t)
	journal := testInstallJournal(t, root)
	journal.Storage = "user_directory"
	journal.Staging = filepath.Join(root.Name(), ".kt-update-stage-symlink")
	requireUpdateTestSymlink(t, root.Symlink(t.TempDir(), ".kt-update-stage-symlink"))
	if err := atomicJSON(root, "journal.json", journal); err != nil {
		t.Fatal(err)
	}
	if _, _, err := readInstallJournal(root); err == nil {
		t.Fatal("接受暂存链接")
	}
}

func TestInstallRecordsRejectNullMissingAndDuplicateFields(t *testing.T) {
	root := archiveTestRoot(t)
	journal := testInstallJournal(t, root)
	original, err := json.Marshal(journal)
	if err != nil {
		t.Fatal(err)
	}
	var changes = [][]byte{
		bytes.Replace(original, []byte(`"storage":"legacy"`), []byte(`"storage":""`), 1),
		bytes.Replace(original, []byte(`"storage":"legacy"`), []byte(`"storage":null`), 1),
		bytes.Replace(original, []byte(`"storage":"legacy"`), []byte(`"storage":"legacy","storage":"legacy"`), 1),
		bytes.Replace(original, []byte(`"staged_sha256":""`), []byte(`"staged_sha256":null`), 1),
		bytes.Replace(original, []byte(`"bundle":false`), []byte(`"bundle":null`), 1),
		bytes.Replace(original, []byte(`,"installed":false`), nil, 1),
		bytes.Replace(original, []byte(`"elevate":false`), []byte(`"elevate":null`), 1),
	}
	for i, data := range changes {
		if bytes.Equal(data, original) {
			t.Fatal("无效测试变体", i)
		}
		if err = root.WriteFile("journal.json", data, 0600); err != nil {
			t.Fatal(err)
		}
		if _, _, err = readInstallJournal(root); err == nil {
			t.Fatal("接受空值、缺字段或重复字段", i)
		}
	}
}

func TestInstalledJournalRustDigestCompatibility(t *testing.T) {
	old, next := strings.Repeat("a", 64), strings.Repeat("b", 64)
	journal := installJournal{Task: installTask{SourceSHA256: old, Installation: installation{Installed: true}}, StagedSHA256: old}
	if journal.verificationDigest() != "" {
		t.Fatal("旧Rust注册版摘要被当成新EXE摘要")
	}
	journal.StagedSHA256 = strings.ToUpper(old)
	if journal.verificationDigest() != "" {
		t.Fatal("旧摘要大小写不兼容")
	}
	journal.StagedSHA256 = next
	if journal.verificationDigest() != next {
		t.Fatal("新注册版丢失已提交EXE摘要")
	}
	journal.Task.Installation.Installed = false
	journal.StagedSHA256 = old
	if journal.verificationDigest() != old {
		t.Fatal("便携版错误跳过摘要验证")
	}
}
