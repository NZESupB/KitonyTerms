package update

import (
	"archive/tar"
	"archive/zip"
	"bufio"
	"compress/gzip"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"strings"
	"unicode"
	"unicode/utf8"

	"golang.org/x/text/unicode/norm"
)

const MaxExpanded uint64 = 1 << 30
const maxArchiveEntries = 20000

type archiveLimits struct {
	bytes   uint64
	entries int
}

type archivePath struct {
	name                string
	directory, explicit bool
}

type archiveTree struct {
	root    *os.Root
	ctx     context.Context
	limits  archiveLimits
	paths   map[string]archivePath
	entries int
	total   uint64
}

func newArchiveTree(ctx context.Context, root *os.Root, limits archiveLimits) *archiveTree {
	return &archiveTree{root: root, ctx: ctx, limits: limits, paths: make(map[string]archivePath)}
}

func archiveComponent(name string) bool {
	if name == "" || name == "." || name == ".." || len(name) > 255 || strings.HasSuffix(name, ".") || strings.HasSuffix(name, " ") || strings.ContainsAny(name, "\\:<>\"|?*") {
		return false
	}
	for _, c := range name {
		if unicode.IsControl(c) {
			return false
		}
	}
	stem := strings.ToUpper(strings.SplitN(name, ".", 2)[0])
	switch stem {
	case "CON", "PRN", "AUX", "NUL", "CLOCK$", "CONIN$", "CONOUT$":
		return false
	}
	runes := []rune(stem)
	if len(runes) == 4 && (strings.HasPrefix(stem, "COM") || strings.HasPrefix(stem, "LPT")) && strings.ContainsRune("123456789¹²³", runes[3]) {
		return false
	}
	return true
}

func (t *archiveTree) claim(raw string, directory bool) (string, error) {
	if err := t.ctx.Err(); err != nil {
		return "", err
	}
	if !utf8.ValidString(raw) || len(raw) > 4096 || strings.Contains(raw, "\\") {
		return "", fmt.Errorf("归档路径编码或分隔符无效")
	}
	if directory {
		raw = strings.TrimSuffix(raw, "/")
	}
	parts := strings.Split(raw, "/")
	if len(parts) > 32 || t.entries >= t.limits.entries {
		return "", fmt.Errorf("归档条目或深度超过上限")
	}
	for _, part := range parts {
		if !archiveComponent(part) {
			return "", fmt.Errorf("归档路径不安全")
		}
	}
	for i := range parts {
		name := strings.Join(parts[:i+1], "/")
		key := strings.ToLower(norm.NFC.String(name))
		last := i == len(parts)-1
		isDir := !last || directory
		if previous, ok := t.paths[key]; ok {
			if previous.name != name || !previous.directory || !isDir || last && previous.explicit {
				return "", fmt.Errorf("归档路径重复或类型冲突")
			}
			if last {
				previous.explicit = true
				t.paths[key] = previous
			}
		} else {
			t.paths[key] = archivePath{name: name, directory: isDir, explicit: last}
		}
	}
	t.entries++
	return strings.Join(parts, "/"), nil
}

func (t *archiveTree) directory(name string) error { return t.root.MkdirAll(name, 0700) }

func (t *archiveTree) write(name string, size uint64, mode fs.FileMode, reader io.Reader) (err error) {
	if size > t.limits.bytes-t.total {
		return fmt.Errorf("归档展开体积超过上限")
	}
	if err = t.root.MkdirAll(path.Dir(name), 0700); err != nil {
		return err
	}
	file, err := t.root.OpenFile(name, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, file.Close()) }()
	n, err := io.CopyBuffer(file, io.LimitReader(contextReader{t.ctx, reader}, int64(size)+1), make([]byte, 64<<10))
	if err != nil {
		return err
	}
	if uint64(n) != size {
		return fmt.Errorf("归档条目大小不匹配")
	}
	t.total += uint64(n)
	if err = file.Sync(); err != nil {
		return err
	}
	permissions := mode.Perm()
	if permissions == 0 {
		permissions = 0600
	}
	return file.Chmod(permissions)
}

// 先限制中央目录，避免 ZIP 解析器在条目数量检查前分配无界列表。
func preflightZIP(file *os.File, size int64, limit int) error {
	n := min(size, int64(65557))
	if n < 22 {
		return fmt.Errorf("ZIP 结束记录缺失")
	}
	data := make([]byte, int(n))
	if _, err := file.ReadAt(data, size-n); err != nil {
		return err
	}
	for offset := len(data) - 22; offset >= 0; offset-- {
		b := data[offset:]
		if string(b[:4]) != "PK\x05\x06" || offset+22+int(binary.LittleEndian.Uint16(b[20:22])) != len(data) {
			continue
		}
		entries := binary.LittleEndian.Uint16(b[10:12])
		centralSize := binary.LittleEndian.Uint32(b[12:16])
		centralOffset := binary.LittleEndian.Uint32(b[16:20])
		if binary.LittleEndian.Uint16(b[4:6]) != 0 || binary.LittleEndian.Uint16(b[6:8]) != 0 || binary.LittleEndian.Uint16(b[8:10]) != entries || int(entries) > limit || entries == 0xffff || centralSize > 32<<20 || int64(centralOffset)+int64(centralSize) != size-n+int64(offset) {
			return fmt.Errorf("ZIP 中央目录无效或超过上限")
		}
		return nil
	}
	return fmt.Errorf("ZIP 结束记录无效")
}

func extractZIP(t *archiveTree, file *os.File, size int64) error {
	if err := preflightZIP(file, size, t.limits.entries); err != nil {
		return err
	}
	archive, err := zip.NewReader(file, size)
	if err != nil {
		return err
	}
	if len(archive.File) > t.limits.entries {
		return fmt.Errorf("ZIP 条目超过上限")
	}
	for _, entry := range archive.File {
		mode := entry.Mode()
		if mode.Type() != 0 && mode.Type() != fs.ModeDir || entry.Flags&1 != 0 || entry.Method != zip.Store && entry.Method != zip.Deflate {
			return fmt.Errorf("不支持的 ZIP 文件类型或压缩方式")
		}
		name, err := t.claim(entry.Name, mode.IsDir())
		if err != nil {
			return err
		}
		if mode.IsDir() {
			if entry.UncompressedSize64 != 0 {
				return fmt.Errorf("ZIP 目录包含数据")
			}
			if err = t.directory(name); err != nil {
				return err
			}
			continue
		}
		if entry.UncompressedSize64 > t.limits.bytes-t.total {
			return fmt.Errorf("归档展开体积超过上限")
		}
		reader, err := entry.Open()
		if err != nil {
			return err
		}
		err = t.write(name, entry.UncompressedSize64, mode, reader)
		err = errors.Join(err, reader.Close())
		if err != nil {
			return err
		}
	}
	return t.ctx.Err()
}

func extractTAR(t *archiveTree, file *os.File) error {
	buffer := bufio.NewReader(contextReader{t.ctx, file})
	compressed, err := gzip.NewReader(buffer)
	if err != nil {
		return err
	}
	defer compressed.Close()
	compressed.Multistream(false)
	limited := &io.LimitedReader{R: contextReader{t.ctx, compressed}, N: int64(t.limits.bytes) + (32 << 20) + 1}
	archive := tar.NewReader(limited)
	for {
		entry, err := archive.Next()
		if err == io.EOF {
			break
		}
		if err != nil {
			return err
		}
		if entry.Typeflag != tar.TypeReg && entry.Typeflag != tar.TypeRegA && entry.Typeflag != tar.TypeDir || entry.Size < 0 {
			return fmt.Errorf("不支持的 TAR 文件类型")
		}
		name, err := t.claim(entry.Name, entry.Typeflag == tar.TypeDir)
		if err != nil {
			return err
		}
		if entry.Typeflag == tar.TypeDir {
			if entry.Size != 0 {
				return fmt.Errorf("TAR 目录包含数据")
			}
			if err = t.directory(name); err != nil {
				return err
			}
		} else if err = t.write(name, uint64(entry.Size), fs.FileMode(entry.Mode), archive); err != nil {
			return err
		}
	}
	// 读至 gzip 校验尾；只接受 TAR 的有界零填充，拒绝拼接归档与隐藏载荷。
	padding := make([]byte, 4096)
	remaining := int64(1 << 20)
	for {
		n, err := limited.Read(padding)
		remaining -= int64(n)
		if remaining < 0 || limited.N == 0 {
			return fmt.Errorf("TAR 填充或元数据超过上限")
		}
		for _, b := range padding[:n] {
			if b != 0 {
				return fmt.Errorf("TAR 结束后存在额外数据")
			}
		}
		if err == io.EOF {
			break
		}
		if err != nil {
			return err
		}
	}
	if _, err = buffer.Peek(1); err != io.EOF {
		return fmt.Errorf("gzip 结束后存在额外数据")
	}
	return t.ctx.Err()
}

func openVerifiedPackage(ctx context.Context, root *os.Root, pkg Package) (*os.File, error) {
	if err := pkg.Validate(); err != nil {
		return nil, err
	}
	name := "package-" + strings.ToLower(pkg.SHA256)
	info, err := root.Lstat(name)
	if err != nil {
		return nil, err
	}
	if !info.Mode().IsRegular() || info.Size() != int64(pkg.Size) {
		return nil, fmt.Errorf("更新包类型或大小不匹配")
	}
	file, err := root.Open(name)
	if err != nil {
		return nil, err
	}
	fail := func(err error) (*os.File, error) { return nil, errors.Join(err, file.Close()) }
	opened, err := file.Stat()
	if err != nil {
		return fail(err)
	}
	if !os.SameFile(info, opened) || !opened.Mode().IsRegular() {
		return fail(fmt.Errorf("更新包在打开时改变"))
	}
	hash := sha256.New()
	n, err := io.Copy(hash, io.LimitReader(contextReader{ctx, file}, int64(pkg.Size)+1))
	if err != nil {
		return fail(err)
	}
	if n != int64(pkg.Size) || hex.EncodeToString(hash.Sum(nil)) != strings.ToLower(pkg.SHA256) {
		return fail(fmt.Errorf("更新包摘要或大小不匹配"))
	}
	if _, err = file.Seek(0, io.SeekStart); err != nil {
		return fail(err)
	}
	return file, nil
}

type preparedPayload struct {
	Directory  string
	Payload    string
	Executable string
	SHA256     string
}

// 仅准备私有载荷；不修改安装目标，也不创建可被旧版误读的安装记录。
func preparePayload(ctx context.Context, root *os.Root, pkg Package, next BuildInfo) (prepared preparedPayload, err error) {
	if err = pendingInstallation(root); err != nil {
		return prepared, err
	}
	return extractVerifiedPayload(ctx, root, pkg, next)
}

// helper 已持有安装锁并验证 task；已有 task 不阻止其重新验证、展开同一包。
func extractVerifiedPayload(ctx context.Context, root *os.Root, pkg Package, next BuildInfo) (prepared preparedPayload, err error) {
	if err = next.Validate(); err != nil {
		return prepared, err
	}
	if next.Target != desktopTarget() {
		return prepared, fmt.Errorf("不能执行其他平台的更新程序")
	}
	manifest := Manifest{Targets: map[string]targetPackages{next.Target: {Archive: pkg}}}
	if _, err = manifest.packageFor(next.Target, false); err != nil {
		return prepared, err
	}
	file, err := openVerifiedPackage(ctx, root, pkg)
	if err != nil {
		return prepared, err
	}
	defer func() {
		if file != nil {
			err = errors.Join(err, file.Close())
		}
	}()
	name := ".payload-" + rand.Text()
	if err = root.Mkdir(name, 0700); err != nil {
		return prepared, err
	}
	defer func() {
		if err != nil {
			err = errors.Join(err, root.RemoveAll(name))
		}
	}()
	destination, err := root.OpenRoot(name)
	if err != nil {
		return prepared, err
	}
	defer func() { err = errors.Join(err, destination.Close()) }()
	tree := newArchiveTree(ctx, destination, archiveLimits{MaxExpanded, maxArchiveEntries})
	switch pkg.Kind {
	case "zip":
		err = extractZIP(tree, file, int64(pkg.Size))
	case "tar_gz":
		err = extractTAR(tree, file)
	case "dmg":
		err = extractDMG(ctx, file, tree, pkg)
	default:
		err = fmt.Errorf("此更新包不能解包")
	}
	if err != nil {
		return prepared, err
	}
	binaryName := "kitonyterms.exe"
	if strings.Contains(next.Target, "apple") {
		binaryName = "KitonyTerms.app/Contents/MacOS/kitonyterms"
	} else if strings.Contains(next.Target, "linux") {
		binaryName = strings.TrimSuffix(pkg.Name, ".tar.gz") + "/kitonyterms"
	}
	if err = validateBinary(destination, binaryName, next.Target); err != nil {
		return prepared, err
	}
	if strings.Contains(next.Target, "apple") {
		if err = verifyBundle(ctx, filepath.Join(destination.Name(), "KitonyTerms.app")); err != nil {
			return prepared, err
		}
	}
	binaryPath := filepath.Join(destination.Name(), filepath.FromSlash(binaryName))
	if err = verifyBuildCommand(ctx, binaryPath, next); err != nil {
		return prepared, err
	}
	executable, err := destination.Open(binaryName)
	if err != nil {
		return prepared, err
	}
	hash := sha256.New()
	_, err = io.Copy(hash, contextReader{ctx, executable})
	err = errors.Join(err, executable.Close())
	if err != nil {
		return prepared, err
	}
	err = file.Close()
	file = nil
	if err != nil {
		return prepared, err
	}
	if err = ctx.Err(); err != nil {
		return prepared, err
	}
	payload := binaryName
	if strings.Contains(next.Target, "apple") {
		payload = "KitonyTerms.app"
	}
	return preparedPayload{Directory: name, Payload: path.Join(name, payload), Executable: path.Join(name, binaryName), SHA256: hex.EncodeToString(hash.Sum(nil))}, nil
}

func validateBinary(root *os.Root, name, target string) error {
	info, err := root.Lstat(name)
	if err != nil {
		return err
	}
	if !info.Mode().IsRegular() {
		return fmt.Errorf("更新程序必须是普通文件")
	}
	file, err := root.Open(name)
	if err != nil {
		return err
	}
	defer file.Close()
	opened, err := file.Stat()
	if err != nil {
		return err
	}
	if !os.SameFile(info, opened) {
		return fmt.Errorf("更新程序在打开时改变")
	}
	var header [64]byte
	if _, err = io.ReadFull(file, header[:]); err != nil {
		return err
	}
	valid := false
	switch target {
	case "aarch64-apple-darwin", "x86_64-apple-darwin":
		machine := uint32(0x0100000c)
		if strings.HasPrefix(target, "x86_64") {
			machine = 0x01000007
		}
		valid = binary.LittleEndian.Uint32(header[:4]) == 0xfeedfacf && binary.LittleEndian.Uint32(header[4:8]) == machine && binary.LittleEndian.Uint32(header[12:16]) == 2
	case "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu":
		machine := uint16(62)
		if strings.HasPrefix(target, "aarch64") {
			machine = 183
		}
		kind := binary.LittleEndian.Uint16(header[16:18])
		valid = string(header[:4]) == "\x7fELF" && header[4] == 2 && header[5] == 1 && header[6] == 1 && (kind == 2 || kind == 3) && binary.LittleEndian.Uint16(header[18:20]) == machine
	case "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc":
		offset := int64(binary.LittleEndian.Uint32(header[60:64]))
		if offset < 64 || offset > 1<<20 || offset+26 > opened.Size() {
			return fmt.Errorf("PE 头位置无效")
		}
		var pe [26]byte
		if _, err = file.ReadAt(pe[:], offset); err != nil {
			return err
		}
		characteristics := binary.LittleEndian.Uint16(pe[22:24])
		machine := uint16(0x8664)
		if strings.HasPrefix(target, "aarch64") {
			machine = 0xaa64
		}
		valid = string(header[:2]) == "MZ" && string(pe[:4]) == "PE\x00\x00" && binary.LittleEndian.Uint16(pe[4:6]) == machine && characteristics&2 != 0 && characteristics&0x2000 == 0 && binary.LittleEndian.Uint16(pe[20:22]) >= 112 && binary.LittleEndian.Uint16(pe[24:26]) == 0x20b
	}
	if !valid {
		return fmt.Errorf("更新程序平台、架构或可执行类型不匹配")
	}
	return nil
}
