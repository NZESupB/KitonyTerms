package qrcode

import (
	"bytes"
	"encoding/base64"
	"errors"
	"fmt"
	"image"
	"image/png"

	"github.com/makiuchi-d/gozxing"
	"github.com/makiuchi-d/gozxing/qrcode"
)

type Frame struct {
	Width  int    `json:"width"`
	Height int    `json:"height"`
	Data   string `json:"data"`
}

func Encode(text string) (string, error) {
	if len(text) == 0 || len(text) > 4096 {
		return "", fmt.Errorf("二维码内容长度无效")
	}
	matrix, err := qrcode.NewQRCodeWriter().EncodeWithoutHint(text, gozxing.BarcodeFormat_QR_CODE, 320, 320)
	if err != nil {
		return "", err
	}
	var data bytes.Buffer
	if err = png.Encode(&data, matrix); err != nil {
		return "", err
	}
	return base64.StdEncoding.EncodeToString(data.Bytes()), nil
}
func Decode(frame Frame) (string, error) {
	if frame.Width < 16 || frame.Height < 16 || frame.Width > 480 || frame.Height > 480 || len(frame.Data) > base64.StdEncoding.EncodedLen(480*480) {
		return "", fmt.Errorf("扫码帧尺寸或编码长度无效")
	}
	pixels, err := base64.StdEncoding.DecodeString(frame.Data)
	if err != nil || len(pixels) != frame.Width*frame.Height {
		return "", fmt.Errorf("扫码帧像素长度无效")
	}
	gray := &image.Gray{Pix: pixels, Stride: frame.Width, Rect: image.Rect(0, 0, frame.Width, frame.Height)}
	bitmap, err := gozxing.NewBinaryBitmapFromImage(gray)
	if err != nil {
		return "", err
	}
	result, err := qrcode.NewQRCodeReader().Decode(bitmap, nil)
	if err != nil {
		var scanError gozxing.ReaderException
		if errors.As(err, &scanError) {
			return "", nil
		}
		return "", err
	}
	if len(result.GetText()) > 4096 {
		return "", fmt.Errorf("二维码内容过长")
	}
	return result.GetText(), nil
}
