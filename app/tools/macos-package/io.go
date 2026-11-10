package main

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"
	"sync"
	"time"
)

func readBounded(path string, limit int64) ([]byte, error) {
	info, err := os.Lstat(path)
	if err != nil || !info.Mode().IsRegular() || info.Size() < 0 || info.Size() > limit {
		return nil, errors.Join(errors.New("输入必须是有界普通文件"), err)
	}
	file, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	opened, statErr := file.Stat()
	if statErr != nil || !os.SameFile(info, opened) {
		return nil, errors.Join(errors.New("打开期间输入身份变化"), statErr, file.Close())
	}
	data, readErr := io.ReadAll(io.LimitReader(file, limit+1))
	after, statErr := os.Lstat(path)
	err = errors.Join(readErr, statErr, file.Close())
	if err == nil && (!os.SameFile(info, after) || info.Mode() != after.Mode() || !info.ModTime().Equal(after.ModTime()) || after.Size() != info.Size() || int64(len(data)) != info.Size()) {
		err = errors.New("读取期间输入内容或身份变化")
	}
	return data, err
}

func writeFile(path string, data []byte, mode os.FileMode) error {
	file, err := os.OpenFile(path, os.O_CREATE|os.O_EXCL|os.O_WRONLY, mode)
	if err != nil {
		return err
	}
	_, writeErr := file.Write(data)
	if writeErr == nil {
		writeErr = file.Sync()
	}
	return errors.Join(writeErr, file.Close())
}

func digestFile(path string) (string, error) {
	data, err := readBounded(path, 128<<20)
	if err != nil {
		return "", err
	}
	hash := sha256.Sum256(data)
	return hex.EncodeToString(hash[:]), nil
}

type boundedOutput struct {
	mu       sync.Mutex
	buffer   bytes.Buffer
	limit    int
	cancel   context.CancelFunc
	overflow bool
}

func (o *boundedOutput) Write(data []byte) (int, error) {
	o.mu.Lock()
	defer o.mu.Unlock()
	if len(data) > o.limit-o.buffer.Len() {
		o.overflow = true
		o.cancel()
		return 0, errors.New("候选工具输出超过上限")
	}
	return o.buffer.Write(data)
}

func environment(fixed map[string]string) []string {
	var result []string
	for _, item := range os.Environ() {
		key, _, _ := strings.Cut(item, "=")
		if _, replaced := fixed[strings.ToUpper(key)]; !replaced {
			result = append(result, item)
		}
	}
	for key, value := range fixed {
		result = append(result, key+"="+value)
	}
	return result
}

// 每次等待真实子进程结束；原生平台在取消时同时停止本次工具的子进程组。
func runTool(ctx context.Context, budget time.Duration, limit int, directory string, env []string, executable string, arguments ...string) ([]byte, []byte, error) {
	deadline, stop := context.WithTimeout(ctx, budget)
	defer stop()
	call, cancel := context.WithCancel(deadline)
	defer cancel()
	stdout := &boundedOutput{limit: limit, cancel: cancel}
	stderr := &boundedOutput{limit: 256 << 10, cancel: cancel}
	command := exec.CommandContext(call, executable, arguments...)
	command.Dir, command.Env, command.WaitDelay = directory, env, time.Second
	command.Stdout, command.Stderr = stdout, stderr
	configureCommand(command)
	err := command.Run()
	if stdout.overflow || stderr.overflow {
		err = errors.Join(err, errors.New("候选工具输出超过上限"))
	}
	if deadline.Err() != nil {
		err = errors.Join(err, deadline.Err())
	}
	if err != nil {
		err = fmt.Errorf("候选工具执行失败：%w", err)
	}
	return append([]byte(nil), stdout.buffer.Bytes()...), append([]byte(nil), stderr.buffer.Bytes()...), err
}
