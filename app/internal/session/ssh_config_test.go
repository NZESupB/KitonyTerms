package session

import (
	"errors"
	"os"
	"path/filepath"
	"testing"

	"kitonyterms/internal/model"
)

type fixtureSSHSettings struct {
	values     map[string]string
	identities []string
	err        error
}

func (s fixtureSSHSettings) GetStrict(_, key string) (string, error)    { return s.values[key], s.err }
func (s fixtureSSHSettings) GetAllStrict(_, _ string) ([]string, error) { return s.identities, s.err }

func TestSSHConfigMissingAutomaticIdentityPreservesPassword(t *testing.T) {
	missing := filepath.Join(t.TempDir(), "missing-identity")
	profile := model.Profile{Host: "alias", Port: 22, User: "tester", Auth: []model.AuthMethod{{Kind: "password"}}}
	result, err := mergeSSHConfig(profile, fixtureSSHSettings{values: map[string]string{"HostName": "127.0.0.1", "Port": "2222"}, identities: []string{missing}})
	if err != nil || len(result.Auth) != 1 || result.Auth[0].Kind != "password" || result.Host != "127.0.0.1" || result.Port != 2222 {
		t.Fatal("缺失自动身份阻断了密码配置", err)
	}
	profile.Auth = []model.AuthMethod{{Kind: "public_key", KeyPath: missing}}
	result, err = mergeSSHConfig(profile, fixtureSSHSettings{})
	if err != nil || len(result.Auth) != 1 || result.Auth[0].KeyPath != missing {
		t.Fatal("显式私钥配置被忽略")
	}
	if _, err = mergeSSHConfig(profile, fixtureSSHSettings{err: errors.New("invalid config")}); err == nil {
		t.Fatal("SSH config 解析错误被吞掉")
	}
}

func TestSSHConfigMultipleIdentityFilesDeduplicate(t *testing.T) {
	root := t.TempDir()
	first, second := filepath.Join(root, "first"), filepath.Join(root, "second")
	for _, name := range []string{first, second} {
		if err := os.WriteFile(name, []byte("public fixture"), 0600); err != nil {
			t.Fatal(err)
		}
	}
	profile := model.Profile{Host: "alias", Auth: []model.AuthMethod{{Kind: "password"}, {Kind: "public_key", KeyPath: first}}}
	result, err := mergeSSHConfig(profile, fixtureSSHSettings{identities: []string{first, second}})
	if err != nil || len(result.Auth) != 3 || result.Auth[0].Kind != "password" || result.Auth[2].KeyPath != second {
		t.Fatal("身份文件未去重或改变了显式认证顺序", err)
	}
}
