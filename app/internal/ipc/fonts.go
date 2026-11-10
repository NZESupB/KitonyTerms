package ipc

import (
	"context"

	"github.com/wailsapp/wails/v3/pkg/application"
	"kitonyterms/internal/fonts"
	"kitonyterms/internal/model"
)

func (b *Backend) GetFonts(ctx context.Context) (model.FontCatalog, error) {
	if err := b.check(); err != nil {
		return model.FontCatalog{}, err
	}
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	stop := context.AfterFunc(b.ctx, cancel)
	defer stop()
	return fonts.List(ctx, application.System.IsMobile())
}
