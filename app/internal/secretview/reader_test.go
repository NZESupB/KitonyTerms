package secretview

import (
	"context"
	"errors"
	"testing"

	"kitonyterms/internal/model"
	"kitonyterms/internal/osauth"
)

type fixture struct {
	profile      model.Profile
	present      bool
	reads, auths int
	afterRead    func()
}

func (f *fixture) Profile(string) (model.Profile, error) { return f.profile, nil }
func (f *fixture) Has(string) bool                       { return f.present }
func (f *fixture) Get(string) (string, bool) {
	f.reads++
	if f.afterRead != nil {
		f.afterRead()
	}
	return "synthetic-private-value", f.present
}
func setup() (*fixture, Reader) {
	f := &fixture{profile: model.Profile{Name: "fixture", Host: "localhost", User: "tester", Port: 22}, present: true}
	return f, Reader{Profiles: f, Vault: f, Ready: func() error { return nil }, Authenticate: func(context.Context, string) error { f.auths++; return nil }}
}
func TestPresenceDoesNotReadAndEachRevealAuthenticates(t *testing.T) {
	f, r := setup()
	present, err := r.Present(f.profile)
	if err != nil || !present || f.reads != 0 || f.auths != 0 {
		t.Fatal("存在性检查读取了明文或启动认证")
	}
	for range 2 {
		value, err := r.Reveal(context.Background(), f.profile, "auto")
		if err != nil || value == "" {
			t.Fatal("认证后的读取失败")
		}
	}
	if f.auths != 2 || f.reads != 2 {
		t.Fatal("复用了认证结果")
	}
}
func TestRevealRejectsCancelledFailedChangedMissingAndQuit(t *testing.T) {
	for _, kind := range []string{"cancelled", "failed", "target", "missing", "quit"} {
		t.Run(kind, func(t *testing.T) {
			f, r := setup()
			target := f.profile
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			quitting := false
			r.Ready = func() error {
				if quitting {
					return errors.New("quitting")
				}
				return nil
			}
			r.Authenticate = func(context.Context, string) error {
				switch kind {
				case "cancelled":
					cancel()
				case "failed":
					return osauth.ErrFailed
				case "target":
					f.profile.Host = "changed"
				case "missing":
					f.present = false
				case "quit":
					quitting = true
				}
				return nil
			}
			value, err := r.Reveal(ctx, target, "auto")
			if err == nil || value != "" {
				t.Fatal("未通过检查仍返回明文")
			}
			if kind != "missing" && f.reads != 0 {
				t.Fatal("拒绝后仍读取了明文")
			}
		})
	}
	f, r := setup()
	f.present = false
	if _, err := r.Reveal(context.Background(), f.profile, "auto"); !errors.Is(err, ErrMissing) || f.auths != 0 {
		t.Fatal("无凭据仍启动认证")
	}
}

func TestRevealRejectsChangesWhileVaultReadWaits(t *testing.T) {
	for _, kind := range []string{"cancel", "target", "quit"} {
		t.Run(kind, func(t *testing.T) {
			f, r := setup()
			target := f.profile
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			quitting := false
			r.Ready = func() error {
				if quitting {
					return errors.New("quitting")
				}
				return nil
			}
			f.afterRead = func() {
				switch kind {
				case "cancel":
					cancel()
				case "target":
					f.profile.VaultID = "changed"
				case "quit":
					quitting = true
				}
			}
			value, err := r.Reveal(ctx, target, "auto")
			if err == nil || value != "" || f.reads != 1 {
				t.Fatal("读取等待期间变化仍返回了明文")
			}
		})
	}
}

func TestUnmatchedTargetCannotQueryOrAuthenticate(t *testing.T) {
	f, r := setup()
	target := f.profile
	target.VaultID = "different"
	if _, err := r.Present(target); !errors.Is(err, ErrTargetChanged) {
		t.Fatal("不匹配的目标通过存在性检查")
	}
	if _, err := r.Reveal(context.Background(), target, "auto"); !errors.Is(err, ErrTargetChanged) {
		t.Fatal("不匹配的目标通过查看检查")
	}
	if f.reads != 0 || f.auths != 0 {
		t.Fatal("不匹配的目标触发了认证或明文读取")
	}
}
