package mobilefiles

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"

	"golang.org/x/text/unicode/norm"
	"kitonyterms/internal/filepolicy"
)

const MaxBytes uint64 = 1 << 30
const MaxNodes = 50000
const MaxDepth = 64

var ErrUnavailable = errors.New("此平台尚未提供移动文件导入/导出")
var ErrCancelled = errors.New("文件选择或导出已取消")

// Request 仅在 Go 和本机适配间传递；不包含文档 URI，也不暴露到 IPC。
type Request struct {
	ID      string `json:"id"`
	StageID string `json:"stageID"`
	Stage   string `json:"stage"`
	Action  string `json:"action"`
	Name    string `json:"name"`
}
type Progress func(bytes uint64, files int)
type Driver interface {
	Available() bool
	Run(context.Context, Request, Progress) error
	Invalidate(string, context.CancelFunc)
}
type Manager struct {
	mu     sync.Mutex
	root   *os.Root
	path   string
	driver Driver
	ctx    context.Context
	stages map[string]*Stage
	closed bool
}
type Stage struct {
	manager  *Manager
	id       string
	ctx      context.Context
	cancel   context.CancelFunc
	opMu     sync.Mutex
	closed   bool
	closeErr error
}

func identifier() string {
	var bytes [16]byte
	if _, err := rand.Read(bytes[:]); err != nil {
		panic(err)
	}
	return hex.EncodeToString(bytes[:])
}
func validID(id string) bool {
	if len(id) != 32 {
		return false
	}
	_, err := hex.DecodeString(id)
	return err == nil && strings.ToLower(id) == id
}
func Available() bool { return platformDriver().Available() }
func Open(ctx context.Context, path string) (*Manager, error) {
	return open(ctx, path, platformDriver())
}
func open(ctx context.Context, path string, driver Driver) (*Manager, error) {
	if !driver.Available() {
		return nil, ErrUnavailable
	}
	if !filepath.IsAbs(path) {
		return nil, fmt.Errorf("移动暂存目录必须为绝对路径")
	}
	if err := os.Mkdir(path, 0700); err != nil && !os.IsExist(err) {
		return nil, err
	}
	info, err := os.Lstat(path)
	if err != nil {
		return nil, err
	}
	if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return nil, fmt.Errorf("移动暂存目录类型无效")
	}
	canonical, err := filepath.EvalSymlinks(path)
	if err != nil {
		return nil, err
	}
	root, err := os.OpenRoot(canonical)
	if err != nil {
		return nil, err
	}
	directory, err := root.Open(".")
	if err == nil {
		err = errors.Join(directory.Chmod(0700), directory.Close())
	}
	if err != nil {
		root.Close()
		return nil, err
	}
	m := &Manager{root: root, path: canonical, driver: driver, ctx: ctx, stages: map[string]*Stage{}}
	// 仅回收本模块的随机请求目录；未知项目保留并显式拒绝继续。
	dir, err := root.Open(".")
	if err == nil {
		defer dir.Close()
		for {
			entries, readErr := dir.ReadDir(256)
			for _, entry := range entries {
				if !validID(entry.Name()) || !entry.IsDir() || entry.Type()&os.ModeSymlink != 0 {
					err = fmt.Errorf("移动暂存存在未知项目，未清理")
					break
				}
				if err = root.RemoveAll(entry.Name()); err != nil {
					break
				}
			}
			if err != nil || readErr == io.EOF {
				break
			}
			if readErr != nil {
				err = readErr
				break
			}
		}
	}
	if err != nil {
		root.Close()
		return nil, err
	}
	return m, nil
}
func (m *Manager) NewStage() (*Stage, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.closed || m.ctx.Err() != nil {
		return nil, fmt.Errorf("移动文件服务已关闭")
	}
	if len(m.stages) >= 16 {
		return nil, fmt.Errorf("移动文件请求过多")
	}
	id := identifier()
	if err := m.root.Mkdir(id, 0700); err != nil {
		return nil, err
	}
	ctx, cancel := context.WithCancel(m.ctx)
	s := &Stage{manager: m, id: id, ctx: ctx, cancel: cancel}
	m.stages[id] = s
	m.driver.Invalidate(id, cancel)
	return s, nil
}
func (s *Stage) Context() context.Context { return s.ctx }
func (s *Stage) Path() string             { return filepath.Join(s.manager.path, s.id) }
func (s *Stage) run(ctx context.Context, action, name string, progress Progress) error {
	s.opMu.Lock()
	defer s.opMu.Unlock()
	if s.closed {
		return fmt.Errorf("移动文件请求已释放")
	}
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	stop := context.AfterFunc(s.ctx, cancel)
	defer stop()
	if err := ctx.Err(); err != nil {
		return err
	}
	return s.manager.driver.Run(ctx, Request{ID: identifier(), StageID: s.id, Stage: s.Path(), Action: action, Name: name}, progress)
}
func (s *Stage) Import(ctx context.Context, directory bool) ([]string, error) {
	action := "import_files"
	if directory {
		action = "import_tree"
	}
	if err := s.run(ctx, action, "", nil); err != nil {
		return nil, err
	}
	return s.Files()
}
func (s *Stage) SelectExport(ctx context.Context, name string) error {
	action := "select_tree"
	if name != "" {
		if !filepolicy.Name(name) {
			return fmt.Errorf("导出文件名无效")
		}
		action = "select_file"
	}
	return s.run(ctx, action, name, nil)
}
func (s *Stage) Export(ctx context.Context, progress Progress) error {
	if _, err := s.Files(); err != nil {
		return err
	}
	return s.run(ctx, "export", "", progress)
}
func (s *Stage) Files() ([]string, error) {
	root, err := os.OpenRoot(s.Path())
	if err != nil {
		return nil, err
	}
	defer root.Close()
	seen := map[string]bool{}
	var count int
	var bytes uint64
	var walk func(string, int) error
	walk = func(relative string, depth int) error {
		if err := s.ctx.Err(); err != nil {
			return err
		}
		count++
		if depth > MaxDepth || count > MaxNodes {
			return fmt.Errorf("移动文件层级或项目数超过限制")
		}
		if !filepolicy.Name(filepath.Base(relative)) {
			return fmt.Errorf("移动文件名称无效")
		}
		key := strings.ToLower(norm.NFC.String(filepath.ToSlash(relative)))
		if seen[key] {
			return fmt.Errorf("移动文件名称冲突")
		}
		seen[key] = true
		info, err := root.Lstat(relative)
		if err != nil {
			return err
		}
		if info.Mode().IsRegular() {
			if info.Size() < 0 || uint64(info.Size()) > MaxBytes-bytes {
				return fmt.Errorf("移动文件总量超过 1GiB 限制")
			}
			bytes += uint64(info.Size())
			return nil
		}
		if !info.IsDir() {
			return fmt.Errorf("移动文件不支持链接或特殊类型")
		}
		dir, err := root.Open(relative)
		if err != nil {
			return err
		}
		defer dir.Close()
		for {
			entries, readErr := dir.ReadDir(256)
			for _, entry := range entries {
				if err := walk(filepath.Join(relative, entry.Name()), depth+1); err != nil {
					return err
				}
			}
			if readErr == io.EOF {
				return nil
			}
			if readErr != nil {
				return readErr
			}
		}
	}
	dir, err := root.Open(".")
	if err != nil {
		return nil, err
	}
	defer dir.Close()
	entries, err := dir.ReadDir(1025)
	if err != nil && err != io.EOF {
		return nil, err
	}
	if len(entries) == 0 || len(entries) > 1024 {
		return nil, fmt.Errorf("请选择 1..1024 个移动文件或目录")
	}
	files := make([]string, 0, len(entries))
	for _, entry := range entries {
		if err := walk(entry.Name(), 0); err != nil {
			return nil, err
		}
		files = append(files, filepath.Join(s.Path(), entry.Name()))
	}
	return files, nil
}
func (s *Stage) Close() error {
	s.cancel()
	s.opMu.Lock()
	defer s.opMu.Unlock()
	if s.closed {
		return s.closeErr
	}
	s.closed = true
	// 原生取消必须实际返回才会获得 opMu；授权释放与本地清理也在终态前完成。
	err := s.manager.driver.Run(context.Background(), Request{ID: identifier(), StageID: s.id, Stage: s.Path(), Action: "release"}, nil)
	s.manager.driver.Invalidate(s.id, nil)
	cleanupErr := s.manager.root.RemoveAll(s.id)
	if cleanupErr != nil {
		cleanupErr = fmt.Errorf("移动暂存清理失败: %w", cleanupErr)
	}
	s.closeErr = errors.Join(err, cleanupErr)
	s.manager.mu.Lock()
	delete(s.manager.stages, s.id)
	s.manager.mu.Unlock()
	return s.closeErr
}
func (m *Manager) Close() error {
	m.mu.Lock()
	m.closed = true
	stages := make([]*Stage, 0, len(m.stages))
	for _, s := range m.stages {
		s.cancel()
		stages = append(stages, s)
	}
	m.mu.Unlock()
	var err error
	for _, s := range stages {
		err = errors.Join(err, s.Close())
	}
	return errors.Join(err, m.root.Close())
}

func (m *Manager) Cancel() {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.closed = true
	for _, s := range m.stages {
		s.cancel()
	}
}
