package qrcode

import (
	"bytes"
	"encoding/base64"
	"image"
	"image/color"
	"image/png"
	"testing"
)

func TestPairingPayloadRoundtrip(t *testing.T) {
	payload := "kitonyterms://lan-share?v=2&url=http://192.168.1.2:12345/v2/config&code=0123456789ABCDEFGHJKMNPQRS"
	encoded, err := Encode(payload)
	if err != nil {
		t.Fatal(err)
	}
	data, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		t.Fatal(err)
	}
	img, err := png.Decode(bytes.NewReader(data))
	if err != nil {
		t.Fatal(err)
	}
	size := img.Bounds()
	gray := image.NewGray(size)
	for y := size.Min.Y; y < size.Max.Y; y++ {
		for x := size.Min.X; x < size.Max.X; x++ {
			gray.SetGray(x, y, color.GrayModel.Convert(img.At(x, y)).(color.Gray))
		}
	}
	decoded, err := Decode(Frame{Width: size.Dx(), Height: size.Dy(), Data: base64.StdEncoding.EncodeToString(gray.Pix)})
	if err != nil || decoded != payload {
		t.Fatal("二维码往返不一致", err)
	}
	if _, err = Decode(Frame{Width: 480, Height: 480, Data: "AA=="}); err == nil {
		t.Fatal("非法帧被接受")
	}
	blank := bytes.Repeat([]byte{255}, 32*32)
	decoded, err = Decode(Frame{Width: 32, Height: 32, Data: base64.StdEncoding.EncodeToString(blank)})
	if err != nil || decoded != "" {
		t.Fatal("空白帧没有返回正常未识别", err)
	}
}
