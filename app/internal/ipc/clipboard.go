package ipc

import (
	"fmt"

	"github.com/wailsapp/wails/v3/pkg/application"
)

// 固定版本runtime未传递原生成功标志，因此在服务边界显式报告失败。
func (b *Backend) WriteClipboard(text string) error {
	if err := b.check(); err != nil {
		return err
	}
	if application.System.IsMobile() {
		return fmt.Errorf("此平台使用WebView剪贴板")
	}
	b.clipboardMu.Lock()
	defer b.clipboardMu.Unlock()
	if !b.app.Clipboard.SetText(text) {
		return fmt.Errorf("原生剪贴板写入失败")
	}
	return nil
}
func (b *Backend) ReadClipboard() (string, error) {
	if err := b.check(); err != nil {
		return "", err
	}
	if application.System.IsMobile() {
		return "", fmt.Errorf("此平台使用WebView剪贴板")
	}
	b.clipboardMu.Lock()
	defer b.clipboardMu.Unlock()
	text, ok := b.app.Clipboard.Text()
	if !ok {
		return "", fmt.Errorf("原生剪贴板读取失败")
	}
	return text, nil
}
