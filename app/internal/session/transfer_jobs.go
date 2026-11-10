package session

import (
	"context"
	"errors"
	"fmt"
	"io"
	"math"
	"strconv"
	"sync"

	"kitonyterms/internal/model"
)

type transferContextKey struct{}
type transferJob struct {
	mu     sync.Mutex
	state  model.TransferProgress
	bytes  uint64
	cancel context.CancelFunc
}

func (j *transferJob) snapshot() model.TransferProgress {
	j.mu.Lock()
	defer j.mu.Unlock()
	return j.state
}
func terminalTransfer(status string) bool {
	return status == "succeeded" || status == "failed" || status == "cancelled"
}
func transferFrom(ctx context.Context) *transferJob {
	job, _ := ctx.Value(transferContextKey{}).(*transferJob)
	return job
}
func (j *transferJob) plan(sizes []int64) {
	if j == nil {
		return
	}
	j.mu.Lock()
	defer j.mu.Unlock()
	var total uint64
	known := true
	for _, size := range sizes {
		if size < 0 || math.MaxUint64-total < uint64(size) {
			known = false
			break
		}
		total += uint64(size)
	}
	j.state.TotalFiles = len(sizes)
	if known {
		j.state.TotalBytes = strconv.FormatUint(total, 10)
	}
	if j.state.Status != "cancelling" {
		j.state.Status = "running"
	}
}
func (j *transferJob) current(name string) {
	if j == nil {
		return
	}
	j.mu.Lock()
	j.state.Current = name
	j.mu.Unlock()
}
func (j *transferJob) advance(count int) {
	if j == nil || count <= 0 {
		return
	}
	j.mu.Lock()
	defer j.mu.Unlock()
	j.bytes += uint64(count)
	j.state.Bytes = strconv.FormatUint(j.bytes, 10)
	if total, err := strconv.ParseUint(j.state.TotalBytes, 10, 64); err == nil && j.bytes > total {
		j.state.TotalBytes = ""
	}
}
func (j *transferJob) unknownTotal() {
	if j == nil {
		return
	}
	j.mu.Lock()
	j.state.TotalBytes = ""
	j.mu.Unlock()
}
func (j *transferJob) completed() {
	if j == nil {
		return
	}
	j.mu.Lock()
	j.state.Files++
	j.mu.Unlock()
}

type progressWriter struct {
	destination io.Writer
	job         *transferJob
}

func (w progressWriter) Write(data []byte) (int, error) {
	n, err := w.destination.Write(data)
	w.job.advance(n)
	return n, err
}

func (m *Manager) startTransfer(id, generation, direction string, run func(context.Context) (model.TransferSummary, error)) (model.TransferProgress, error) {
	return m.startTransferOwned(id, generation, direction, context.Background(), nil, run)
}

func (m *Manager) startTransferOwned(id, generation, direction string, lifetime context.Context, cleanup func() error, run func(context.Context) (model.TransferSummary, error)) (model.TransferProgress, error) {
	if err := lifetime.Err(); err != nil {
		return model.TransferProgress{}, err
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	s := m.sessions[id]
	if m.closed || m.draining != "" || s == nil || s.generation != generation {
		return model.TransferProgress{}, fmt.Errorf("会话或连接代次已失效")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.status != "connected" || s.ctx.Err() != nil {
		return model.TransferProgress{}, fmt.Errorf("SSH 未连接")
	}
	for _, job := range s.transfers {
		if !terminalTransfer(job.snapshot().Status) {
			return model.TransferProgress{}, fmt.Errorf("请先完成或取消当前传输")
		}
	}
	ctx, cancel := context.WithCancel(s.ctx)
	stop := context.AfterFunc(lifetime, cancel)
	job := &transferJob{cancel: cancel, state: model.TransferProgress{ID: randomID(), SessionID: id, Generation: generation, Direction: direction, Status: "scanning", Bytes: "0"}}
	if len(s.transfers) >= 32 {
		s.transfers = s.transfers[1:]
	}
	s.transfers = append(s.transfers, job)
	initial := job.state
	m.addWorkerLocked()
	go func() {
		defer m.workerDone()
		defer cancel()
		defer stop()
		_, err := run(context.WithValue(ctx, transferContextKey{}, job))
		operationCancelled := errors.Is(err, context.Canceled) || ctx.Err() != nil
		var cleanupErr error
		if cleanup != nil {
			cleanupErr = cleanup()
			err = errors.Join(err, cleanupErr)
		}
		job.mu.Lock()
		defer job.mu.Unlock()
		switch {
		case cleanupErr != nil:
			job.state.Status = "failed"
			job.state.Error = err.Error()
		case err == nil:
			job.state.Status = "succeeded"
		case operationCancelled:
			job.state.Status = "cancelled"
			job.state.Error = fmt.Sprintf("传输已取消: %v", err)
		default:
			job.state.Status = "failed"
			job.state.Error = err.Error()
		}
	}()
	return initial, nil
}
func (m *Manager) Transfers(id, generation string) ([]model.TransferProgress, error) {
	s, err := m.Get(id, generation)
	if err != nil {
		return nil, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make([]model.TransferProgress, 0, len(s.transfers))
	for _, job := range s.transfers {
		out = append(out, job.snapshot())
	}
	return out, nil
}
func (m *Manager) CancelTransfer(id, generation, transferID string) error {
	s, err := m.Get(id, generation)
	if err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, job := range s.transfers {
		job.mu.Lock()
		if job.state.ID == transferID {
			if !terminalTransfer(job.state.Status) {
				job.state.Status = "cancelling"
				job.cancel()
			}
			job.mu.Unlock()
			return nil
		}
		job.mu.Unlock()
	}
	return fmt.Errorf("传输记录已失效")
}
func (m *Manager) StartUpload(id, generation, dir string, locals []string, overwrite bool) (model.TransferProgress, error) {
	locals = append([]string(nil), locals...)
	return m.startTransfer(id, generation, "upload", func(ctx context.Context) (model.TransferSummary, error) {
		return m.UploadBatch(ctx, id, generation, dir, locals, overwrite)
	})
}
func (m *Manager) StartDownload(id, generation string, remotes []string, destination string) (model.TransferProgress, error) {
	remotes = append([]string(nil), remotes...)
	return m.startTransfer(id, generation, "download", func(ctx context.Context) (model.TransferSummary, error) {
		return m.DownloadBatch(ctx, id, generation, remotes, destination)
	})
}

// 已准备资源在准入成功后归真实 worker 所有；准入失败仍由调用者清理。
func (m *Manager) StartUploadPrepared(id, generation, dir string, locals []string, overwrite bool, lifetime context.Context, cleanup func() error) (model.TransferProgress, error) {
	locals = append([]string(nil), locals...)
	return m.startTransferOwned(id, generation, "upload", lifetime, cleanup, func(ctx context.Context) (model.TransferSummary, error) {
		return m.UploadBatch(ctx, id, generation, dir, locals, overwrite)
	})
}
func (m *Manager) StartDownloadPrepared(id, generation string, remotes []string, destination string, limit uint64, lifetime context.Context, export func(context.Context, func(uint64, int)) error, cleanup func() error) (model.TransferProgress, error) {
	remotes = append([]string(nil), remotes...)
	return m.startTransferOwned(id, generation, "download", lifetime, cleanup, func(ctx context.Context) (model.TransferSummary, error) {
		result, err := m.DownloadBatch(WithPrivateDownloadStaging(ctx, limit), id, generation, remotes, destination)
		if err != nil {
			return result, err
		}
		job := transferFrom(ctx)
		job.mu.Lock()
		job.bytes = 0
		job.state.Bytes = "0"
		job.state.Files = 0
		job.state.Current = ""
		job.state.TotalBytes = result.Bytes
		if job.state.Status != "cancelling" {
			job.state.Status = "exporting"
		}
		job.mu.Unlock()
		if err = ctx.Err(); err != nil {
			return result, err
		}
		err = export(ctx, func(bytes uint64, files int) {
			job.mu.Lock()
			defer job.mu.Unlock()
			if bytes >= job.bytes && files >= job.state.Files {
				job.bytes = bytes
				job.state.Bytes = strconv.FormatUint(bytes, 10)
				job.state.Files = files
			}
		})
		return result, err
	})
}
