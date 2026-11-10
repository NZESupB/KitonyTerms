package shellintegration

import (
	"bytes"
	"context"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"testing"
	"time"
)

func prepared() *Observer {
	o := NewObserver()
	o.Output([]byte(DoneMarker+PromptMarker), true, time.Now())
	return o
}
func prompt(o *Observer) { o.Output([]byte(PromptMarker), true, time.Now()) }

func TestHistoryRequiresInitializedPromptAndOneSubmission(t *testing.T) {
	o := NewObserver()
	for _, value := range []string{"login-private\r", PromptMarker, "initial-private\r"} {
		if strings.HasPrefix(value, "\x1b") {
			o.Output([]byte(value), true, time.Now())
		} else if o.Input([]byte(value)) != "" {
			t.Fatal("初始化期间采集输入")
		}
	}
	for _, b := range []byte(DoneMarker + PromptMarker) {
		o.Output([]byte{b}, true, time.Now())
	}
	command := o.Input([]byte("read -s value\rprivate-second-line\r"))
	if command != "read -s value" {
		t.Fatal(command)
	}
	o.Commit(command, time.Now())
	if o.Input([]byte("password-private\r")) != "" {
		t.Fatal("采集了运行中程序输入")
	}
	o.Output([]byte("Password: "+PromptMarker), false, time.Now())
	if o.Input([]byte("stderr-private\r")) != "" {
		t.Fatal("stderr开放采集")
	}
	prompt(o)
	if command = o.Input([]byte("pwd\r")); command != "pwd" {
		t.Fatal(command)
	}
	o.Commit(command, time.Now())
	history := o.History()
	if len(history) != 2 || history[0].Command != "read -s value" || history[1].Command != "pwd" {
		t.Fatal(history)
	}
}

func TestHistoryPrivateEditsUnicodeAndLimits(t *testing.T) {
	for _, input := range []string{" hidden\r", "echo\tvalue\r", "pwd\x1b[A\r", "stop\x03\r", "pwd\x1b[D\x15other\r", strings.Repeat("x", 4097) + "\r"} {
		if prepared().Input([]byte(input)) != "" {
			t.Fatalf("不确定输入被采集: %q", input[:min(len(input), 30)])
		}
	}
	o := prepared()
	o.Input([]byte("echo 中文"))
	if got := o.Input([]byte("\x7f\r")); got != "echo 中" {
		t.Fatal(got)
	}
	o = prepared()
	if got := o.Input([]byte("discard\x15pwd\r")); got != "pwd" {
		t.Fatal(got)
	}
	if got := prepared().Input([]byte(strings.Repeat("x", 4096) + "\r")); len(got) != 4096 {
		t.Fatal("错误的字节上限")
	}
	o = prepared()
	o.Output([]byte("\x1b[?10"), true, time.Now())
	o.Output([]byte("49h"), true, time.Now())
	if !o.Snapshot(time.Now()).AltScreen || o.Input([]byte("vim-private\r")) != "" {
		t.Fatal("备用屏保护失败")
	}
	if err := o.Directory("/tmp", time.Now()); err == nil {
		t.Fatal("备用屏允许目录命令")
	}
	o.Output([]byte("\x1b[?1049l"), true, time.Now())
	if o.Input([]byte("still-private\r")) != "" {
		t.Fatal("退出备用屏后未等提示符")
	}
	prompt(o)
	if o.Input([]byte("pwd\r")) != "pwd" {
		t.Fatal("提示符未恢复采集")
	}
}

func TestCwdBoundedParsingAndPendingTarget(t *testing.T) {
	o := prepared()
	now := time.Now()
	for _, b := range []byte("\x1b]7;file://host/tmp/%E4%B8%AD%20%zz\x1b\\") {
		o.Output([]byte{b}, true, now)
	}
	if got := o.Snapshot(now); got.Cwd != "/tmp/中 %zz" || got.Revision != "1" {
		t.Fatal(got)
	}
	if err := o.Directory("/new", now); err != nil {
		t.Fatal(err)
	}
	o.Output([]byte("\x1b]7;file:///old\a"), true, now)
	if got := o.Snapshot(now); got.Cwd != "/tmp/中 %zz" || got.PendingDirectory != "/new" {
		t.Fatal(got)
	}
	o.Output([]byte("\x1b]7;file:///new\a"), true, now)
	if got := o.Snapshot(now); got.Cwd != "/new" || got.PendingDirectory != "" {
		t.Fatal(got)
	}
	if err := o.Directory("/missing", now); err != nil {
		t.Fatal(err)
	}
	o.Output([]byte("\x1b]7;file:///later\a"), true, now.Add(4*time.Second))
	if got := o.Snapshot(now.Add(4 * time.Second)); got.Cwd != "/later" || got.PendingDirectory != "" {
		t.Fatal(got)
	}
	o.Output([]byte("\x1b]7;file:///"+strings.Repeat("x", 8000)+"\a"), true, now)
	o.Output([]byte("\x1bPignored\x1b[?1049h\x1b\\"), true, now)
	if got := o.Snapshot(now); got.Cwd != "/later" || got.AltScreen {
		t.Fatal(got)
	}
	o.Output([]byte("\x1b]7;file:///safe\a"), true, now)
	if got := o.Snapshot(now); got.Cwd != "/safe" {
		t.Fatal(got)
	}
}

func TestHistoryBoundedScopedCopiedAndClear(t *testing.T) {
	o := prepared()
	for i := range 210 {
		prompt(o)
		o.Commit(o.Input([]byte("echo "+strconv.Itoa(i)+"\r")), time.Now())
	}
	entries := o.History()
	if len(entries) != 200 || entries[0].Sequence != "11" || entries[199].Sequence != "210" {
		t.Fatal("历史边界错误")
	}
	entries[0].Command = "changed"
	if o.History()[0].Command == "changed" || len(NewObserver().History()) != 0 {
		t.Fatal("历史未隔离")
	}
	o.ClearHistory()
	if len(o.History()) != 0 {
		t.Fatal("清空失败")
	}
	prompt(o)
	o.Commit(o.Input([]byte("pwd\r")), time.Now())
	if o.History()[0].Sequence != "211" {
		t.Fatal("清空后复用了序号")
	}
}

func TestBootstrapFilterChunksAndRecovery(t *testing.T) {
	now := time.Now()
	var filter BootstrapFilter
	filter.Start(now)
	visible := filter.Filter([]byte("Welcome\r\nLast login\r\n$ "), now)
	echo := []byte(strings.TrimSuffix(Bootstrap, "\n"))
	for _, b := range append(append(echo, []byte("\r\n\x1b]7;file:///home\a")...), []byte(DoneMarker+"$ ")...) {
		visible = append(visible, filter.Filter([]byte{b}, now)...)
	}
	if !bytes.Contains(visible, []byte("Welcome")) || !bytes.Contains(visible, []byte("Last login")) || bytes.Contains(visible, []byte("__kt_cwd")) || filter.Active() {
		t.Fatal("过滤未保留登录输出或未收尾")
	}
	filter.Start(now)
	fragment := []byte(Bootstrap[:80])
	if len(filter.Filter(fragment, now)) != 0 {
		t.Fatal("不完整回显提前输出")
	}
	if got := filter.Filter([]byte("tail"), now.Add(4*time.Second)); !bytes.Equal(got, append(bytes.Clone(fragment), []byte("tail")...)) {
		t.Fatal("超时丢失未确认数据")
	}
	filter.Start(now)
	filter.Filter(echo, now)
	huge := bytes.Repeat([]byte("x"), 65<<10)
	if got := filter.Filter(huge, now); !bytes.Equal(got, huge) || filter.Active() {
		t.Fatal("超限数据未释放")
	}
	filter.Start(now)
	if got := filter.Filter([]byte("quiet"+DoneMarker+"$ "), now); !bytes.Contains(got, []byte("quiet")) || filter.Active() {
		t.Fatal("无回显过滤失败")
	}
	filter.Start(now)
	if got := filter.Filter(bytes.Repeat([]byte("\r\n"), 16000), now); len(got) != 32000 {
		t.Fatal("纯换行被过滤")
	}
}

func TestDirectoryCommandEscapesAndRuns(t *testing.T) {
	if DirectoryCommand("   ") != "" || DirectoryCommand("/bad\npath") != "" {
		t.Fatal("接受了非法目录")
	}
	directory := t.TempDir() + "/it's here"
	command := DirectoryCommand(directory)
	if !strings.HasPrefix(command, "\x15 printf") || !strings.Contains(command, `'\''`) {
		t.Fatal("未清行或转义")
	}
	for _, value := range []string{"~", "."} {
		if !strings.HasSuffix(DirectoryCommand(value), "; cd\n") {
			t.Fatal("home缩写改变")
		}
	}
	binary, err := exec.LookPath("sh")
	if err != nil {
		t.Skip("未安装POSIX shell")
	}
	root := t.TempDir()
	if err = os.Mkdir(root+"/it's here", 0700); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, binary, "-c", strings.TrimPrefix(DirectoryCommand("./it's here"), "\x15")+`printf '%s' "${PWD##*/}"`)
	cmd.Dir = root
	cmd.Env = []string{"PATH=/usr/bin:/bin"}
	output, err := cmd.CombinedOutput()
	if err != nil || !bytes.HasSuffix(output, []byte("it's here")) {
		t.Fatalf("目录命令执行失败: %v: %q", err, output)
	}
}

func TestBootstrapPreservesRealShellHooks(t *testing.T) {
	cases := []struct{ name, shell, prefix, after string }{
		{"bash-scalar", "bash", `PROMPT_COMMAND='printf "user-hook\n"'; `, `eval "$PROMPT_COMMAND"`},
		{"bash-array", "bash", `PROMPT_COMMAND=('printf "user-hook\n"'); `, `for hook in "${PROMPT_COMMAND[@]}"; do eval "$hook"; done`},
		{"zsh", "zsh", `user_hook(){ printf 'user-hook\n'; }; precmd_functions=(user_hook); `, `for hook in $precmd_functions; do $hook; done`},
	}
	for _, test := range cases {
		t.Run(test.name, func(t *testing.T) {
			binary, err := exec.LookPath(test.shell)
			if err != nil {
				t.Skip("未安装对应shell")
			}
			ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
			defer cancel()
			cmd := exec.CommandContext(ctx, binary, "-f", "-c", test.prefix+Bootstrap+test.after)
			cmd.Dir = t.TempDir()
			cmd.Env = []string{"PATH=/usr/bin:/bin"}
			output, err := cmd.CombinedOutput()
			if err != nil {
				t.Fatalf("shell hook失败: %v: %s", err, output)
			}
			if pos := bytes.Index(output, []byte("user-hook")); pos < 0 || bytes.Index(output, []byte(PromptMarker)) < pos || !bytes.Contains(output, []byte(DoneMarker)) {
				t.Fatalf("用户hook或提示符顺序改变: %q", output)
			}
		})
	}
}
