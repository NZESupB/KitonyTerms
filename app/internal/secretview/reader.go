package secretview

import (
	"context"
	"errors"
	"reflect"

	"kitonyterms/internal/model"
)

var ErrTargetChanged = errors.New("连接配置已改变，请重新打开连接设置")
var ErrMissing = errors.New("未保存该连接的凭据")

type Profiles interface {
	Profile(string) (model.Profile, error)
}
type Vault interface {
	Has(string) bool
	Get(string) (string, bool)
}

type Reader struct {
	Profiles     Profiles
	Vault        Vault
	Authenticate func(context.Context, string) error
	Ready        func() error
}

func (r Reader) target(target model.Profile) (model.Profile, error) {
	current, err := r.Profiles.Profile(target.Name)
	if err != nil {
		return model.Profile{}, err
	}
	if !reflect.DeepEqual(current, target) {
		return model.Profile{}, ErrTargetChanged
	}
	return current, nil
}

// Present只查询索引，不读取或复制凭据明文。
func (r Reader) Present(target model.Profile) (bool, error) {
	p, err := r.target(target)
	if err != nil {
		return false, err
	}
	return r.Vault.Has(p.SecretID()), nil
}

func (r Reader) valid(ctx context.Context, target model.Profile) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	if err := r.Ready(); err != nil {
		return err
	}
	_, err := r.target(target)
	return err
}

func (r Reader) Reveal(ctx context.Context, target model.Profile, method string) (string, error) {
	if err := ctx.Err(); err != nil {
		return "", err
	}
	p, err := r.target(target)
	if err != nil {
		return "", err
	}
	if !r.Vault.Has(p.SecretID()) {
		return "", ErrMissing
	}
	if err = r.Authenticate(ctx, method); err != nil {
		return "", err
	}
	if err = r.valid(ctx, target); err != nil {
		return "", err
	}
	value, ok := r.Vault.Get(p.SecretID())
	if !ok {
		return "", ErrMissing
	}
	if err = r.valid(ctx, target); err != nil {
		return "", err
	}
	return value, nil
}
