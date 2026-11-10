package update

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// 固定夹具由退役前的真实kt-update序列化；仅临时根、宿主目标及包类型适配当前平台。
func TestLegacyFrozenUpdateRecords(t *testing.T) {
	for _, layout := range []string{"legacy", "user_directory", "old_defaults"} {
		t.Run(layout, func(t *testing.T) {
			base := t.TempDir()
			for _, child := range []string{"install", "updates"} {
				if err := os.Mkdir(filepath.Join(base, child), 0700); err != nil {
					t.Fatal(err)
				}
			}
			root, err := os.OpenRoot(filepath.Join(base, "updates"))
			if err != nil {
				t.Fatal(err)
			}
			defer root.Close()
			for _, name := range []string{"journal", "task"} {
				data, err := os.ReadFile(filepath.Join("testdata", "legacy-"+layout+"-"+name+".json"))
				if err != nil {
					t.Fatal(err)
				}
				quoted, _ := json.Marshal(base)
				data = bytes.ReplaceAll(data, []byte("@ROOT@"), quoted[1:len(quoted)-1])
				// 路径分隔符由Rust跨平台JSON惯例转为本机路径，保留原字段/缺省布局。
				var fields map[string]any
				if err = json.Unmarshal(data, &fields); err != nil {
					t.Fatal(err)
				}
				task := fields
				if name == "journal" {
					task = fields["task"].(map[string]any)
					parent := "install"
					if layout == "user_directory" {
						parent = "updates"
					}
					fields["staging"] = filepath.Join(base, parent, ".kt-update-stage-fixture")
				}
				install := task["installation"].(map[string]any)
				install["target"], install["executable"] = filepath.Join(base, "install", "kitonyterms"), filepath.Join(base, "install", "kitonyterms")
				for _, key := range []string{"current", "next"} {
					task[key].(map[string]any)["target"] = desktopTarget()
				}
				if !strings.Contains(desktopTarget(), "apple") {
					pkg := task["package"].(map[string]any)
					kind, suffix := "tar_gz", ".tar.gz"
					if strings.Contains(desktopTarget(), "windows") {
						kind, suffix = "zip", ".zip"
					}
					pkg["kind"], pkg["name"] = kind, "kitonyterms-v0.1.18-fixture"+suffix
				}
				data, err = json.Marshal(fields)
				if err != nil {
					t.Fatal(err)
				}
				if err = root.WriteFile(name+".json", data, 0600); err != nil {
					t.Fatal(err)
				}
			}
			before, err := root.ReadFile("journal.json")
			if err != nil {
				t.Fatal(err)
			}
			journal, digest, err := readInstallJournal(root)
			if err != nil || !validHex(digest, 64) || journal.Stage != "prepared" {
				t.Fatal(journal, err)
			}
			if task, err := readInstallTask(root); err != nil || task != journal.Task {
				t.Fatal("旧task与journal不一致", err)
			}
			want := layout
			if layout == "old_defaults" {
				want = "legacy"
			}
			if journal.Storage != want || journal.Task.Elevate {
				t.Fatal("旧缺省字段不兼容")
			}
			after, err := root.ReadFile("journal.json")
			if err != nil || !bytes.Equal(before, after) {
				t.Fatal("回读修改了旧记录", err)
			}
			if err = writeInstallRecord(root, "journal.json", journal); err != nil {
				t.Fatal(err)
			}
			decoded, _, err := readInstallJournal(root)
			if err != nil || decoded != journal {
				t.Fatal("Go回写改变了旧记录语义", err)
			}
		})
	}
}
