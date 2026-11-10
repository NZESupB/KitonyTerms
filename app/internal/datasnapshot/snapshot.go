// Package datasnapshot 提供显式指定目录的离线备份与新目录恢复，不接管运行中的应用。
package datasnapshot

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"sort"
	"strings"

	"github.com/gofrs/flock"
	"golang.org/x/text/unicode/norm"

	"kitonyterms/internal/filepolicy"
)

const (
	MaxBytes     = int64(8 << 30)
	MaxNodes     = 50000
	MaxDepth     = 64
	maxManifest  = 16 << 20
	manifestName = "manifest.json"
)

var lockNames = []string{"kitonyterms.lock", "updates/helper.lock", "updates/installation.lock"}

// Entry 的相对路径及摘要仅写入私有清单，不用于日志。
type Entry struct {
	Path       string `json:"path"`
	Directory  bool   `json:"directory"`
	Bytes      int64  `json:"bytes"`
	SHA256     string `json:"sha256,omitempty"`
	Executable bool   `json:"executable,omitempty"`
}

type Manifest struct {
	Version int     `json:"version"`
	Entries []Entry `json:"entries"`
}

type Summary struct {
	Files         int   `json:"files"`
	Directories   int   `json:"directories"`
	Bytes         int64 `json:"bytes"`
	PendingUpdate bool  `json:"pendingUpdate"`
}

// Failure 不把目录名、文件名或底层错误内容输出到命令行。
type Failure struct {
	Stage string
	Cause error
}

func (e *Failure) Error() string {
	reason := e.Cause.Error()
	var pathError *os.PathError
	if errors.As(e.Cause, &pathError) {
		reason = pathError.Err.Error()
	}
	var syntaxError *json.SyntaxError
	var typeError *json.UnmarshalTypeError
	if strings.HasPrefix(reason, "json:") || errors.As(e.Cause, &syntaxError) || errors.As(e.Cause, &typeError) {
		reason = "完成清单格式或字段无效"
	}
	return "目录快照操作失败：" + e.Stage + "：" + reason + "；未完成目标保留"
}
func (e *Failure) Unwrap() error            { return e.Cause }
func failure(stage string, err error) error { return &Failure{Stage: stage, Cause: err} }

func canonical(path string) (string, error) {
	if !filepath.IsAbs(path) {
		return "", errors.New("必须显式使用绝对路径")
	}
	info, err := os.Lstat(path)
	if err != nil {
		return "", err
	}
	if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return "", errors.New("源必须是实际目录")
	}
	return filepath.EvalSymlinks(filepath.Clean(path))
}
func overlaps(a, b string) bool {
	if runtime.GOOS == "windows" {
		a = strings.ToLower(a)
		b = strings.ToLower(b)
	}
	within := func(parent, child string) bool {
		relative, err := filepath.Rel(parent, child)
		return err == nil && (relative == "." || relative != ".." && !strings.HasPrefix(relative, ".."+string(filepath.Separator)))
	}
	return within(a, b) || within(b, a)
}
func openSources(config, data string) (map[string]*os.Root, []string, error) {
	paths := []string{config, data}
	for i, path := range paths {
		value, err := canonical(path)
		if err != nil {
			return nil, nil, err
		}
		paths[i] = value
	}
	if paths[0] != paths[1] && overlaps(paths[0], paths[1]) {
		return nil, nil, errors.New("配置与数据目录不能相互嵌套")
	}
	roots := map[string]*os.Root{}
	for i, name := range []string{"config", "data"} {
		root, err := os.OpenRoot(paths[i])
		if err != nil {
			closeRoots(roots)
			return nil, nil, err
		}
		roots[name] = root
	}
	return roots, paths, nil
}
func closeRoots(roots map[string]*os.Root) error {
	var result error
	for _, name := range []string{"data", "config"} {
		if root := roots[name]; root != nil {
			result = errors.Join(result, root.Close())
		}
	}
	return result
}
func acquireSourceLocks(root *os.Root, path string) (release func() error, validate func() error, err error) {
	var locks []*flock.Flock
	identities := map[string]fs.FileInfo{}
	release = func() error {
		var result error
		for i := len(locks) - 1; i >= 0; i-- {
			result = errors.Join(result, locks[i].Close())
		}
		return result
	}
	defer func() {
		if err != nil {
			err = errors.Join(err, release())
		}
	}()
	for index, name := range lockNames {
		info, checkErr := root.Lstat(filepath.FromSlash(name))
		identities[name] = info
		if errors.Is(checkErr, fs.ErrNotExist) && index > 0 {
			continue
		}
		if checkErr != nil {
			return release, nil, errors.Join(errors.New("缺少可用的离线应用锁"), checkErr)
		}
		if !info.Mode().IsRegular() {
			return release, nil, errors.New("源锁必须是普通文件")
		}
		lock := flock.New(filepath.Join(path, filepath.FromSlash(name)), flock.SetFlag(os.O_RDWR))
		locked, lockErr := lock.TryLock()
		if lockErr != nil || !locked {
			return release, nil, errors.Join(errors.New("源应用或更新进程仍在运行"), lockErr, lock.Close())
		}
		locks = append(locks, lock)
	}
	validate = func() error {
		for name, before := range identities {
			after, checkErr := root.Lstat(filepath.FromSlash(name))
			if before == nil && errors.Is(checkErr, fs.ErrNotExist) {
				continue
			}
			if checkErr != nil || before == nil || !after.Mode().IsRegular() || !os.SameFile(before, after) {
				return errors.Join(errors.New("源锁身份变化"), checkErr)
			}
		}
		return nil
	}
	if err = validate(); err != nil {
		return release, nil, err
	}
	return release, validate, nil
}
func newDestination(output string, sources []string) (*os.Root, error) {
	if !filepath.IsAbs(output) {
		return nil, errors.New("目标必须是绝对路径")
	}
	output = filepath.Clean(output)
	parent, err := canonical(filepath.Dir(output))
	if err != nil {
		return nil, err
	}
	name := filepath.Base(output)
	if !filepolicy.Name(name) {
		return nil, errors.New("目标名称不可跨平台保存")
	}
	for _, source := range sources {
		if overlaps(filepath.Join(parent, name), source) {
			return nil, errors.New("源与目标不能重叠")
		}
	}
	root, err := os.OpenRoot(parent)
	if err != nil {
		return nil, err
	}
	defer root.Close()
	if err = root.Mkdir(name, 0700); err != nil {
		return nil, err
	}
	destination, err := root.OpenRoot(name)
	if err != nil {
		return nil, err
	}
	if err = protectDestination(destination); err != nil {
		return nil, errors.Join(errors.New("目标私有权限设置失败"), err, destination.Close())
	}
	if err = syncDirectory(root, "."); err != nil {
		return nil, errors.Join(err, destination.Close())
	}
	return destination, nil
}
func isLock(name string) bool {
	for _, lock := range lockNames {
		if name == lock {
			return true
		}
	}
	return false
}
func validRelative(name string) error {
	if !fs.ValidPath(name) || strings.Contains(name, "\\") || len(strings.Split(name, "/")) > MaxDepth {
		return errors.New("路径或深度无效")
	}
	for _, part := range strings.Split(name, "/") {
		if !filepolicy.Name(part) {
			return errors.New("文件名不可跨平台保存")
		}
	}
	return nil
}

type contextReader struct {
	ctx    context.Context
	reader io.Reader
}

func (r contextReader) Read(p []byte) (int, error) {
	if err := r.ctx.Err(); err != nil {
		return 0, err
	}
	return r.reader.Read(p)
}

func readFile(ctx context.Context, root *os.Root, name string, write io.Writer) (entry Entry, err error) {
	info, err := root.Lstat(name)
	if err != nil {
		return entry, err
	}
	if !info.Mode().IsRegular() {
		return entry, errors.New("只支持普通文件")
	}
	if info.Size() < 0 || info.Size() > MaxBytes {
		return entry, errors.New("文件超限")
	}
	file, err := root.Open(name)
	if err != nil {
		return entry, err
	}
	defer func() { err = errors.Join(err, file.Close()) }()
	opened, err := file.Stat()
	if err != nil {
		return entry, err
	}
	if !opened.Mode().IsRegular() || !os.SameFile(info, opened) {
		return entry, errors.New("打开的文件身份变化")
	}
	hash := sha256.New()
	var writer io.Writer = hash
	if write != nil {
		writer = io.MultiWriter(write, hash)
	}
	count, err := io.Copy(writer, io.LimitReader(contextReader{ctx, file}, info.Size()+1))
	if err != nil {
		return entry, err
	}
	after, err := root.Lstat(name)
	if err != nil {
		return entry, err
	}
	final, err := file.Stat()
	if err != nil {
		return entry, err
	}
	if count != info.Size() || !os.SameFile(info, after) || !os.SameFile(info, final) || info.Size() != final.Size() || !info.ModTime().Equal(final.ModTime()) || info.Mode() != final.Mode() {
		return entry, errors.New("读取期间源文件变化")
	}
	return Entry{Bytes: count, SHA256: hex.EncodeToString(hash.Sum(nil)), Executable: info.Mode().Perm()&0111 != 0}, nil
}
func scan(ctx context.Context, roots map[string]*os.Root, skipLocks bool) ([]Entry, error) {
	var entries []Entry
	var total int64
	seen := map[string]bool{}
	for _, scope := range []string{"config", "data"} {
		root := roots[scope]
		if root == nil {
			return nil, errors.New("缺少目录")
		}
		err := walk(ctx, root, ".", func(name string, info fs.FileInfo) error {
			if name != "." && skipLocks && isLock(name) && (scope == "data" || roots["config"].Name() == roots["data"].Name()) {
				if !info.Mode().IsRegular() {
					return errors.New("锁类型无效")
				}
				return nil
			}
			if name != "." {
				if err := validRelative(name); err != nil {
					return err
				}
			}
			full := scope
			if name != "." {
				full += "/" + name
			}
			key := strings.ToLower(norm.NFC.String(full))
			if seen[key] {
				return errors.New("大小写或Unicode路径冲突")
			}
			seen[key] = true
			if len(entries) >= MaxNodes {
				return errors.New("节点数量超限")
			}
			entry := Entry{Path: full, Directory: info.IsDir()}
			if !info.IsDir() {
				var err error
				entry, err = readFile(ctx, root, filepath.FromSlash(name), nil)
				if err != nil {
					return err
				}
				entry.Path = full
				if entry.Bytes > MaxBytes-total {
					return errors.New("目录总字节超限")
				}
				total += entry.Bytes
			}
			entries = append(entries, entry)
			return nil
		})
		if err != nil {
			return nil, err
		}
	}
	sort.Slice(entries, func(i, j int) bool { return entries[i].Path < entries[j].Path })
	return entries, nil
}
func summary(entries []Entry) Summary {
	var result Summary
	for _, entry := range entries {
		if entry.Directory {
			result.Directories++
		} else {
			result.Files++
			result.Bytes += entry.Bytes
		}
		switch entry.Path {
		case "data/updates/task.json", "data/updates/journal.json", "data/updates/relaunch.json":
			result.PendingUpdate = true
		}
	}
	return result
}
func syncDirectory(root *os.Root, name string) (err error) {
	file, err := root.Open(name)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, file.Close()) }()
	if runtime.GOOS == "windows" {
		return nil
	}
	return file.Sync()
}
func copyEntries(ctx context.Context, roots map[string]*os.Root, destination *os.Root, entries []Entry) error {
	for _, entry := range entries {
		if err := ctx.Err(); err != nil {
			return err
		}
		name := filepath.FromSlash(entry.Path)
		if entry.Directory {
			if err := destination.Mkdir(name, 0700); err != nil {
				return err
			}
			continue
		}
		scope, relative, ok := strings.Cut(entry.Path, "/")
		if !ok {
			return errors.New("文件缺少所属目录")
		}
		mode := os.FileMode(0600)
		if entry.Executable {
			mode = 0700
		}
		file, err := destination.OpenFile(name, os.O_WRONLY|os.O_CREATE|os.O_EXCL, mode)
		if err != nil {
			return err
		}
		actual, readErr := readFile(ctx, roots[scope], filepath.FromSlash(relative), file)
		actual.Path = entry.Path
		if readErr == nil && !reflect.DeepEqual(actual, entry) {
			readErr = errors.New("复制内容与预检清单不同")
		}
		if readErr == nil {
			readErr = file.Sync()
		}
		if err = errors.Join(readErr, file.Close()); err != nil {
			return err
		}
	}
	for index := len(entries) - 1; index >= 0; index-- {
		if entries[index].Directory {
			if err := syncDirectory(destination, filepath.FromSlash(entries[index].Path)); err != nil {
				return err
			}
		}
	}
	return syncDirectory(destination, ".")
}
func childRoots(root *os.Root) (map[string]*os.Root, error) {
	children := map[string]*os.Root{}
	for _, name := range []string{"config", "data"} {
		info, err := root.Lstat(name)
		if err != nil {
			closeRoots(children)
			return nil, err
		}
		if !info.IsDir() {
			closeRoots(children)
			return nil, errors.New("备份目录类型无效")
		}
		child, err := root.OpenRoot(name)
		if err != nil {
			closeRoots(children)
			return nil, err
		}
		children[name] = child
	}
	return children, nil
}
func publish(ctx context.Context, root *os.Root, name string, manifest Manifest) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	data, err := json.MarshalIndent(manifest, "", "  ")
	if err != nil {
		return err
	}
	if len(data) > maxManifest {
		return errors.New("清单超限")
	}
	if _, err = root.Lstat(name); !errors.Is(err, fs.ErrNotExist) {
		return errors.Join(errors.New("完成文件必须不存在"), err)
	}
	pending := name + ".partial"
	file, err := root.OpenFile(pending, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return err
	}
	_, writeErr := file.Write(append(data, '\n'))
	if writeErr == nil {
		writeErr = file.Sync()
	}
	if err = errors.Join(writeErr, file.Close()); err != nil {
		return err
	}
	if err = ctx.Err(); err != nil {
		return err
	}
	if err = root.Rename(pending, name); err != nil {
		return err
	}
	if err = syncDirectory(root, "."); err != nil {
		// 已知目录刷盘失败时退回未完成名，保留清单及复制材料。
		return errors.Join(err, root.Rename(name, pending), syncDirectory(root, "."))
	}
	return nil
}

// Create 只读已停止应用的config/data内容；失败时保留没有有效完成清单的新目标。
func Create(ctx context.Context, config, data, output string) (result Summary, err error) {
	defer func() {
		if err != nil {
			err = failure("创建快照", err)
		}
	}()
	roots, paths, err := openSources(config, data)
	if err != nil {
		return result, err
	}
	defer func() { err = errors.Join(err, closeRoots(roots)) }()
	release, validateLocks, err := acquireSourceLocks(roots["data"], paths[1])
	if err != nil {
		return result, err
	}
	defer func() { err = errors.Join(err, release()) }()
	before, err := scan(ctx, roots, true)
	if err != nil {
		return result, err
	}
	destination, err := newDestination(output, paths)
	if err != nil {
		return result, err
	}
	defer func() { err = errors.Join(err, destination.Close()) }()
	if err = copyEntries(ctx, roots, destination, before); err != nil {
		return result, err
	}
	after, err := scan(ctx, roots, true)
	if err != nil {
		return result, err
	}
	if !reflect.DeepEqual(before, after) {
		return result, errors.New("快照期间源目录变化")
	}
	children, err := childRoots(destination)
	if err != nil {
		return result, err
	}
	copied, scanErr := scan(ctx, children, false)
	err = errors.Join(scanErr, closeRoots(children))
	if err != nil {
		return result, err
	}
	if !reflect.DeepEqual(before, copied) {
		return result, errors.New("目标回读与源清单不符")
	}
	if err = validateLocks(); err != nil {
		return result, err
	}
	for index, scope := range []string{"config", "data"} {
		original, statErr := roots[scope].Stat(".")
		if statErr != nil {
			return result, statErr
		}
		actual, statErr := os.Lstat(paths[index])
		if statErr != nil {
			return result, statErr
		}
		if !actual.IsDir() || !os.SameFile(original, actual) {
			return result, errors.New("源目录身份变化")
		}
	}
	if err = publish(ctx, destination, manifestName, Manifest{Version: 1, Entries: before}); err != nil {
		return result, err
	}
	return summary(before), nil
}
func verify(ctx context.Context, root *os.Root) (manifest Manifest, err error) {
	info, err := root.Lstat(manifestName)
	if err != nil {
		return manifest, err
	}
	if !info.Mode().IsRegular() || info.Size() > maxManifest {
		return manifest, errors.New("完成清单无效")
	}
	file, err := root.Open(manifestName)
	if err != nil {
		return manifest, err
	}
	opened, statErr := file.Stat()
	if statErr != nil || !os.SameFile(info, opened) {
		return manifest, errors.Join(errors.New("完成清单身份变化"), statErr, file.Close())
	}
	decoder := json.NewDecoder(io.LimitReader(contextReader{ctx, file}, maxManifest+1))
	decoder.DisallowUnknownFields()
	err = decoder.Decode(&manifest)
	if err == nil {
		var extra any
		if decoder.Decode(&extra) != io.EOF {
			err = errors.New("完成清单含多余内容")
		}
	}
	err = errors.Join(err, file.Close())
	if err != nil {
		return manifest, err
	}
	if manifest.Version != 1 || len(manifest.Entries) > MaxNodes {
		return manifest, errors.New("不支持的完成清单")
	}
	entries, err := rootEntries(root)
	if err != nil {
		return manifest, err
	}
	if len(entries) != 3 {
		return manifest, errors.New("备份根目录含额外或缺失项目")
	}
	for _, entry := range entries {
		if entry.Name() != "config" && entry.Name() != "data" && entry.Name() != manifestName {
			return manifest, errors.New("备份根目录含额外项目")
		}
	}
	children, err := childRoots(root)
	if err != nil {
		return manifest, err
	}
	actual, scanErr := scan(ctx, children, false)
	err = errors.Join(scanErr, closeRoots(children))
	if err != nil {
		return manifest, err
	}
	if !reflect.DeepEqual(actual, manifest.Entries) {
		return manifest, errors.New("备份摘要或完整清单不匹配")
	}
	return manifest, nil
}

// Verify 验证字节和完整性，不解密vault、不执行恢复材料、不证明备份来源身份。
func Verify(ctx context.Context, path string) (result Summary, err error) {
	defer func() {
		if err != nil {
			err = failure("校验快照", err)
		}
	}()
	path, err = canonical(path)
	if err != nil {
		return result, err
	}
	root, err := os.OpenRoot(path)
	if err != nil {
		return result, err
	}
	defer func() { err = errors.Join(err, root.Close()) }()
	manifest, err := verify(ctx, root)
	if err != nil {
		return result, err
	}
	return summary(manifest.Entries), nil
}

// Restore 只写全新根目录，不覆盖现有目录，不改写更新记录中的绝对路径，也不启动应用。
func Restore(ctx context.Context, snapshot, output string) (result Summary, err error) {
	defer func() {
		if err != nil {
			err = failure("恢复新目录", err)
		}
	}()
	snapshot, err = canonical(snapshot)
	if err != nil {
		return result, err
	}
	root, err := os.OpenRoot(snapshot)
	if err != nil {
		return result, err
	}
	defer func() { err = errors.Join(err, root.Close()) }()
	manifest, err := verify(ctx, root)
	if err != nil {
		return result, err
	}
	destination, err := newDestination(output, []string{snapshot})
	if err != nil {
		return result, err
	}
	defer func() { err = errors.Join(err, destination.Close()) }()
	children, err := childRoots(root)
	if err != nil {
		return result, err
	}
	defer func() { err = errors.Join(err, closeRoots(children)) }()
	if err = copyEntries(ctx, children, destination, manifest.Entries); err != nil {
		return result, err
	}
	if _, err = verify(ctx, root); err != nil {
		return result, err
	}
	written, err := childRoots(destination)
	if err != nil {
		return result, err
	}
	actual, scanErr := scan(ctx, written, false)
	err = errors.Join(scanErr, closeRoots(written))
	if err != nil {
		return result, err
	}
	if !reflect.DeepEqual(manifest.Entries, actual) {
		return result, errors.New("恢复后回读不符")
	}
	if err = publish(ctx, destination, ".kitonyterms-restored.json", manifest); err != nil {
		return result, err
	}
	return summary(manifest.Entries), nil
}
