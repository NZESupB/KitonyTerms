package session

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"kitonyterms/internal/externaledit"
)

func TestExternalEditBinaryConflictPermissionsAndSessionOwnership(t *testing.T) {
	server := startLoopback(t)
	target := filepath.Join(server.root, "a '中.bin")
	initial := bytes.Repeat([]byte{0, 255, 7, 8}, 300000)
	if err := os.WriteFile(target, initial, 0640); err != nil {
		t.Fatal(err)
	}
	initialInfo, err := os.Stat(target)
	if err != nil {
		t.Fatal(err)
	}
	remote := remoteTestPath(target)
	m, _ := newLoopbackManager(t, server, "tester")
	opened, err := m.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	s := waitConnected(t, m, opened, true)
	ctx, cancel := context.WithTimeout(t.Context(), 10*time.Second)
	defer cancel()
	data, revision, err := m.ReadEditFile(ctx, s.ID, s.Generation, remote)
	if err != nil || !bytes.Equal(data, initial) {
		t.Fatal("二进制读取失败", err)
	}
	if _, _, err = m.ReadEditFile(ctx, s.ID, "old", remote); err == nil {
		t.Fatal("旧代次读取被接受")
	}
	modified := append(bytes.Clone(data), 0, 254, 3)
	if _, err = m.WriteEditFile(ctx, s.ID, s.Generation, remote, modified, revision); err != nil {
		t.Fatal(err)
	}
	info, err := os.Stat(target)
	if err != nil || info.Mode().Perm() != initialInfo.Mode().Perm() {
		t.Fatal("权限未保留", err)
	}
	if _, err = m.WriteEditFile(ctx, s.ID, s.Generation, remote, []byte("stale"), revision); err == nil || !strings.Contains(err.Error(), "已更改") {
		t.Fatal("旧版本覆盖了远端", err)
	}
	entries, err := os.ReadDir(server.root)
	if err != nil {
		t.Fatal(err)
	}
	for _, entry := range entries {
		if strings.HasPrefix(entry.Name(), ".kitony-edit-") {
			t.Fatal("临时文件未清理")
		}
	}
	copied := make(chan string, 1)
	edits, err := externaledit.New(t.Context(), t.TempDir(), m, func(file, _ string) error { copied <- file; return nil })
	if err != nil {
		t.Fatal(err)
	}
	defer edits.Close()
	waitWatching := func(editID string) {
		t.Helper()
		end := time.Now().Add(2 * time.Second)
		for time.Now().Before(end) {
			for _, s := range edits.List() {
				if s.ID == editID && s.Status == "watching" {
					return
				}
			}
			time.Sleep(time.Millisecond)
		}
		t.Fatal("未开始监听", edits.List())
	}
	edit, err := edits.Start(s.ID, s.Generation, remote, "")
	if err != nil {
		t.Fatal(err)
	}
	var local string
	select {
	case local = <-copied:
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	waitWatching(edit.ID)
	if err = os.WriteFile(local, []byte("local saved 中文"), 0600); err != nil {
		t.Fatal(err)
	}
	if err = edits.Action(ctx, s.ID, s.Generation, edit.ID, "once"); err != nil {
		t.Fatal(err)
	}
	end := time.Now().Add(3 * time.Second)
	for len(edits.List()) > 0 && time.Now().Before(end) {
		time.Sleep(time.Millisecond)
	}
	if len(edits.List()) > 0 {
		t.Fatal(edits.List())
	}
	actual, err := os.ReadFile(target)
	if err != nil || string(actual) != "local saved 中文" {
		t.Fatal("实际SSH未回传", err)
	}
	edit, err = edits.Start(s.ID, s.Generation, remote, "")
	if err != nil {
		t.Fatal(err)
	}
	select {
	case local = <-copied:
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
	waitWatching(edit.ID)
	if err = os.WriteFile(local, []byte("keep after disconnect"), 0600); err != nil {
		t.Fatal(err)
	}
	if err = m.CloseSession(s.ID, s.Generation); err != nil {
		t.Fatal(err)
	}
	edits.StopSession(s.ID, s.Generation)
	states := edits.List()
	if len(states) != 1 || states[0].ID != edit.ID || states[0].Status != "retained" {
		t.Fatal(states)
	}
	if actual, err = os.ReadFile(local); err != nil || string(actual) != "keep after disconnect" {
		t.Fatal("断开后副本未保留", err)
	}
	if _, err = m.WriteEditFile(ctx, s.ID, s.Generation, target, []byte("closed"), ""); err == nil {
		t.Fatal("关闭连接后允许回传")
	}
}
