package session

import (
	"context"
	"encoding/base64"
	"fmt"
	"strconv"
	"sync"

	"kitonyterms/internal/model"
)

const outputBudget = 1 << 20

type outputChunk struct {
	sequence uint64
	data     []byte
}
type outputBuffer struct {
	mu      sync.Mutex
	chunks  []outputChunk
	bytes   int
	next    uint64
	ack     uint64
	wake    chan struct{}
	closed  bool
	failure string
}

func newOutput() *outputBuffer        { return &outputBuffer{wake: make(chan struct{})} }
func (b *outputBuffer) signalLocked() { close(b.wake); b.wake = make(chan struct{}) }
func (b *outputBuffer) append(ctx context.Context, data []byte) error {
	if len(data) > 32768 {
		return fmt.Errorf("终端分块过大")
	}
	if len(data) == 0 {
		return nil
	}
	for {
		if err := ctx.Err(); err != nil {
			return err
		}
		b.mu.Lock()
		if b.closed {
			b.mu.Unlock()
			return fmt.Errorf("终端已关闭")
		}
		// 已确认字节作为重建历史保留；仅在预算不足时回收，未确认字节施加背压。
		for b.bytes+len(data) > outputBudget && len(b.chunks) > 0 && b.chunks[0].sequence <= b.ack {
			b.bytes -= len(b.chunks[0].data)
			b.chunks[0].data = nil
			b.chunks = b.chunks[1:]
		}
		if b.bytes+len(data) <= outputBudget {
			b.next++
			b.chunks = append(b.chunks, outputChunk{b.next, append([]byte(nil), data...)})
			b.bytes += len(data)
			b.signalLocked()
			b.mu.Unlock()
			return nil
		}
		wake := b.wake
		b.mu.Unlock()
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-wake:
		}
	}
}
func (b *outputBuffer) poll(after string) (model.TerminalBatch, error) {
	cursor, err := strconv.ParseUint(after, 10, 64)
	if err != nil {
		return model.TerminalBatch{}, fmt.Errorf("终端序号无效")
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	if cursor > b.next {
		return model.TerminalBatch{}, fmt.Errorf("终端消费序号超过输出序号")
	}
	batch := model.TerminalBatch{Chunks: []model.TerminalChunk{}, NextSequence: after, Closed: b.closed, Error: b.failure}
	if len(b.chunks) > 0 && cursor < b.chunks[0].sequence-1 {
		batch.Reset = true
		cursor = b.chunks[0].sequence - 1
	}
	if cursor > b.ack {
		b.ack = cursor
		b.signalLocked()
	}
	total := 0
	for _, chunk := range b.chunks {
		if chunk.sequence <= cursor {
			continue
		}
		if total >= 128<<10 {
			break
		}
		seq := strconv.FormatUint(chunk.sequence, 10)
		batch.Chunks = append(batch.Chunks, model.TerminalChunk{Sequence: seq, Data: base64.StdEncoding.EncodeToString(chunk.data)})
		batch.NextSequence = seq
		total += len(chunk.data)
	}
	return batch, nil
}
func (b *outputBuffer) close(failure string) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.closed = true
	b.failure = failure
	b.signalLocked()
}
