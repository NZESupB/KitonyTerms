package update

import (
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"

	"kitonyterms/internal/model"
)

func openRoot(path string) (*os.Root, error) {
	if err := os.MkdirAll(path, 0700); err != nil {
		return nil, err
	}
	info, err := os.Lstat(path)
	if err != nil {
		return nil, err
	}
	if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return nil, fmt.Errorf("更新目录不能是链接或普通文件")
	}
	canonical, err := canonicalUpdatePath(path)
	if err != nil {
		return nil, err
	}
	root, err := os.OpenRoot(canonical)
	if err != nil {
		return nil, err
	}
	if err = root.Chmod(".", 0700); err != nil {
		root.Close()
		return nil, err
	}
	return root, nil
}
func privateTemp(root *os.Root) (*os.File, string, error) {
	for range 4 {
		name := ".update-" + rand.Text()
		f, err := root.OpenFile(name, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
		if errors.Is(err, fs.ErrExist) {
			continue
		}
		return f, name, err
	}
	return nil, "", fmt.Errorf("无法创建唯一更新临时文件")
}
func atomicJSON(root *os.Root, name string, value any) (err error) {
	data, err := json.Marshal(value)
	if err != nil {
		return err
	}
	data = append(data, '\n')
	f, temp, err := privateTemp(root)
	if err != nil {
		return err
	}
	defer func() {
		f.Close()
		if cleanup := root.Remove(temp); cleanup != nil && !errors.Is(cleanup, fs.ErrNotExist) {
			err = errors.Join(err, cleanup)
		}
	}()
	if _, err = f.Write(data); err != nil {
		return err
	}
	if err = f.Sync(); err != nil {
		return err
	}
	if err = f.Close(); err != nil {
		return err
	}
	return root.Rename(temp, name)
}
func readPrivate(root *os.Root, name string, limit int) ([]byte, error) {
	info, err := root.Lstat(name)
	if err != nil {
		return nil, err
	}
	if !info.Mode().IsRegular() || info.Size() > int64(limit) {
		return nil, fmt.Errorf("更新文件类型或大小无效")
	}
	f, err := root.Open(name)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	opened, err := f.Stat()
	if err != nil {
		return nil, err
	}
	if !os.SameFile(info, opened) || !opened.Mode().IsRegular() {
		return nil, fmt.Errorf("更新文件在打开时已改变")
	}
	data, err := io.ReadAll(io.LimitReader(f, int64(limit)+1))
	if err != nil {
		return nil, err
	}
	if len(data) > limit {
		return nil, fmt.Errorf("更新文件过大")
	}
	return data, nil
}
func readPreferences(root *os.Root, current BuildInfo) (model.UpdatePreferences, string, error) {
	enabled := current.Channel != "development"
	preferences := model.UpdatePreferences{AutoCheck: enabled, AutoDownload: enabled, Proxy: model.UpdateProxy{Kind: "system"}}
	data, err := readPrivate(root, "preferences.json", 32<<10)
	if errors.Is(err, fs.ErrNotExist) {
		return preferences, "", nil
	}
	if err != nil {
		return preferences, "", err
	}
	var disk struct {
		AutoCheck    *bool           `json:"auto_check"`
		AutoDownload *bool           `json:"auto_download"`
		Proxy        json.RawMessage `json:"proxy"`
	}
	if err = decodeJSON(data, &disk, true); err != nil {
		return preferences, "", fmt.Errorf("更新偏好JSON无效：%w", err)
	}
	if disk.AutoCheck == nil || disk.AutoDownload == nil {
		return preferences, "", fmt.Errorf("更新偏好缺少检查或下载设置")
	}
	preferences.AutoCheck = *disk.AutoCheck
	preferences.AutoDownload = *disk.AutoDownload
	if len(disk.Proxy) > 0 {
		var proxy model.UpdateProxy
		if err = decodeJSON(disk.Proxy, &proxy, true); err != nil || validateProxy(proxy) != nil {
			preferences.Proxy = model.UpdateProxy{Kind: "system"}
			return preferences, "invalid_proxy", nil
		}
		preferences.Proxy = proxy
	}
	return preferences, "", nil
}
