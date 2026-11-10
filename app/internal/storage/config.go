package storage

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/pelletier/go-toml/v2"
	"kitonyterms/internal/model"
)

type Repository struct {
	mu         sync.Mutex
	configPath string
	hostsPath  string
	config     model.Config
	revision   string
	hosts      []model.KnownHost
}

func Clone[T any](value T) T {
	data, err := json.Marshal(value)
	if err != nil {
		panic(err)
	}
	var result T
	if err = json.Unmarshal(data, &result); err != nil {
		panic(err)
	}
	return result
}
func readLimited(path string, limit int64) ([]byte, error) {
	info, err := os.Stat(path)
	if err != nil {
		return nil, err
	}
	if info.Size() > limit {
		return nil, fmt.Errorf("文件大小超限")
	}
	return os.ReadFile(path)
}
func OpenRepository(configDir, dataDir string) (*Repository, error) {
	r := &Repository{configPath: filepath.Join(configDir, "config.toml"), hostsPath: filepath.Join(dataDir, "known_hosts.toml"), config: model.DefaultConfig()}
	data, err := readLimited(r.configPath, 4<<20)
	if err == nil {
		decoder := toml.NewDecoder(bytes.NewReader(data)).DisallowUnknownFields()
		if err = decoder.Decode(&r.config); err != nil {
			return nil, fmt.Errorf("解析配置（未知字段不会被丢弃，原件未修改）: %w", err)
		}
	} else if !errors.Is(err, os.ErrNotExist) {
		return nil, err
	}
	if err := r.config.Normalize(); err != nil {
		return nil, fmt.Errorf("配置无效: %w", err)
	}
	data, err = readLimited(r.hostsPath, 4<<20)
	if err == nil {
		var h struct {
			Hosts []model.KnownHost `toml:"hosts"`
		}
		if err = toml.Unmarshal(data, &h); err != nil {
			return nil, err
		}
		r.hosts = h.Hosts
	} else if !errors.Is(err, os.ErrNotExist) {
		return nil, err
	}
	r.revision = strconv.FormatInt(time.Now().UnixNano(), 10)
	return r, nil
}
func (r *Repository) Snapshot() model.ConfigSnapshot {
	r.mu.Lock()
	defer r.mu.Unlock()
	return model.ConfigSnapshot{Revision: r.revision, Config: Clone(r.config)}
}
func (r *Repository) Profile(name string) (model.Profile, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, p := range r.config.Sessions {
		if p.Name == name {
			return Clone(p), nil
		}
	}
	return model.Profile{}, fmt.Errorf("连接配置不存在")
}
func (r *Repository) update(ctx context.Context, expected string, change func(*model.Config) error) (model.ConfigSnapshot, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return model.ConfigSnapshot{}, err
	}
	if expected != r.revision {
		return model.ConfigSnapshot{}, fmt.Errorf("REVISION_CONFLICT: 配置已更新，请重新加载")
	}
	next := Clone(r.config)
	if err := change(&next); err != nil {
		return model.ConfigSnapshot{}, err
	}
	if err := next.Normalize(); err != nil {
		return model.ConfigSnapshot{}, err
	}
	data, err := toml.Marshal(next)
	if err != nil {
		return model.ConfigSnapshot{}, err
	}
	if err = AtomicWrite(r.configPath, data); err != nil {
		return model.ConfigSnapshot{}, err
	}
	r.config = next
	version, _ := strconv.ParseUint(r.revision, 10, 64)
	r.revision = strconv.FormatUint(version+1, 10)
	return model.ConfigSnapshot{Revision: r.revision, Config: Clone(next)}, nil
}
func (r *Repository) SaveProfile(ctx context.Context, req model.SaveProfileRequest) (model.ConfigSnapshot, error) {
	if err := req.Profile.Normalize(); err != nil {
		return model.ConfigSnapshot{}, err
	}
	return r.update(ctx, req.ExpectedRevision, func(c *model.Config) error {
		index := -1
		for i, p := range c.Sessions {
			if p.Name == req.OriginalName && req.OriginalName != "" {
				index = i
			}
			if p.Name == req.Profile.Name && p.Name != req.OriginalName {
				return fmt.Errorf("连接名称已存在")
			}
		}
		if req.OriginalName != "" && index < 0 {
			return fmt.Errorf("原连接已不存在")
		}
		if index < 0 {
			c.Sessions = append(c.Sessions, req.Profile)
		} else {
			old := c.Sessions[index]
			if req.Profile.VaultID == "" && old.Host == req.Profile.Host && old.User == req.Profile.User && old.Port == req.Profile.Port {
				req.Profile.VaultID = old.SecretID()
			}
			c.Sessions[index] = req.Profile
		}
		return nil
	})
}
func (r *Repository) DeleteProfile(ctx context.Context, name, expected string) (model.ConfigSnapshot, error) {
	return r.update(ctx, expected, func(c *model.Config) error {
		for i, p := range c.Sessions {
			if p.Name == name {
				c.Sessions = append(c.Sessions[:i], c.Sessions[i+1:]...)
				return nil
			}
		}
		return fmt.Errorf("连接不存在")
	})
}
func (r *Repository) SaveSettings(ctx context.Context, req model.SaveSettingsRequest) (model.ConfigSnapshot, error) {
	return r.update(ctx, req.ExpectedRevision, func(c *model.Config) error { c.Settings = req.Settings; return nil })
}
func (r *Repository) CheckHost(host string, port int, fingerprint string) (string, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, h := range r.hosts {
		if strings.EqualFold(h.Host, host) && h.Port == port {
			return h.Fingerprint, h.Fingerprint == fingerprint
		}
	}
	return "", false
}
func (r *Repository) TrustHost(host string, port int, fingerprint string) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, h := range r.hosts {
		if strings.EqualFold(h.Host, host) && h.Port == port {
			if h.Fingerprint != fingerprint {
				return fmt.Errorf("主机密钥变更，拒绝自动替换")
			}
			return nil
		}
	}
	now := time.Now().Unix()
	next := append(append([]model.KnownHost{}, r.hosts...), model.KnownHost{Host: host, Port: port, Fingerprint: fingerprint, FirstSeen: now, LastSeen: now})
	data, err := toml.Marshal(struct {
		Hosts []model.KnownHost `toml:"hosts"`
	}{next})
	if err != nil {
		return err
	}
	if err = AtomicWrite(r.hostsPath, data); err != nil {
		return err
	}
	r.hosts = next
	return nil
}

func (r *Repository) ImportConfig(ctx context.Context, expected string, config model.Config) (model.ConfigSnapshot, error) {
	if err := config.Normalize(); err != nil {
		return model.ConfigSnapshot{}, err
	}
	return r.update(ctx, expected, func(next *model.Config) error { *next = Clone(config); return nil })
}
