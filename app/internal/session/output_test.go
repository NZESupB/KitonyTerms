package session

import (
	"bytes"
	"context"
	"encoding/base64"
	"strconv"
	"testing"
	"time"
)

func TestOutputReplayAndAcknowledgement(t *testing.T) {
	b := newOutput()
	if err := b.append(context.Background(), []byte("\x1b[31m中文\x1b[0m")); err != nil {
		t.Fatal(err)
	}
	first, err := b.poll("0")
	if err != nil {
		t.Fatal(err)
	}
	replay, err := b.poll("0")
	if err != nil {
		t.Fatal(err)
	}
	if len(first.Chunks) != 1 || first.Chunks[0] != replay.Chunks[0] {
		t.Fatal("未确认字节无法重读")
	}
	ack, err := b.poll(first.NextSequence)
	if err != nil || len(ack.Chunks) != 0 {
		t.Fatal("确认后仍重复投递", err)
	}
	rebuilt, err := b.poll("0")
	if err != nil || len(rebuilt.Chunks) != 1 {
		t.Fatal("重建时历史丢失", err)
	}
	if _, err = b.poll("2"); err == nil {
		t.Fatal("接受了未来序号")
	}
}
func TestOutputBackpressureCancellationAndBoundedHistory(t *testing.T) {
	b := newOutput()
	chunk := bytes.Repeat([]byte("x"), 32768)
	for i := 0; i < outputBudget/len(chunk); i++ {
		if err := b.append(context.Background(), chunk); err != nil {
			t.Fatal(err)
		}
	}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- b.append(ctx, []byte("tail")) }()
	select {
	case <-done:
		t.Fatal("未确认数据超过预算")
	case <-time.After(20 * time.Millisecond):
	}
	cancel()
	select {
	case err := <-done:
		if err == nil {
			t.Fatal("取消未传播")
		}
	case <-time.After(time.Second):
		t.Fatal("背压取消挂起")
	}
	if _, err := b.poll("32"); err != nil {
		t.Fatal(err)
	}
	if err := b.append(context.Background(), []byte("tail")); err != nil {
		t.Fatal(err)
	}
	replay, err := b.poll("0")
	if err != nil || !replay.Reset {
		t.Fatal("历史截断未通知", err)
	}
	b.mu.Lock()
	size := b.bytes
	b.mu.Unlock()
	if size > outputBudget {
		t.Fatal("缓冲区超预算")
	}
	b.close("finished")
	last, err := b.poll(strconv.FormatUint(32, 10))
	if err != nil {
		t.Fatal(err)
	}
	if !last.Closed || len(last.Chunks) != 1 {
		t.Fatal("关闭丢失尾块")
	}
	data, err := base64.StdEncoding.DecodeString(last.Chunks[0].Data)
	if err != nil || string(data) != "tail" {
		t.Fatal("尾块错误")
	}
}
