package syncconfig

import (
	"context"
	"os"
	"path/filepath"
	"reflect"
	"testing"

	"kitonyterms/internal/storage"
)

func TestLegacyFrozenEnvelope(t *testing.T) {
	data, err := os.ReadFile(filepath.Join("testdata", "legacy-envelope.json"))
	if err != nil {
		t.Fatal(err)
	}
	envelope, err := Decode(data)
	if err != nil {
		t.Fatal(err)
	}
	if len(envelope.Config.Sessions) != 1 || envelope.Config.Sessions[0].Name != "Rust 夹具" || len(envelope.Config.Sessions[0].Auth) != 4 || envelope.Config.Settings.UIFontFamily != "测试字体" {
		t.Fatal("旧同步字段不兼容")
	}
	encoded, err := envelope.Encode()
	if err != nil {
		t.Fatal(err)
	}
	roundtrip, err := Decode(encoded)
	if err != nil || !reflect.DeepEqual(envelope, roundtrip) {
		t.Fatal("旧同步回写语义改变", err)
	}
	root := t.TempDir()
	repo, err := storage.OpenRepository(root, root)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = repo.ImportConfig(context.Background(), repo.Snapshot().Revision, roundtrip.Config); err != nil {
		t.Fatal(err)
	}
	reopened, err := storage.OpenRepository(root, root)
	if err != nil || !reflect.DeepEqual(reopened.Snapshot().Config, roundtrip.Config) {
		t.Fatal("旧同步导入未完整持久化", err)
	}
}
