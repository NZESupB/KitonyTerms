package monitor

import (
	"fmt"
	"math"
	"strings"
	"testing"
	"time"
)

func block(cpu, mem, net, disk, load, uptime string) string {
	return Begin + "\n" + strings.Join([]string{cpu, mem, net, disk, load, uptime, "hostname=node-1\narch=x86_64\ndistro=Debian\n"}, Section+"\n") + End + "\n"
}
func netLine(name string, rx, tx uint64) string {
	return fmt.Sprintf("%s: %d 0 0 0 0 0 0 0 %d 0 0 0 0 0 0 0\n", name, rx, tx)
}
func TestMonitorDeltasUnknownResetAndInterfaceRemoval(t *testing.T) {
	p := Parser{}
	at := time.Now()
	cpu := "cpu 100 0 0 100 0 0 0 0 50 0\ncpu0 100 0 0 100\n"
	mem := "MemTotal: 1024 kB\nMemAvailable: 512 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n"
	disk := "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/x 1000 400 600 40% /\n"
	first, err := p.Parse(block(cpu, mem, netLine("lo", 999, 999)+netLine("eth0", 100, 200), disk, "0.25 0.1 0.0\n", "9007199254740993.42 0\n"), at)
	if err != nil {
		t.Fatal(err)
	}
	if first.CPUPercent != nil || first.NetRXRate != "" || first.PerCore[0].Percent != nil {
		t.Fatal("首采样被误报为零")
	}
	if first.MemUsed != "524288" || first.SwapUsed != "0" || first.UptimeSecs != "9007199254740993" || first.Hostname != "node-1" || first.Disks[0].Total != "1024000" {
		t.Fatalf("字段解析错误: %+v", first)
	}
	cpu = "cpu 150 0 0 250 0 0 0 0 90 0\ncpu0 150 0 0 250\n"
	second, err := p.Parse(block(cpu, mem, netLine("lo", 999999, 999999)+netLine("eth0", 300, 500), disk, "0.25\n", "3600.5\n"), at.Add(2*time.Second))
	if err != nil {
		t.Fatal(err)
	}
	if second.CPUPercent == nil || *second.CPUPercent != 25 || second.NetRXRate != "100" || second.NetTXRate != "150" {
		t.Fatalf("差分/guest重复累计: %+v", second)
	}
	reset, _ := p.Parse(block("cpu 1 0 0 1\ncpu1 1 0 0 1\n", mem, netLine("eth0", 1, 1)+netLine("veth1", 10, 10), disk, "NaN\n", "bad\n"), at.Add(4*time.Second))
	if reset.CPUPercent != nil || reset.NetRXRate != "" || reset.Load1 != nil || reset.UptimeSecs != "" || !reset.Interfaces[1].IsVirtual || len(p.cpu) != 2 {
		t.Fatalf("重置/新增值未未知: %+v", reset)
	}
	removed, _ := p.Parse(block("cpu 2 0 0 2\n", mem, netLine("eth0", 3, 3), disk, "-1\n", "-1\n"), at.Add(6*time.Second))
	if removed.NetRXRate != "" || len(p.net) != 1 || len(p.cpu) != 1 {
		t.Fatal("消失网卡/核心未清理")
	}
	stable, _ := p.Parse(block("cpu 2 0 0 2\n", mem, netLine("eth0", 3, 3), disk, "0\n", "0\n"), at.Add(8*time.Second))
	if stable.NetRXRate != "0" || stable.NetTXRate != "0" || stable.CPUPercent != nil {
		t.Fatal("已知零/不变CPU处理错误")
	}
}
func TestMonitorMalformedFieldsOverflowAndBounds(t *testing.T) {
	p := Parser{}
	raw := block("cpu 1 bad 3 4\ncpu0 18446744073709551615 1 0 0\n", "MemTotal: 18446744073709551615 kB\nMemAvailable: 1 kB\nSwapTotal: 2 kB\n", "eth0: 1 bad 0 0 0 0 0 0 3 0 0 0 0 0 0 0\n", "/dev/x 18446744073709551615 1 0 0% /\n", "+Inf\n", "bad\n")
	stats, err := p.Parse(raw, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if stats.CPUPercent != nil || stats.CPUCores != 0 || stats.MemUsed != "" || stats.SwapUsed != "" || stats.Load1 != nil || len(stats.Disks) != 0 || len(stats.Interfaces) != 0 {
		t.Fatal("无效字段被接受", stats)
	}
	for _, text := range []string{"", Begin + "\n" + End, strings.Repeat("x", MaxOutput+1), strings.Replace(raw, "node-1", string([]byte{255}), 1)} {
		if _, err := p.Parse(text, time.Now()); err == nil {
			t.Fatal("无效帧/超限/UTF8被接受")
		}
	}
	if _, ok := rate(math.MaxUint64, time.Nanosecond); ok {
		t.Fatal("速率溢出被接受")
	}
	if value, ok := rate(math.MaxUint64, time.Second); !ok || value != math.MaxUint64 {
		t.Fatal("精确uint64速率丢失")
	}
}
func TestMonitorMissingMetricsAndPartialReset(t *testing.T) {
	p := Parser{}
	at := time.Now()
	_, _ = p.Parse(block("cpu 10 0 0 100 20\n", "", "", "", "", ""), at)
	stats, err := p.Parse(block("cpu 30 0 0 200 10\n", "MemTotal: 10 kB\n", "", "invalid df\n", "0.5\n", "1\n"), at.Add(time.Second))
	if err != nil || stats.CPUPercent != nil || stats.MemUsed != "" || stats.MemTotal != "10240" || stats.Load1 == nil || *stats.Load1 != 0.5 {
		t.Fatal("部分重置或缺失值处理错误", err, stats)
	}
	command, marker := Heartbeat(42)
	if marker != "__KTM_HEARTBEAT__42__" || !strings.Contains(command, marker) || !strings.HasSuffix(Command, "\n") {
		t.Fatal("心跳/命令包格式错误")
	}
}

func TestMonitorDuplicateCPUInvalidatesBaseline(t *testing.T) {
	p := Parser{}
	at := time.Now()
	_, _ = p.Parse(block("cpu 1 0 0 1\n", "", "", "", "", ""), at)
	stats, err := p.Parse(block("cpu 2 0 0 2\ncpu 3 0 0 3\n", "", "", "", "", ""), at.Add(time.Second))
	if err != nil || stats.CPUPercent != nil || len(p.cpu) != 0 {
		t.Fatal("重复CPU保留可疑基线", err)
	}
	stats, _ = p.Parse(block("cpu 4 0 0 4\n", "", "", "", "", ""), at.Add(2*time.Second))
	if stats.CPUPercent != nil {
		t.Fatal("重复CPU后首样本未重建")
	}
}
