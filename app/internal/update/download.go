package update

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"io"
	"io/fs"
	"os"
	"strings"
	"time"
)

type contextReader struct {
	ctx context.Context
	io.Reader
}

func (r contextReader) Read(buffer []byte) (int, error) {
	if err := r.ctx.Err(); err != nil {
		return 0, err
	}
	return r.Reader.Read(buffer)
}
func verifyCache(ctx context.Context, root *os.Root, name string, pkg Package) error {
	info, err := root.Lstat(name)
	if err != nil {
		return err
	}
	if !info.Mode().IsRegular() {
		return &failure{Kind: "storage", Stage: "cache"}
	}
	if info.Size() != int64(pkg.Size) {
		return &failure{Kind: "size", Stage: "cache"}
	}
	f, err := root.Open(name)
	if err != nil {
		return &failure{Kind: "storage", Stage: "cache"}
	}
	defer f.Close()
	opened, err := f.Stat()
	if err != nil || !os.SameFile(info, opened) || !opened.Mode().IsRegular() {
		return &failure{Kind: "storage", Stage: "cache"}
	}
	hash := sha256.New()
	n, err := io.CopyBuffer(hash, io.LimitReader(contextReader{ctx, f}, int64(pkg.Size)+1), make([]byte, 64<<10))
	if err != nil {
		return err
	}
	if n != int64(pkg.Size) {
		return &failure{Kind: "size", Stage: "cache"}
	}
	if hex.EncodeToString(hash.Sum(nil)) != pkg.SHA256 {
		return &failure{Kind: "digest", Stage: "cache"}
	}
	return ctx.Err()
}
func (c *releaseClient) download(ctx context.Context, root *os.Root, a available, progress func(uint64, uint64)) (err error) {
	if err = a.Package.Validate(); err != nil {
		return &failure{Kind: "metadata", Stage: "download"}
	}
	pkg := a.Package
	pkg.SHA256 = strings.ToLower(pkg.SHA256)
	cached := "package-" + pkg.SHA256
	if _, statErr := root.Lstat(cached); statErr == nil {
		if err = verifyCache(ctx, root, cached, pkg); err == nil {
			progress(pkg.Size, 0)
			return pruneCache(root, cached)
		}
		var invalid *failure
		if !errors.As(err, &invalid) || (invalid.Kind != "digest" && invalid.Kind != "size") {
			return err
		}
		if err = root.Remove(cached); err != nil {
			return &failure{Kind: "storage", Stage: "cache"}
		}
	} else if !errors.Is(statErr, fs.ErrNotExist) {
		return &failure{Kind: "storage", Stage: "cache"}
	}
	f, temp, err := privateTemp(root)
	if err != nil {
		return &failure{Kind: "storage", Stage: "download"}
	}
	defer func() {
		f.Close()
		if cleanup := root.Remove(temp); cleanup != nil && !errors.Is(cleanup, fs.ErrNotExist) {
			err = &failure{Kind: "storage", Stage: "download"}
		}
	}()
	response, err := c.asset(ctx, a.AssetID, "download")
	if err != nil {
		return err
	}
	defer response.Body.Close()
	if response.ContentLength >= 0 && uint64(response.ContentLength) != pkg.Size {
		return &failure{Kind: "size", Stage: "download"}
	}
	var received, previous uint64
	var speed float64
	hash := sha256.New()
	buffer := make([]byte, 64<<10)
	sample := time.Now()
	for {
		if err = ctx.Err(); err != nil {
			return err
		}
		n, readErr := response.Body.Read(buffer)
		if n > 0 {
			if uint64(n) > pkg.Size-received {
				return &failure{Kind: "size", Stage: "download"}
			}
			received += uint64(n)
			if _, err = f.Write(buffer[:n]); err != nil {
				return &failure{Kind: "storage", Stage: "download"}
			}
			hash.Write(buffer[:n])
			now := time.Now()
			elapsed := now.Sub(sample)
			if elapsed >= 100*time.Millisecond {
				rate := float64(received-previous) / elapsed.Seconds()
				if speed == 0 {
					speed = rate
				} else {
					speed = speed*.75 + rate*.25
				}
				progress(received, uint64(speed))
				previous = received
				sample = now
			}
		}
		if readErr == io.EOF {
			break
		}
		if readErr != nil {
			return failureOf(readErr, "download")
		}
	}
	if err = ctx.Err(); err != nil {
		return err
	}
	if received != pkg.Size {
		return &failure{Kind: "size", Stage: "download"}
	}
	if hex.EncodeToString(hash.Sum(nil)) != pkg.SHA256 {
		return &failure{Kind: "digest", Stage: "download"}
	}
	if err = f.Sync(); err != nil {
		return &failure{Kind: "storage", Stage: "download"}
	}
	if err = f.Close(); err != nil {
		return &failure{Kind: "storage", Stage: "download"}
	}
	if err = ctx.Err(); err != nil {
		return err
	}
	if err = root.Rename(temp, cached); err != nil {
		return &failure{Kind: "storage", Stage: "cache"}
	}
	progress(received, 0)
	return pruneCache(root, cached)
}
func pruneCache(root *os.Root, keep string) error {
	// 已准备但尚未写 journal 的旧事务同样可能引用缓存。
	for _, record := range []string{"task.json", "journal.json"} {
		if _, err := root.Lstat(record); err == nil {
			return nil
		} else if !errors.Is(err, fs.ErrNotExist) {
			return &failure{Kind: "storage", Stage: "cache"}
		}
	}
	dir, err := root.Open(".")
	if err != nil {
		return &failure{Kind: "storage", Stage: "cache"}
	}
	defer dir.Close()
	for {
		entries, readErr := dir.ReadDir(128)
		for _, entry := range entries {
			name := entry.Name()
			if name == keep || !strings.HasPrefix(name, "package-") || !validHex(strings.TrimPrefix(name, "package-"), 64) || !entry.Type().IsRegular() {
				continue
			}
			info, err := root.Lstat(name)
			if err != nil {
				return &failure{Kind: "storage", Stage: "cache"}
			}
			if info.Mode().IsRegular() {
				if err = root.Remove(name); err != nil {
					return &failure{Kind: "storage", Stage: "cache"}
				}
			}
		}
		if readErr == io.EOF {
			return nil
		}
		if readErr != nil {
			return &failure{Kind: "storage", Stage: "cache"}
		}
	}
}
