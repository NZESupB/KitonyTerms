package externaledit

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"os"
	"path"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"kitonyterms/internal/model"
)

const maxBytes = 32 << 20

type Remote interface {
	ReadEditFile(context.Context, string, string, string) ([]byte, string, error)
	WriteEditFile(context.Context, string, string, string, []byte, string) (string, error)
	EditSessionActive(string, string) bool
}
type Launcher func(string, string) error

type Manager struct {
	mu       sync.Mutex
	ctx      context.Context
	remote   Remote
	launch   Launcher
	root     string
	jobs     map[string]*job
	closed   bool
	interval time.Duration
}
type job struct {
	mu            sync.Mutex
	state         model.ExternalEdit
	ctx           context.Context
	cancel        context.CancelFunc
	done          chan struct{}
	actions       chan string
	dir           string
	file          string
	baseline      string
	revision      string
	observed      os.FileInfo
	changedOnDisk bool
}

func New(ctx context.Context, root string, remote Remote, launch Launcher) (*Manager, error) {
	if err := os.MkdirAll(root, 0700); err != nil {
		return nil, err
	}
	info, err := os.Lstat(root)
	if err != nil {
		return nil, err
	}
	if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return nil, fmt.Errorf("编辑副本目录无效")
	}
	if err = os.Chmod(root, 0700); err != nil {
		return nil, err
	}
	return &Manager{ctx: ctx, root: root, remote: remote, launch: launch, jobs: map[string]*job{}, interval: 750 * time.Millisecond}, nil
}
func (j *job) snapshot() model.ExternalEdit        { j.mu.Lock(); defer j.mu.Unlock(); return j.state }
func (j *job) update(fn func(*model.ExternalEdit)) { j.mu.Lock(); defer j.mu.Unlock(); fn(&j.state) }
func (j *job) fail(err error) {
	j.update(func(s *model.ExternalEdit) { s.Status = "error"; s.AutoUpload = false; s.Error = err.Error() })
}
func hash(data []byte) string  { value := sha256.Sum256(data); return hex.EncodeToString(value[:]) }
func ended(status string) bool { return status == "retained" || status == "failed" }
func sameFile(a, b os.FileInfo) bool {
	return a != nil && b != nil && os.SameFile(a, b) && a.Size() == b.Size() && a.ModTime().Equal(b.ModTime())
}
func filename(remote string) string {
	var b strings.Builder
	for _, r := range path.Base(remote) {
		if b.Len() >= 160 {
			break
		}
		if r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || r == '.' || r == '-' || r == '_' {
			b.WriteRune(r)
		} else {
			b.WriteByte('_')
		}
	}
	return "file-" + strings.TrimRight(b.String(), ". ")
}
func (m *Manager) Start(id, generation, remote, command string) (model.ExternalEdit, error) {
	if id == "" || generation == "" || !path.IsAbs(remote) || len(remote) > 8192 || strings.ContainsAny(remote, "\x00\r\n") {
		return model.ExternalEdit{}, fmt.Errorf("外部编辑目标无效")
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.closed || m.ctx.Err() != nil {
		return model.ExternalEdit{}, fmt.Errorf("外部编辑服务已停止")
	}
	active := 0
	for _, j := range m.jobs {
		s := j.snapshot()
		if !ended(s.Status) {
			active++
			if s.SessionID == id && s.Generation == generation && s.RemotePath == remote {
				return s, nil
			}
		}
	}
	if active >= 16 || len(m.jobs) >= 32 {
		return model.ExternalEdit{}, fmt.Errorf("外部编辑副本过多，请先结束编辑或关闭已保留的记录")
	}
	dir, err := os.MkdirTemp(m.root, "edit-")
	if err != nil {
		return model.ExternalEdit{}, err
	}
	ctx, cancel := context.WithCancel(m.ctx)
	j := &job{ctx: ctx, cancel: cancel, done: make(chan struct{}), actions: make(chan string, 1), dir: dir, file: filename(remote)}
	j.state = model.ExternalEdit{ID: rand.Text(), SessionID: id, Generation: generation, RemotePath: remote, LocalPath: filepath.Join(dir, j.file), Status: "downloading"}
	m.jobs[j.state.ID] = j
	go m.run(j, command)
	return j.snapshot(), nil
}
func (m *Manager) List() []model.ExternalEdit {
	m.mu.Lock()
	defer m.mu.Unlock()
	out := make([]model.ExternalEdit, 0, len(m.jobs))
	for _, j := range m.jobs {
		out = append(out, j.snapshot())
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out
}
func (m *Manager) Active() int {
	count := 0
	for _, s := range m.List() {
		if !ended(s.Status) {
			count++
		}
	}
	return count
}
func (m *Manager) Action(ctx context.Context, id, generation, editID, action string) error {
	m.mu.Lock()
	j := m.jobs[editID]
	closed := m.closed
	m.mu.Unlock()
	if j == nil {
		return fmt.Errorf("编辑记录已结束")
	}
	s := j.snapshot()
	if s.SessionID != id || s.Generation != generation {
		return fmt.Errorf("编辑记录不属于当前连接")
	}
	if action == "stop" {
		j.cancel()
		select {
		case <-j.done:
		case <-ctx.Done():
			return ctx.Err()
		}
		return nil
	}
	if action == "dismiss" && ended(s.Status) {
		m.mu.Lock()
		delete(m.jobs, editID)
		m.mu.Unlock()
		return nil
	}
	if closed || ended(s.Status) || s.Status == "downloading" || s.Status == "uploading" {
		return fmt.Errorf("编辑任务暂不可操作")
	}
	if action != "once" && action != "auto" {
		return fmt.Errorf("未知编辑动作")
	}
	select {
	case j.actions <- action:
		return nil
	default:
		return fmt.Errorf("编辑动作正在处理")
	}
}
func (m *Manager) StopSession(id, generation string) {
	m.mu.Lock()
	var jobs []*job
	for _, j := range m.jobs {
		s := j.snapshot()
		if s.SessionID == id && s.Generation == generation {
			j.cancel()
			jobs = append(jobs, j)
		}
	}
	m.mu.Unlock()
	for _, j := range jobs {
		<-j.done
	}
}
func (m *Manager) Close() {
	m.mu.Lock()
	m.closed = true
	var jobs []*job
	for _, j := range m.jobs {
		j.cancel()
		jobs = append(jobs, j)
	}
	m.mu.Unlock()
	for _, j := range jobs {
		<-j.done
	}
}
func readLocal(j *job) ([]byte, error) {
	root, err := os.OpenRoot(j.dir)
	if err != nil {
		return nil, err
	}
	defer root.Close()
	info, err := root.Lstat(j.file)
	if err != nil {
		return nil, err
	}
	if !info.Mode().IsRegular() || info.Size() < 0 || info.Size() > maxBytes {
		return nil, fmt.Errorf("本地副本必须是不超过32 MiB的普通文件")
	}
	file, err := root.OpenFile(j.file, os.O_RDONLY, 0)
	if err != nil {
		return nil, err
	}
	defer file.Close()
	opened, err := file.Stat()
	if err != nil {
		return nil, err
	}
	if !sameFile(info, opened) || !opened.Mode().IsRegular() {
		return nil, fmt.Errorf("读取期间本地文件已替换")
	}
	if err = file.Chmod(0600); err != nil {
		return nil, err
	}
	data, err := io.ReadAll(io.LimitReader(file, maxBytes+1))
	if err != nil {
		return nil, err
	}
	if len(data) > maxBytes {
		return nil, fmt.Errorf("本地副本超过32 MiB")
	}
	after, err := root.Lstat(j.file)
	if err != nil {
		return nil, err
	}
	if !sameFile(info, after) || int64(len(data)) != after.Size() {
		return nil, fmt.Errorf("本地文件仍在保存，请稍后重试")
	}
	return data, nil
}
func (m *Manager) finish(j *job) {
	defer close(j.done)
	s := j.snapshot()
	data, readErr := readLocal(j)
	clean := readErr == nil && j.baseline != "" && hash(data) == j.baseline
	if clean || j.baseline == "" && os.IsNotExist(readErr) {
		root, err := os.OpenRoot(j.dir)
		if err == nil {
			err = root.Remove(j.file)
			if os.IsNotExist(err) {
				err = nil
			}
			root.Close()
		}
		if err == nil {
			err = os.Remove(j.dir)
		}
		if err == nil {
			if s.Error != "" && j.baseline == "" {
				j.update(func(s *model.ExternalEdit) { s.Status = "failed"; s.LocalPath = ""; s.Dirty = false })
			} else {
				m.mu.Lock()
				delete(m.jobs, s.ID)
				m.mu.Unlock()
			}
			return
		}
		j.fail(fmt.Errorf("清理编辑副本失败: %w", err))
	}
	j.update(func(s *model.ExternalEdit) {
		s.Status = "retained"
		s.AutoUpload = false
		s.Dirty = !clean
		if s.Error == "" {
			s.Error = "监听已结束，本地副本已保留"
		}
	})
}
func (m *Manager) run(j *job, command string) {
	defer m.finish(j)
	defer j.cancel()
	s := j.snapshot()
	readContext, cancelRead := context.WithTimeout(j.ctx, 30*time.Second)
	data, revision, err := m.remote.ReadEditFile(readContext, s.SessionID, s.Generation, s.RemotePath)
	cancelRead()
	if err != nil {
		j.fail(err)
		return
	}
	if len(data) > maxBytes {
		j.fail(fmt.Errorf("文件超过32 MiB"))
		return
	}
	if err = j.ctx.Err(); err != nil {
		return
	}
	file, err := os.OpenFile(s.LocalPath, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
	if err != nil {
		j.fail(err)
		return
	}
	_, writeErr := file.Write(data)
	closeErr := file.Close()
	if writeErr != nil {
		j.fail(writeErr)
		return
	}
	if closeErr != nil {
		j.fail(closeErr)
		return
	}
	j.baseline = hash(data)
	j.revision = revision
	j.observed, _ = os.Lstat(s.LocalPath)
	if err = j.ctx.Err(); err != nil {
		return
	}
	if !m.remote.EditSessionActive(s.SessionID, s.Generation) {
		j.fail(fmt.Errorf("SSH会话已断开"))
		return
	}
	if err = m.launch(s.LocalPath, command); err != nil {
		j.fail(err)
	} else {
		j.update(func(s *model.ExternalEdit) { s.Status = "watching" })
	}
	timer := time.NewTicker(m.interval)
	defer timer.Stop()
	for {
		action := ""
		select {
		case <-j.ctx.Done():
			return
		case action = <-j.actions:
		case <-timer.C:
		}
		if err = j.ctx.Err(); err != nil {
			return
		}
		if !m.remote.EditSessionActive(s.SessionID, s.Generation) {
			j.fail(fmt.Errorf("SSH会话已断开，本地副本已保留"))
			return
		}
		if action == "" {
			info, statErr := os.Lstat(s.LocalPath)
			if statErr != nil {
				j.fail(statErr)
				continue
			}
			if !sameFile(info, j.observed) {
				j.observed = info
				j.changedOnDisk = true
				continue
			}
			if !j.changedOnDisk {
				continue
			}
			j.changedOnDisk = false
		}
		current, err := readLocal(j)
		if err != nil {
			j.update(func(s *model.ExternalEdit) { s.Dirty = true })
			j.fail(err)
			continue
		}
		changed := hash(current) != j.baseline
		j.update(func(s *model.ExternalEdit) {
			s.Dirty = changed
			if s.Status != "error" {
				s.Status = "watching"
				if changed {
					s.Status = "changed"
				}
			}
		})
		state := j.snapshot()
		if action == "" && (!changed || !state.AutoUpload) {
			continue
		}
		if action == "auto" {
			j.update(func(s *model.ExternalEdit) { s.AutoUpload = true })
		}
		if !changed {
			if action == "once" {
				return
			}
			j.update(func(s *model.ExternalEdit) { s.Status = "watching"; s.Error = "" })
			continue
		}
		j.update(func(s *model.ExternalEdit) { s.Status = "uploading"; s.Error = "" })
		writeContext, cancelWrite := context.WithTimeout(j.ctx, 30*time.Second)
		revision, err = m.remote.WriteEditFile(writeContext, s.SessionID, s.Generation, s.RemotePath, current, j.revision)
		cancelWrite()
		if err != nil {
			j.fail(err)
			continue
		}
		j.baseline = hash(current)
		j.revision = revision
		after, err := readLocal(j)
		dirty := err != nil || hash(after) != j.baseline
		j.update(func(s *model.ExternalEdit) {
			s.Dirty = dirty
			s.Status = "watching"
			if dirty {
				s.Status = "changed"
			}
		})
		if action == "once" {
			if !dirty {
				return
			}
			j.update(func(s *model.ExternalEdit) {
				s.AutoUpload = false
				s.Error = "上传期间本地文件再次保存，请确认最新修改"
			})
		}
	}
}
