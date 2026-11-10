package storage

import (
	"context"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"

	"kitonyterms/internal/model"
)

func TestRepositoryCommitConflictAndRollback(t *testing.T) {
	dir := t.TempDir()
	repo, err := OpenRepository(dir, dir)
	if err != nil {
		t.Fatal(err)
	}
	first := repo.Snapshot()
	p := model.Profile{Name: "回环", Host: "localhost", User: "tester", Auth: []model.AuthMethod{{Kind: "password"}}}
	next, err := repo.SaveProfile(context.Background(), model.SaveProfileRequest{ExpectedRevision: first.Revision, Profile: p})
	if err != nil {
		t.Fatal(err)
	}
	a, _ := strconv.ParseUint(first.Revision, 10, 64)
	b, _ := strconv.ParseUint(next.Revision, 10, 64)
	if b != a+1 {
		t.Fatal("版本没有递增")
	}
	if _, err = repo.DeleteProfile(context.Background(), p.Name, first.Revision); err == nil {
		t.Fatal("接受过期修订")
	}
	reopened, err := OpenRepository(dir, dir)
	if err != nil {
		t.Fatal(err)
	}
	if got := reopened.Snapshot().Config.Sessions; len(got) != 1 || got[0].Port != 22 {
		t.Fatal("配置没有落盘")
	}
	blocker := filepath.Join(dir, "blocker")
	if err = os.WriteFile(blocker, []byte("original"), 0600); err != nil {
		t.Fatal(err)
	}
	repo.configPath = filepath.Join(blocker, "config.toml")
	if _, err = repo.DeleteProfile(context.Background(), p.Name, next.Revision); err == nil {
		t.Fatal("写入失败未传播")
	}
	if got := repo.Snapshot(); got.Revision != next.Revision || len(got.Config.Sessions) != 1 {
		t.Fatal("落盘失败污染内存")
	}
}
func TestRepositoryRejectsUnknownNestedFieldsWithoutMutation(t *testing.T) {
	for _, data := range []string{"future = true\n", "[settings]\nfuture = true\n", "[[sessions]]\nname='a'\nhost='localhost'\nuser='a'\n[sessions.proxy]\nkind='direct'\npassword='must not leak'\n"} {
		dir := t.TempDir()
		p := filepath.Join(dir, "config.toml")
		if err := os.WriteFile(p, []byte(data), 0600); err != nil {
			t.Fatal(err)
		}
		if _, err := OpenRepository(dir, dir); err == nil {
			t.Fatal("未知字段未拒绝")
		}
		original, err := os.ReadFile(p)
		if err != nil || string(original) != data {
			t.Fatal("原件被修改")
		}
	}
}
func TestHostTrustPersistenceAndFailedSave(t *testing.T) {
	dir := t.TempDir()
	repo, err := OpenRepository(dir, dir)
	if err != nil {
		t.Fatal(err)
	}
	if err = repo.TrustHost("HOST", 22, "SHA256:one"); err != nil {
		t.Fatal(err)
	}
	if err = repo.TrustHost("host", 22, "SHA256:two"); err == nil {
		t.Fatal("主机变更被接受")
	}
	reread, err := OpenRepository(dir, dir)
	if err != nil {
		t.Fatal(err)
	}
	if _, ok := reread.CheckHost("host", 22, "SHA256:one"); !ok {
		t.Fatal("信任没有持久化")
	}
	blocker := filepath.Join(dir, "blocker")
	if err = os.WriteFile(blocker, []byte("x"), 0600); err != nil {
		t.Fatal(err)
	}
	repo.hostsPath = filepath.Join(blocker, "hosts.toml")
	if err = repo.TrustHost("other", 22, "SHA256:new"); err == nil {
		t.Fatal("信任保存失败未传播")
	}
	if expected, trusted := repo.CheckHost("other", 22, "SHA256:new"); trusted || expected != "" {
		t.Fatal("保存失败仍信任")
	}
}
func TestVaultRoundtripTamperAndFailedWrite(t *testing.T) {
	dir := t.TempDir()
	v, err := OpenVault(dir)
	if err != nil {
		t.Fatal(err)
	}
	if err = v.Set("test", "测试凭据"); err != nil {
		t.Fatal(err)
	}
	v.Close()
	v, err = OpenVault(dir)
	if err != nil {
		t.Fatal(err)
	}
	defer v.Close()
	if got, ok := v.Get("test"); !ok || got != "测试凭据" {
		t.Fatal("vault 内容往返失败")
	}
	original, err := os.ReadFile(v.path)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(original), "测试凭据") {
		t.Fatal("vault 暴露明文")
	}
	key, err := os.ReadFile(filepath.Join(dir, "secrets.vault.key"))
	if err != nil {
		t.Fatal(err)
	}
	for _, data := range [][]byte{original[:20], append([]byte(nil), original...)} {
		if len(data) > 49 {
			data[len(data)-1] ^= 1
		}
		if _, err := decodeVault(data, strings.TrimSpace(string(key))); err == nil {
			t.Fatal("损坏密文被接受")
		}
	}
	v.path = filepath.Join(dir, "not-created", "bad", "vault")
	if err = os.WriteFile(filepath.Join(dir, "not-created"), []byte("block"), 0600); err != nil {
		t.Fatal(err)
	}
	if err = v.Set("test", "changed"); err == nil {
		t.Fatal("保存错误未返回")
	}
	if got, _ := v.Get("test"); got != "测试凭据" {
		t.Fatal("保存失败仍更新秘密")
	}
}
