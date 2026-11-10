// Package monitor 解析固定监控命令的有界输出，不执行用户输入。
package monitor

import (
	"fmt"
	"math"
	"math/bits"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"kitonyterms/internal/model"
)

const (
	Begin     = "__KTM_BEGIN__"
	Section   = "__KTM_SEC__"
	End       = "__KTM_END__"
	MaxOutput = 1 << 20
	MaxLine   = 64 << 10
)

// LC_ALL 只影响本监控 shell；宿主 PTY、登录 hook 与历史不受影响。
const Command = "printf '__KTM_BEGIN__\\n'; " +
	"grep '^cpu' /proc/stat 2>/dev/null; printf '__KTM_SEC__\\n'; " +
	"cat /proc/meminfo 2>/dev/null; printf '__KTM_SEC__\\n'; " +
	"cat /proc/net/dev 2>/dev/null; printf '__KTM_SEC__\\n'; " +
	"LC_ALL=C df -P -k 2>/dev/null; printf '__KTM_SEC__\\n'; " +
	"cat /proc/loadavg 2>/dev/null; printf '__KTM_SEC__\\n'; " +
	"cat /proc/uptime 2>/dev/null; printf '__KTM_SEC__\\n'; " +
	"printf 'hostname=%s\\n' \"$(hostname 2>/dev/null)\"; " +
	"printf 'arch=%s\\n' \"$(uname -m 2>/dev/null)\"; " +
	"awk -F= '/^PRETTY_NAME=/{gsub(/^\"|\"$/, \"\", $2); print \"distro=\" $2; exit}' /etc/os-release 2>/dev/null; " +
	"printf '__KTM_END__\\n'\n"

func Heartbeat(sequence uint64) (command, marker string) {
	marker = "__KTM_HEARTBEAT__" + strconv.FormatUint(sequence, 10) + "__"
	return "printf '" + marker + "\\n'\n", marker
}

type cpuSample struct {
	fields      [8]uint64
	total, busy uint64
}
type netSample struct{ rx, tx uint64 }
type Parser struct {
	cpu map[string]cpuSample
	net map[string]netSample
	at  time.Time
}

func decimal(value uint64) string    { return strconv.FormatUint(value, 10) }
func add(a, b uint64) (uint64, bool) { value, carry := bits.Add64(a, b, 0); return value, carry == 0 }
func kb(text string) (uint64, bool) {
	value, err := strconv.ParseUint(text, 10, 64)
	return value * 1024, err == nil && value <= math.MaxUint64/1024
}
func nonnegative(text string) *float64 {
	value, err := strconv.ParseFloat(text, 64)
	if err != nil || value < 0 || math.IsNaN(value) || math.IsInf(value, 0) {
		return nil
	}
	return &value
}
func cpuValue(fields []string) (cpuSample, bool) {
	var sample cpuSample
	if len(fields) < 4 {
		return sample, false
	}
	for i, text := range fields {
		value, err := strconv.ParseUint(text, 10, 64)
		if err != nil {
			return cpuSample{}, false
		}
		// guest/guest_nice 已包含于 user/nice，不重复累计。
		if i < len(sample.fields) {
			sample.fields[i] = value
			var ok bool
			sample.total, ok = add(sample.total, value)
			if !ok {
				return cpuSample{}, false
			}
		}
	}
	idle, ok := add(sample.fields[3], sample.fields[4])
	if !ok || idle > sample.total {
		return cpuSample{}, false
	}
	sample.busy = sample.total - idle
	return sample, true
}
func cpuPercent(current cpuSample, prior cpuSample, exists bool) *float64 {
	if !exists || current.total <= prior.total {
		return nil
	}
	for i, value := range current.fields {
		if value < prior.fields[i] {
			return nil
		}
	}
	percent := float64(current.busy-prior.busy) / float64(current.total-prior.total) * 100
	return &percent
}
func (p *Parser) parseCPU(text string, stats *model.MonitorStats) {
	current := map[string]cpuSample{}
	for _, line := range strings.Split(text, "\n") {
		fields := strings.Fields(line)
		if len(fields) < 5 || !strings.HasPrefix(fields[0], "cpu") {
			continue
		}
		name := fields[0]
		var id uint64
		if name != "cpu" {
			var err error
			id, err = strconv.ParseUint(strings.TrimPrefix(name, "cpu"), 10, 32)
			if err != nil {
				continue
			}
		}
		if _, duplicate := current[name]; duplicate {
			p.cpu = nil
			stats.CPUPercent, stats.PerCore = nil, []model.MonitorCore{}
			stats.CPUCores = 0
			return
		}
		sample, ok := cpuValue(fields[1:])
		if !ok {
			continue
		}
		current[name] = sample
		prior, exists := p.cpu[name]
		percent := cpuPercent(sample, prior, exists)
		if name == "cpu" {
			stats.CPUPercent = percent
		} else {
			stats.PerCore = append(stats.PerCore, model.MonitorCore{ID: uint32(id), Percent: percent})
		}
	}
	stats.CPUCores = uint32(len(stats.PerCore))
	sort.Slice(stats.PerCore, func(i, j int) bool { return stats.PerCore[i].ID < stats.PerCore[j].ID })
	p.cpu = current
}
func parseMemory(text string, stats *model.MonitorStats) {
	values := map[string]uint64{}
	for _, line := range strings.Split(text, "\n") {
		fields := strings.Fields(line)
		if len(fields) != 3 || fields[2] != "kB" {
			continue
		}
		if value, ok := kb(fields[1]); ok {
			values[fields[0]] = value
		}
	}
	used := func(totalKey, freeKey string) (string, string) {
		total, hasTotal := values[totalKey]
		free, hasFree := values[freeKey]
		if !hasTotal {
			return "", ""
		}
		if !hasFree || free > total {
			return "", decimal(total)
		}
		return decimal(total - free), decimal(total)
	}
	stats.MemUsed, stats.MemTotal = used("MemTotal:", "MemAvailable:")
	stats.SwapUsed, stats.SwapTotal = used("SwapTotal:", "SwapFree:")
}
func virtual(name string) bool {
	if name == "lo" {
		return true
	}
	for _, prefix := range []string{"veth", "br-", "docker", "virbr", "cni", "flannel", "tun", "tap"} {
		if strings.HasPrefix(name, prefix) {
			return true
		}
	}
	return false
}
func rate(delta uint64, elapsed time.Duration) (uint64, bool) {
	if elapsed <= 0 {
		return 0, false
	}
	hi, lo := bits.Mul64(delta, uint64(time.Second))
	if hi >= uint64(elapsed) {
		return 0, false
	}
	value, _ := bits.Div64(hi, lo, uint64(elapsed))
	return value, true
}
func (p *Parser) parseNetwork(text string, elapsed time.Duration, stats *model.MonitorStats) {
	current := map[string]netSample{}
	allKnown, seen := elapsed > 0, false
	var rxDelta, txDelta uint64
	for _, line := range strings.Split(text, "\n") {
		name, rest, found := strings.Cut(line, ":")
		name = strings.TrimSpace(name)
		if !found || name == "" {
			continue
		}
		fields := strings.Fields(rest)
		if len(fields) != 16 {
			allKnown = false
			continue
		}
		var values [16]uint64
		valid := true
		for i, text := range fields {
			var err error
			values[i], err = strconv.ParseUint(text, 10, 64)
			valid = valid && err == nil
		}
		if _, duplicate := current[name]; duplicate {
			allKnown = false
			continue
		}
		if !valid {
			allKnown = false
			continue
		}
		sample := netSample{values[0], values[8]}
		current[name] = sample
		prior, exists := p.net[name]
		known := exists && sample.rx >= prior.rx && sample.tx >= prior.tx && elapsed > 0
		entry := model.MonitorInterface{Name: name, IsVirtual: virtual(name)}
		if known {
			rx, a := rate(sample.rx-prior.rx, elapsed)
			tx, b := rate(sample.tx-prior.tx, elapsed)
			known = a && b
			if known {
				entry.RXRate, entry.TXRate = decimal(rx), decimal(tx)
			}
		}
		stats.Interfaces = append(stats.Interfaces, entry)
		if name == "lo" {
			continue
		}
		seen = true
		allKnown = allKnown && known
		if known {
			var a, b bool
			rxDelta, a = add(rxDelta, sample.rx-prior.rx)
			txDelta, b = add(txDelta, sample.tx-prior.tx)
			allKnown = allKnown && a && b
		}
	}
	for name := range p.net {
		if _, exists := current[name]; !exists && name != "lo" {
			allKnown = false
		}
	}
	if allKnown && seen {
		rx, a := rate(rxDelta, elapsed)
		tx, b := rate(txDelta, elapsed)
		if a && b {
			stats.NetRXRate, stats.NetTXRate = decimal(rx), decimal(tx)
		}
	}
	sort.Slice(stats.Interfaces, func(i, j int) bool { return stats.Interfaces[i].Name < stats.Interfaces[j].Name })
	p.net = current
}
func parseDisks(text string) []model.MonitorDisk {
	disks := []model.MonitorDisk{}
	for _, line := range strings.Split(text, "\n") {
		fields := strings.Fields(line)
		if len(fields) < 6 {
			continue
		}
		mount := strings.Join(fields[5:], " ")
		total, a := kb(fields[1])
		used, b := kb(fields[2])
		if !a || !b || total == 0 || used > total || !strings.HasPrefix(mount, "/") {
			continue
		}
		disks = append(disks, model.MonitorDisk{Mount: mount, Used: decimal(used), Total: decimal(total)})
	}
	return disks
}

func (p *Parser) Parse(raw string, now time.Time) (*model.MonitorStats, error) {
	if len(raw) > MaxOutput || !utf8.ValidString(raw) {
		return nil, fmt.Errorf("资源监控输出无效或超过限制")
	}
	lines := strings.Split(strings.TrimSpace(raw), "\n")
	if len(lines) < 2 || lines[0] != Begin || lines[len(lines)-1] != End {
		return nil, fmt.Errorf("资源监控采样边界无效")
	}
	sections := strings.Split(strings.Join(lines[1:len(lines)-1], "\n"), Section+"\n")
	if len(sections) != 7 {
		return nil, fmt.Errorf("资源监控采样段数无效")
	}
	stats := &model.MonitorStats{PerCore: []model.MonitorCore{}, Interfaces: []model.MonitorInterface{}}
	p.parseCPU(sections[0], stats)
	parseMemory(sections[1], stats)
	elapsed := time.Duration(0)
	if !p.at.IsZero() {
		elapsed = now.Sub(p.at)
	}
	p.parseNetwork(sections[2], elapsed, stats)
	stats.Disks = parseDisks(sections[3])
	if fields := strings.Fields(sections[4]); len(fields) > 0 {
		stats.Load1 = nonnegative(fields[0])
	}
	if fields := strings.Fields(sections[5]); len(fields) > 0 {
		// /proc/uptime 的秒整数可完整保留，不经 float64 转换。
		integer, fraction, _ := strings.Cut(fields[0], ".")
		if value, err := strconv.ParseUint(integer, 10, 64); err == nil && (fraction == "" || digits(fraction)) {
			stats.UptimeSecs = decimal(value)
		}
	}
	for _, line := range strings.Split(sections[6], "\n") {
		key, value, _ := strings.Cut(line, "=")
		value = strings.TrimSpace(value)
		switch key {
		case "hostname":
			stats.Hostname = value
		case "distro":
			stats.Distro = value
		case "arch":
			stats.Architecture = value
		}
	}
	p.at = now
	return stats, nil
}
func digits(text string) bool {
	for _, ch := range text {
		if ch < '0' || ch > '9' {
			return false
		}
	}
	return text != ""
}
