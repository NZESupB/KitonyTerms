package storage

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"kitonyterms/internal/model"
)

// 夹具由退役前的kt-config/kt-secrets生成，只包含公开测试值，测试不需要Rust。
func TestRustFixtureRoundtrip(t *testing.T) {
	dir := t.TempDir()
	for source, target := range map[string]string{"rust-config.toml": "config.toml", "rust-v1.vault": "secrets.vault"} {
		data, err := os.ReadFile(filepath.Join("testdata", source))
		if err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(filepath.Join(dir, target), data, 0600); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(filepath.Join(dir, "secrets.vault.key"), []byte("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n"), 0600); err != nil {
		t.Fatal(err)
	}
	repo, err := OpenRepository(dir, dir)
	if err != nil {
		t.Fatal(err)
	}
	snapshot := repo.Snapshot()
	if len(snapshot.Config.Sessions) != 1 || snapshot.Config.Sessions[0].Name != "Rust 夹具" || len(snapshot.Config.Sessions[0].Auth) != 4 || len(snapshot.Config.Settings.SFTPColumnWidths) != 6 {
		t.Fatal("Rust TOML 字段不兼容")
	}
	snapshot.Config.Settings.FontSize = 15
	if _, err = repo.SaveSettings(context.Background(), model.SaveSettingsRequest{ExpectedRevision: snapshot.Revision, Settings: snapshot.Config.Settings}); err != nil {
		t.Fatal(err)
	}
	vault, err := OpenVault(dir)
	if err != nil {
		t.Fatal(err)
	}
	defer vault.Close()
	if got, ok := vault.Get("tester@test.invalid:22"); !ok || got != "interop-test-value" {
		t.Fatal("Rust vault 解密不兼容")
	}
	if err = vault.Set("go-write", "go-roundtrip"); err != nil {
		t.Fatal(err)
	}
}
func TestRustLegacyVaultMigrationPreservesBackup(t *testing.T) {
	dir := t.TempDir()
	original, err := os.ReadFile(filepath.Join("testdata", "rust-legacy.vault"))
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(dir, "secrets.vault"), original, 0600); err != nil {
		t.Fatal(err)
	}
	vault, err := OpenVault(dir)
	if err != nil {
		t.Fatal(err)
	}
	if got, ok := vault.Get("legacy-test"); !ok || got != "legacy-value" {
		t.Fatal("旧 vault 迁移丢失数据")
	}
	vault.Close()
	backup, err := os.ReadFile(filepath.Join(dir, "secrets.vault.legacy-v1.backup"))
	if err != nil || string(backup) != string(original) {
		t.Fatal("旧 vault 原件备份不一致")
	}
	reopened, err := OpenVault(dir)
	if err != nil {
		t.Fatal(err)
	}
	defer reopened.Close()
	if got, _ := reopened.Get("legacy-test"); got != "legacy-value" {
		t.Fatal("迁移后不能重开")
	}
}
