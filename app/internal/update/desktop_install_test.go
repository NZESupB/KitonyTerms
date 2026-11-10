package update

import (
	"context"
	"encoding/json"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"kitonyterms/internal/model"
)

// 仅测试二进制读取邻接身份，用同一测试宿主构造新旧副本；不属于产品 CLI。
func testFixtureBuildInfo() (BuildInfo, error) {
	self, err := os.Executable()
	if err != nil {
		return BuildInfo{}, err
	}
	data, err := os.ReadFile(filepath.Join(filepath.Dir(self), ".test-build-info.json"))
	if errors.Is(err, fs.ErrNotExist) {
		return archiveTestBuild(), nil
	}
	var info BuildInfo
	if err != nil {
		return info, err
	}
	if err = decodeJSON(data, &info, true); err != nil {
		return info, err
	}
	return info, info.Validate()
}

func preparedDesktopFixture(t *testing.T) (*Desktop, installFixture) {
	t.Helper()
	fixture := helperInstallFixture(t)
	if err := fixture.installer.installHelper(context.Background(), fixture.task); err != nil {
		t.Fatal(err)
	}
	process, err := spawnApplyHelper(context.Background(), fixture.root, fixture.task.SourceSHA256)
	if err != nil {
		t.Fatal(err)
	}
	root, err := os.OpenRoot(fixture.root.Name())
	if err != nil {
		t.Fatal(err)
	}
	_, cancel := context.WithCancel(context.Background())
	p := &desktopPreparation{id: "prepared-owner", cancel: cancel, task: &fixture.task, process: process, ready: true}
	d := &Desktop{root: root, engine: fixture.installer, current: fixture.task.Current, install: fixture.task.Installation, preparation: p}
	t.Cleanup(func() {
		// 已提交的夹具不能等待测试宿主退出执行安装；只在此隔离夹具中终止。
		if p.committed {
			_ = p.process.command.Process.Kill()
			<-p.process.done
		}
		if err := d.Close(); err != nil {
			t.Error(err)
		}
	})
	return d, fixture
}

func TestDesktopPreparedAbortOwnsTaskAndReapsHelper(t *testing.T) {
	d, fixture := preparedDesktopFixture(t)
	p := d.preparation
	if err := d.abortInstall("stale"); err == nil {
		t.Fatal("旧请求取消了新准备")
	}
	if err := d.commitInstall(context.Background(), "stale"); err == nil {
		t.Fatal("旧请求提交了新准备")
	}
	if d.preparation != p {
		t.Fatal("旧请求改变了准备所有权")
	}
	if err := d.abortInstall(p.id); err != nil {
		t.Fatal(err)
	}
	select {
	case <-p.process.done:
	default:
		t.Fatal("取消返回时辅助进程仍未回收")
	}
	if d.preparation != nil {
		t.Fatal("取消后仍保留准备所有权")
	}
	if err := pendingInstallation(fixture.root); err != nil {
		t.Fatal(err)
	}
	if err := requireCleanHelperMarkers(fixture.root); err != nil {
		t.Fatal(err)
	}
	if _, err := acquireInstallLock(context.Background(), fixture.root, "installation.lock", 0); err == nil {
		t.Fatal("取消准备提前释放了安装锁")
	}
	assertInstallOriginal(t, fixture)
}

func TestDesktopPreparedCommitRechecksHandshakeAndParentExit(t *testing.T) {
	d, fixture := preparedDesktopFixture(t)
	p := d.preparation
	if err := fixture.root.Remove("helper-ready.json"); err != nil {
		t.Fatal(err)
	}
	if err := d.commitInstall(context.Background(), p.id); err == nil {
		t.Fatal("握手被删除后仍提交")
	}
	if err := writeInstallRecord(fixture.root, "helper-ready.json", []any{uint32(p.process.command.Process.Pid), fixture.task.SourceSHA256}); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err := d.commitInstall(ctx, p.id); !errors.Is(err, context.Canceled) {
		t.Fatal("取消的提交被接受", err)
	}
	if err := d.commitInstall(context.Background(), p.id); err != nil {
		t.Fatal(err)
	}
	if err := d.commitInstall(context.Background(), p.id); err != nil {
		t.Fatal("重复提交不幂等", err)
	}
	if err := d.abortInstall(p.id); err == nil {
		t.Fatal("提交后仍允许撤销")
	}
	if err := restartCommitted(fixture.root); err != nil {
		t.Fatal(err)
	}
	if err := d.Close(); err != nil {
		t.Fatal(err)
	}
	select {
	case <-p.process.done:
		t.Fatal("提交后的 helper 未继续等待真实父进程退出", p.process.err)
	default:
	}
	assertInstallOriginal(t, fixture)
}

func TestDesktopPreparedCommitRejectsExitedHelper(t *testing.T) {
	d, fixture := preparedDesktopFixture(t)
	p := d.preparation
	if err := p.process.command.Process.Kill(); err != nil {
		t.Fatal(err)
	}
	<-p.process.done
	if err := d.commitInstall(context.Background(), p.id); err == nil {
		t.Fatal("辅助进程已退出却提交安装")
	}
	if err := d.abortInstall(p.id); err != nil {
		t.Fatal("崩溃辅助进程的准备未回收", err)
	}
	assertInstallOriginal(t, fixture)
}

func TestPreparationCleanupPreservesForeignTaskAndJournal(t *testing.T) {
	fixture := helperInstallFixture(t)
	foreign := fixture.task
	foreign.Next.Build++
	if err := writeInstallRecord(fixture.root, "task.json", foreign); err != nil {
		t.Fatal(err)
	}
	before, err := fixture.root.ReadFile("task.json")
	if err != nil {
		t.Fatal(err)
	}
	if err = cleanupPreparationRecords(fixture.root, fixture.task, 0); err == nil {
		t.Fatal("取消清除了外来任务")
	}
	after, err := fixture.root.ReadFile("task.json")
	if err != nil || string(before) != string(after) {
		t.Fatal("外来任务原文改变", err)
	}
	if err = writeInstallRecord(fixture.root, "task.json", fixture.task); err != nil {
		t.Fatal(err)
	}
	if err = fixture.root.WriteFile("journal.json", []byte("保留未知事务材料"), 0600); err != nil {
		t.Fatal(err)
	}
	if err = cleanupPreparationRecords(fixture.root, fixture.task, 0); err == nil {
		t.Fatal("意外 journal 未阻止取消清理")
	}
	data, err := fixture.root.ReadFile("journal.json")
	if err != nil || string(data) != "保留未知事务材料" {
		t.Fatal("取消破坏了事务材料", err)
	}
}

func TestServicePreparationCancellationAndPreferenceOwnership(t *testing.T) {
	d := desktopDiscoveryFixture(t, archiveTestBuild())
	s, err := openService(context.Background(), d.root.Name(), d.current, d, newClient)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := s.Close(); err != nil {
			t.Error(err)
		}
	})
	s.mu.Lock()
	s.available = &available{Info: archiveTestBuild()}
	s.state.Phase = "downloaded"
	s.mu.Unlock()
	d.installOp.Lock()
	locked := true
	defer func() {
		if locked {
			d.installOp.Unlock()
		}
	}()
	prepared := make(chan error, 1)
	go func() { prepared <- s.PrepareInstall(context.Background(), "service-owner") }()
	deadline := time.Now().Add(time.Second)
	for {
		d.mu.Lock()
		pending := d.preparation != nil
		d.mu.Unlock()
		if pending {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("准备没有取得桌面所有权")
		}
		time.Sleep(time.Millisecond)
	}
	state := s.Snapshot()
	if !state.Busy || state.InstallAvailable || state.Phase != "install_preparing" {
		t.Fatal("准备状态未隔离", state)
	}
	if err = s.Check(); err == nil {
		t.Fatal("检查覆盖了安装准备")
	}
	if err = s.Download(); err == nil {
		t.Fatal("下载覆盖了安装准备")
	}
	if err = s.Cancel(); err == nil {
		t.Fatal("普通取消绕过了安装所有权")
	}
	if _, err = s.SavePreferences(context.Background(), model.SaveUpdatePreferencesRequest{ExpectedRevision: state.PreferenceRevision, Preferences: state.Preferences}); err == nil {
		t.Fatal("偏好保存覆盖了安装准备")
	}
	if err = s.AbortInstall("stale"); err == nil {
		t.Fatal("旧取消影响新准备")
	}
	aborted := make(chan error, 1)
	go func() { aborted <- s.AbortInstall("service-owner") }()
	select {
	case <-aborted:
		t.Fatal("准备尚未结束就返回取消")
	default:
	}
	d.installOp.Unlock()
	locked = false
	if err = <-prepared; err == nil {
		t.Fatal("取消后准备成功")
	}
	if err = <-aborted; err != nil {
		t.Fatal(err)
	}
	state = s.Snapshot()
	if state.Busy || state.Phase != "downloaded" || !state.InstallAvailable {
		t.Fatal("取消后没有恢复更新服务", state)
	}
}

func TestPersistedInstallFailureDoesNotExposeContentsOrHideProxyNotice(t *testing.T) {
	for _, record := range []string{`"私有路径与旧版错误内容"`, `{}`} {
		t.Run(record, func(t *testing.T) {
			root := archiveTestRoot(t)
			if err := root.WriteFile("error.json", []byte(record), 0600); err != nil {
				t.Fatal(err)
			}
			if err := root.WriteFile("preferences.json", []byte(`{"auto_check":false,"auto_download":false,"proxy":{"kind":"bad"}}`), 0600); err != nil {
				t.Fatal(err)
			}
			s, err := openService(context.Background(), root.Name(), testBuild(), nil, newClient)
			if err != nil {
				t.Fatal(err)
			}
			defer s.Close()
			state := s.Snapshot()
			if state.Notice != "invalid_proxy" || state.Failure == nil || state.Failure.Stage != "install" {
				t.Fatal("历史安装失败或代理提示缺失", state)
			}
			encoded, err := json.Marshal(state)
			if err != nil || strings.Contains(string(encoded), "私有路径") {
				t.Fatal("原始安装错误进入快照", err)
			}
			want := "install_failed"
			if record == `{}` {
				want = "install_record"
			}
			if state.Failure.Kind != want {
				t.Fatal("失败类别错误", state.Failure)
			}
		})
	}
}

func TestDesktopConcurrentCloseReapsUncommittedHelper(t *testing.T) {
	d, fixture := preparedDesktopFixture(t)
	p := d.preparation
	results := make(chan error, 16)
	for range 16 {
		go func() { results <- d.Close() }()
	}
	for range 16 {
		if err := <-results; err != nil {
			t.Fatal(err)
		}
	}
	select {
	case <-p.process.done:
	default:
		t.Fatal("并发关闭返回时 helper 仍未回收")
	}
	gate, err := acquireInstallLock(context.Background(), fixture.root, "installation.lock", 0)
	if err != nil {
		t.Fatal("关闭返回后安装锁未释放", err)
	}
	if err = gate.Close(); err != nil {
		t.Fatal(err)
	}
	if err = pendingInstallation(fixture.root); err != nil {
		t.Fatal(err)
	}
	assertInstallOriginal(t, fixture)
}
