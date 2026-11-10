package syncconfig

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"time"

	"kitonyterms/internal/model"
)

const MaxEnvelopeBytes = 1 << 20

type Envelope struct {
	Format      string       `json:"format"`
	Version     uint16       `json:"version"`
	CreatedUnix uint64       `json:"created_unix"`
	Config      model.Config `json:"config"`
}

func NewEnvelope(config model.Config) Envelope {
	return Envelope{Format: "kitonyterms.config", Version: 1, CreatedUnix: uint64(time.Now().Unix()), Config: config}
}
func (e Envelope) Encode() ([]byte, error) {
	if err := e.validate(); err != nil {
		return nil, err
	}
	data, err := json.Marshal(e)
	if err != nil {
		return nil, err
	}
	if len(data) > MaxEnvelopeBytes {
		return nil, fmt.Errorf("同步配置超过 1 MiB")
	}
	return data, nil
}
func Decode(data []byte) (Envelope, error) {
	if len(data) > MaxEnvelopeBytes {
		return Envelope{}, fmt.Errorf("同步配置超过 1 MiB")
	}
	e := Envelope{Config: model.DefaultConfig()}
	d := json.NewDecoder(bytes.NewReader(data))
	d.DisallowUnknownFields()
	if err := d.Decode(&e); err != nil {
		return Envelope{}, fmt.Errorf("同步配置字段或格式无效")
	}
	if err := d.Decode(new(any)); err != io.EOF {
		return Envelope{}, fmt.Errorf("同步配置有多余内容")
	}
	if err := e.validate(); err != nil {
		return Envelope{}, err
	}
	return e, nil
}
func (e *Envelope) validate() error {
	if e.Format != "kitonyterms.config" || e.Version != 1 {
		return fmt.Errorf("不支持的同步格式或版本")
	}
	return e.Config.Normalize()
}
