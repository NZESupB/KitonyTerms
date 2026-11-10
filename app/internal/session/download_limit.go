package session

import (
	"context"
	"fmt"
	"io"
)

type downloadLimitKey struct{}
type privateDownloadStagingKey struct{}

// WithPrivateDownloadStaging 仅用于模块独占的暂存目录；完整批次返回后才允许导出。
func WithPrivateDownloadStaging(ctx context.Context, limit uint64) context.Context {
	return context.WithValue(WithDownloadLimit(ctx, limit), privateDownloadStagingKey{}, true)
}

type downloadBudget struct{ remaining uint64 }

// WithDownloadLimit 只约束显式请求的移动暂存，普通桌面批次不受影响。
func WithDownloadLimit(ctx context.Context, limit uint64) context.Context {
	return context.WithValue(ctx, downloadLimitKey{}, &downloadBudget{remaining: limit})
}
func downloadBudgetFrom(ctx context.Context) *downloadBudget {
	budget, _ := ctx.Value(downloadLimitKey{}).(*downloadBudget)
	return budget
}
func checkDownloadBudget(ctx context.Context, nodes []remoteNode) error {
	budget := downloadBudgetFrom(ctx)
	if budget == nil {
		return nil
	}
	var total uint64
	for _, node := range nodes {
		if node.info.IsDir() {
			continue
		}
		size := node.info.Size()
		if size < 0 || uint64(size) > budget.remaining-total {
			return fmt.Errorf("移动下载总量超过限制")
		}
		total += uint64(size)
	}
	return nil
}

type budgetWriter struct {
	destination io.Writer
	budget      *downloadBudget
}

func (w budgetWriter) Write(data []byte) (int, error) {
	if w.budget != nil && uint64(len(data)) > w.budget.remaining {
		return 0, fmt.Errorf("移动下载实际字节超过限制")
	}
	n, err := w.destination.Write(data)
	if w.budget != nil {
		w.budget.remaining -= uint64(n)
	}
	return n, err
}
