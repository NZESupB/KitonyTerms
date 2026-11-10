package update

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"io"
	"testing"
)

func TestBuildRecordAndReadOnlyCommand(t *testing.T) {
	info := testBuild()
	record := map[string]any{"schema": 1, "channel": info.Channel, "version": info.Version, "commit": info.Commit, "build": info.Build, "attempt": info.Attempt, "target": info.Target, "label": "v" + info.Version}
	data, _ := json.Marshal(record)
	encoded, err := EncodeBuildRecord(data, info.Version)
	if err != nil {
		t.Fatal(err)
	}
	decoded, err := base64.RawURLEncoding.DecodeString(encoded)
	if err != nil {
		t.Fatal(err)
	}
	var got BuildInfo
	if err = json.Unmarshal(decoded, &got); err != nil || got != info {
		t.Fatal(got, err)
	}
	if _, err = EncodeBuildRecord(data, "9.9.9"); err == nil {
		t.Fatal("接受不同应用版本")
	}
	record["label"] = "alpha"
	data, _ = json.Marshal(record)
	if _, err = EncodeBuildRecord(data, info.Version); err == nil {
		t.Fatal("接受不同发布标签")
	}
	before := embeddedIdentity
	embeddedIdentity = ""
	defer func() { embeddedIdentity = before }()
	var output bytes.Buffer
	if handled, err := HandleCommand([]string{"--update-info"}, &output); !handled || err != nil {
		t.Fatal(handled, err)
	}
	if err = json.Unmarshal(output.Bytes(), &got); err != nil || got.Channel != "development" {
		t.Fatal(got, err)
	}
	for _, args := range [][]string{{"--update-info", "extra"}, {"--apply-update"}, {"--relaunch-update"}, {"--elevated-replace", "fixture"}, {"--restore-update"}} {
		if handled, err := HandleCommand(args, io.Discard); !handled || err == nil {
			t.Fatal("未拒绝尚未支持的辅助命令", args[0])
		}
	}
	if handled, err := HandleCommand([]string{"--port", "1234"}, io.Discard); handled || err != nil {
		t.Fatal("拦截了普通启动参数")
	}
}
