package storage

import (
	"bytes"
	"crypto/rand"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"

	"golang.org/x/crypto/argon2"
	"golang.org/x/crypto/chacha20poly1305"
)

const maxVaultBytes = 16 << 20
const vaultHeader = 49
const legacyPassword = "kitonyterms:app-managed-vault:v1"

type Vault struct {
	mu      sync.Mutex
	path    string
	salt    []byte
	key     []byte
	entries map[string]string
}
type payload struct {
	Entries map[string]string `json:"entries"`
}

func decodeVault(data []byte, password string) (*Vault, error) {
	if len(data) < vaultHeader || len(data) > maxVaultBytes || string(data[:4]) != "KTVT" || data[4] != 1 {
		return nil, fmt.Errorf("不支持或损坏的 vault 文件")
	}
	n := binary.LittleEndian.Uint32(data[45:49])
	if uint64(n) != uint64(len(data)-vaultHeader) {
		return nil, fmt.Errorf("vault 密文长度错误")
	}
	key := argon2.IDKey([]byte(password), data[5:21], 3, 65536, 1, 32)
	aead, err := chacha20poly1305.NewX(key)
	if err != nil {
		clear(key)
		return nil, err
	}
	plain, err := aead.Open(nil, data[21:45], data[49:], nil)
	if err != nil {
		clear(key)
		return nil, fmt.Errorf("vault 密码错误或文件损坏")
	}
	defer clear(plain)
	var p payload
	if err = json.Unmarshal(plain, &p); err != nil {
		clear(key)
		return nil, fmt.Errorf("vault 内容无效")
	}
	if p.Entries == nil {
		p.Entries = map[string]string{}
	}
	return &Vault{salt: append([]byte(nil), data[5:21]...), key: key, entries: p.Entries}, nil
}
func (v *Vault) encode(entries map[string]string) ([]byte, error) {
	plain, err := json.Marshal(payload{entries})
	if err != nil {
		return nil, err
	}
	defer clear(plain)
	if len(plain) > maxVaultBytes-128 {
		return nil, fmt.Errorf("vault 大小超限")
	}
	nonce := make([]byte, 24)
	if _, err = rand.Read(nonce); err != nil {
		return nil, err
	}
	aead, err := chacha20poly1305.NewX(v.key)
	if err != nil {
		return nil, err
	}
	cipher := aead.Seal(nil, nonce, plain, nil)
	out := make([]byte, 49, len(cipher)+49)
	copy(out, "KTVT")
	out[4] = 1
	copy(out[5:], v.salt)
	copy(out[21:], nonce)
	binary.LittleEndian.PutUint32(out[45:], uint32(len(cipher)))
	return append(out, cipher...), nil
}
func OpenVault(dataDir string) (*Vault, error) {
	path := filepath.Join(dataDir, "secrets.vault")
	keyPath := filepath.Join(dataDir, "secrets.vault.key")
	data, readErr := readLimited(path, maxVaultBytes)
	if readErr != nil && !errors.Is(readErr, os.ErrNotExist) {
		return nil, readErr
	}
	keyData, keyErr := readLimited(keyPath, 1024)
	if keyErr != nil && !errors.Is(keyErr, os.ErrNotExist) {
		return nil, keyErr
	}
	password := strings.ToLower(strings.TrimSpace(string(keyData)))
	var legacy *Vault
	if readErr == nil {
		if keyErr == nil {
			if current, err := decodeVault(data, password); err == nil {
				current.path = path
				return current, nil
			}
		}
		var err error
		legacy, err = decodeVault(data, legacyPassword)
		if err != nil {
			return nil, fmt.Errorf("无法解密，原 vault 未修改: %w", err)
		}
		defer legacy.Close()
		backup := path + ".legacy-v1.backup"
		previous, err := readLimited(backup, maxVaultBytes)
		if err == nil && !bytes.Equal(previous, data) {
			return nil, fmt.Errorf("旧 vault 备份已存在且内容不同")
		}
		if err != nil && !errors.Is(err, os.ErrNotExist) {
			return nil, err
		}
		if errors.Is(err, os.ErrNotExist) {
			if err = AtomicWrite(backup, data); err != nil {
				return nil, fmt.Errorf("备份旧 vault: %w", err)
			}
		}
	}
	if keyErr == nil {
		decoded, e := hex.DecodeString(password)
		if e != nil || len(decoded) != 32 {
			return nil, fmt.Errorf("本机密钥格式无效")
		}
		clear(decoded)
	} else {
		if err := os.MkdirAll(dataDir, 0700); err != nil {
			return nil, err
		}
		random := make([]byte, 32)
		if _, err := rand.Read(random); err != nil {
			return nil, err
		}
		password = hex.EncodeToString(random)
		clear(random)
		f, err := os.OpenFile(keyPath, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
		if err != nil {
			return nil, err
		}
		_, err = f.WriteString(password + "\n")
		if err == nil {
			err = f.Sync()
		}
		closeErr := f.Close()
		if err != nil {
			return nil, err
		}
		if closeErr != nil {
			return nil, closeErr
		}
	}
	salt := make([]byte, 16)
	if _, err := rand.Read(salt); err != nil {
		return nil, err
	}
	v := &Vault{path: path, salt: salt, key: argon2.IDKey([]byte(password), salt, 3, 65536, 1, 32), entries: map[string]string{}}
	if legacy != nil {
		for id, value := range legacy.entries {
			v.entries[id] = value
		}
	}
	encoded, err := v.encode(v.entries)
	if err == nil {
		err = AtomicWrite(path, encoded)
	}
	if err != nil {
		v.Close()
		return nil, err
	}
	return v, nil
}
func (v *Vault) Get(id string) (string, bool) {
	v.mu.Lock()
	defer v.mu.Unlock()
	value, ok := v.entries[id]
	return value, ok
}
func (v *Vault) Has(id string) bool {
	v.mu.Lock()
	defer v.mu.Unlock()
	_, ok := v.entries[id]
	return ok
}
func (v *Vault) Set(id, value string) error {
	v.mu.Lock()
	defer v.mu.Unlock()
	next := make(map[string]string, len(v.entries)+1)
	for k, val := range v.entries {
		next[k] = val
	}
	if value == "" {
		delete(next, id)
	} else {
		next[id] = value
	}
	data, err := v.encode(next)
	if err != nil {
		return err
	}
	if err = AtomicWrite(v.path, data); err != nil {
		return err
	}
	v.entries = next
	return nil
}
func (v *Vault) Close() { v.mu.Lock(); defer v.mu.Unlock(); clear(v.key); clear(v.entries) }
