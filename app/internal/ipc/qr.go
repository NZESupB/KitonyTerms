package ipc

import (
	"context"
	"fmt"
	"net/url"
	"strings"
	"time"

	"kitonyterms/internal/qrcode"
)

func (b *Backend) ShareQRCode() (string, error) {
	info, err := b.GetShare()
	if err != nil {
		return "", err
	}
	if info == nil {
		return "", fmt.Errorf("分享已结束")
	}
	payload := "kitonyterms://lan-share?v=2&url=" + url.QueryEscape(info.URL) + "&code=" + info.PairingCode
	return qrcode.Encode(payload)
}
func (b *Backend) DecodeQR(ctx context.Context, frame qrcode.Frame) (string, error) {
	if err := b.check(); err != nil {
		return "", err
	}
	if !b.scannerMu.TryLock() {
		return "", fmt.Errorf("上一帧仍在解码")
	}
	defer b.scannerMu.Unlock()
	if time.Since(b.lastScan) < 200*time.Millisecond {
		return "", fmt.Errorf("扫码请求过于频繁")
	}
	b.lastScan = time.Now()
	if err := ctx.Err(); err != nil {
		return "", err
	}
	text, err := qrcode.Decode(frame)
	if err != nil {
		return "", err
	}
	if err = ctx.Err(); err != nil {
		return "", err
	}
	if text != "" && !strings.HasPrefix(text, "kitonyterms://lan-share?") {
		return "", fmt.Errorf("请扫描 KitonyTerms 局域网分享二维码")
	}
	return text, nil
}
