package ipc

import (
	"context"
	"errors"
	"runtime"

	"kitonyterms/internal/model"
	"kitonyterms/internal/osauth"
	"kitonyterms/internal/secretview"
)

func (b *Backend) secretReader() secretview.Reader {
	return secretview.Reader{
		Profiles: b.repo, Vault: b.vault, Ready: b.check,
		Authenticate: func(ctx context.Context, method string) error {
			if !osauth.Available() || b.window == nil {
				return osauth.ErrUnavailable
			}
			reason := "验证身份以查看已保存的连接凭据"
			if b.repo.Snapshot().Config.Settings.Language == "english" {
				reason = "Authenticate to view the saved connection credential"
			}
			return osauth.Authenticate(ctx, reason, uintptr(b.window.NativeWindow()), method)
		},
	}
}

func (b *Backend) GetSecretState(target model.Profile) (model.SecretState, error) {
	if err := b.check(); err != nil {
		return model.SecretState{}, err
	}
	present, err := b.secretReader().Present(target)
	available := osauth.Available() && b.window != nil
	return model.SecretState{Present: present, Available: available, AccountPassword: available && runtime.GOOS == "windows"}, err
}

func (b *Backend) RevealSecret(ctx context.Context, target model.Profile, method string) (model.SecretReveal, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.SecretReveal{}, err
	}
	defer done()
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	stop := context.AfterFunc(b.ctx, cancel)
	defer stop()
	value, err := b.secretReader().Reveal(ctx, target, method)
	failure := osauth.Failure(err)
	if errors.Is(err, secretview.ErrTargetChanged) {
		failure = "target_changed"
	}
	if errors.Is(err, secretview.ErrMissing) {
		failure = "missing"
	}
	return model.SecretReveal{Value: value, Failure: failure}, nil
}
