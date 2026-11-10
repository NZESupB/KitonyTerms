package remoteops

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"kitonyterms/internal/model"
	"math"
	"strconv"
	"strings"
	"unicode/utf8"
)

func parseFailure() error {
	return Failure("parse_failed", "远程运维响应格式无效或字段不完整")
}
func lines(text string) ([]string, error) {
	if !utf8.ValidString(text) {
		return nil, parseFailure()
	}
	out := []string{}
	for _, line := range strings.Split(text, "\n") {
		if strings.TrimSpace(line) != "" {
			out = append(out, strings.TrimSpace(line))
		}
	}
	if len(out) > 50000 {
		return nil, Failure("output_limit", "运维响应超过 50000 条限制")
	}
	return out, nil
}
func jsonValue(text string, out any) error {
	d := json.NewDecoder(strings.NewReader(text))
	if d.Decode(out) != nil {
		return parseFailure()
	}
	var extra any
	if d.Decode(&extra) != io.EOF {
		return parseFailure()
	}
	return nil
}
func parseCapabilities(text string) (*model.OperationsCapabilities, error) {
	var fields map[string]json.RawMessage
	if jsonValue(text, &fields) != nil || len(fields) != 8 {
		return nil, parseFailure()
	}
	for _, key := range []string{"linux", "systemd", "ps", "proc", "ss", "proc_net", "docker_daemon", "compose"} {
		raw, ok := fields[key]
		if !ok || (!bytes.Equal(bytes.TrimSpace(raw), []byte("true")) && !bytes.Equal(bytes.TrimSpace(raw), []byte("false"))) {
			return nil, parseFailure()
		}
	}
	var out model.OperationsCapabilities
	if jsonValue(text, &out) != nil {
		return nil, parseFailure()
	}
	return &out, nil
}
func finite(raw string) (float64, error) {
	value, err := strconv.ParseFloat(raw, 64)
	if err != nil || math.IsNaN(value) || math.IsInf(value, 0) || value < 0 {
		return 0, parseFailure()
	}
	return value, nil
}
func unsigned(raw string, bits int) (uint64, error) {
	v, err := strconv.ParseUint(raw, 10, bits)
	if err != nil {
		return 0, parseFailure()
	}
	return v, nil
}
func percent(raw string) (*float64, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" || raw == "--" {
		return nil, nil
	}
	if !strings.HasSuffix(raw, "%") {
		return nil, parseFailure()
	}
	v, err := finite(strings.TrimSpace(strings.TrimSuffix(raw, "%")))
	if err != nil {
		return nil, err
	}
	return &v, nil
}
func memoryBytes(raw string) (string, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" || raw == "--" {
		return "", nil
	}
	split := strings.IndexFunc(raw, func(r rune) bool { return (r < '0' || r > '9') && r != '.' })
	if split <= 0 {
		return "", parseFailure()
	}
	value, err := finite(raw[:split])
	if err != nil {
		return "", err
	}
	factor, ok := map[string]float64{"B": 1, "kB": 1e3, "KB": 1e3, "MB": 1e6, "GB": 1e9, "TB": 1e12, "KiB": 1024, "MiB": 1 << 20, "GiB": 1 << 30, "TiB": 1 << 40}[strings.TrimSpace(raw[split:])]
	result := math.Round(value * factor)
	if !ok || math.IsInf(result, 0) || result >= math.Exp2(64) {
		return "", parseFailure()
	}
	return strconv.FormatUint(uint64(result), 10), nil
}
func size(raw json.RawMessage) (string, error) {
	if len(raw) == 0 || string(raw) == "null" || string(raw) == "-1" {
		return "", nil
	}
	v, err := unsigned(string(raw), 64)
	if err != nil {
		return "", err
	}
	return strconv.FormatUint(v, 10), nil
}
func Parse(r model.OperationRequest, text string) (model.OperationReply, error) {
	out := model.OperationReply{RequestID: r.ID, Kind: r.Kind, Services: []model.ServiceSummary{}, Processes: []model.ProcessSummary{}, Network: []model.NetworkConnection{}, Docker: []model.DockerItem{}, Runtime: []model.DockerRuntime{}, Storage: []model.DockerStorage{}}
	rows, err := lines(text)
	if err != nil {
		return out, err
	}
	switch r.Kind {
	case "capabilities":
		out.Capabilities, err = parseCapabilities(text)
	case "services":
		for _, line := range rows {
			fields := strings.Fields(line)
			if len(fields) < 4 || !serviceName.MatchString(fields[0]) {
				return out, parseFailure()
			}
			out.Services = append(out.Services, model.ServiceSummary{Name: fields[0], LoadState: fields[1], ActiveState: fields[2], SubState: fields[3], Description: strings.Join(fields[4:], " ")})
		}
	case "processes":
		for _, line := range rows {
			f := strings.Fields(line)
			if len(f) < 10 {
				return out, parseFailure()
			}
			numbers := make([]uint64, 3)
			for i := range 3 {
				numbers[i], err = unsigned(f[i], 32)
				if err != nil {
					return out, err
				}
			}
			if numbers[0] == 0 {
				return out, parseFailure()
			}
			cpu, e := finite(f[4])
			if e != nil {
				return out, e
			}
			mem, e := finite(f[5])
			if e != nil {
				return out, e
			}
			rss, e := unsigned(f[6], 64)
			if e != nil {
				return out, e
			}
			vsz, e := unsigned(f[7], 64)
			if e != nil {
				return out, e
			}
			out.Processes = append(out.Processes, model.ProcessSummary{PID: uint32(numbers[0]), PPID: uint32(numbers[1]), UID: uint32(numbers[2]), State: f[3], CPUPercent: cpu, MemoryPercent: mem, RSSKiB: strconv.FormatUint(rss, 10), VSZKiB: strconv.FormatUint(vsz, 10), Elapsed: f[8], Command: strings.Join(f[9:], " ")})
		}
	case "network":
		for _, line := range rows {
			f := strings.Fields(line)
			if len(f) < 6 {
				return out, parseFailure()
			}
			owner := strings.Join(f[6:], " ")
			out.Network = append(out.Network, model.NetworkConnection{Protocol: f[0], State: f[1], Local: f[4], Peer: f[5], Owner: owner, OwnerAvailable: owner != ""})
		}
	case "docker_list":
		if r.Resource == "compose" && strings.HasPrefix(strings.TrimSpace(text), "[") {
			var values []json.RawMessage
			if jsonValue(text, &values) != nil || len(values) > 50000 {
				return out, parseFailure()
			}
			rows = []string{}
			for _, v := range values {
				rows = append(rows, string(v))
			}
		}
		for _, line := range rows {
			var f map[string]string
			if jsonValue(line, &f) != nil || f == nil {
				return out, parseFailure()
			}
			item := model.DockerItem{ID: f["ID"], Name: f["Names"], Status: f["Status"], State: f["State"], Image: f["Image"], Repository: f["Repository"], Tag: f["Tag"], Size: f["Size"], Created: f["CreatedSince"], Driver: f["Driver"], Scope: f["Scope"]}
			if r.Resource == "volumes" {
				item.ID = f["Name"]
				item.Name = f["Name"]
			}
			if r.Resource == "networks" {
				item.Name = f["Name"]
			}
			if r.Resource == "compose" {
				item.ID = f["Name"]
				if item.ID == "" {
					item.ID = f["name"]
				}
				item.Name = item.ID
				item.ConfigFiles = f["ConfigFiles"]
				if item.ConfigFiles == "" {
					item.ConfigFiles = f["config_files"]
				}
				if item.Status == "" {
					item.Status = f["status"]
				}
			}
			if !resourceID.MatchString(item.ID) {
				return out, parseFailure()
			}
			if item.Name == "" {
				item.Name = item.ID
			}
			out.Docker = append(out.Docker, item)
		}
	case "docker_stats":
		for _, line := range rows {
			var f map[string]string
			if jsonValue(line, &f) != nil || !resourceID.MatchString(f["ID"]) {
				return out, parseFailure()
			}
			cpu, e := percent(f["CPUPerc"])
			if e != nil {
				return out, e
			}
			mem, e := percent(f["MemPerc"])
			if e != nil {
				return out, e
			}
			used, limit := "", ""
			raw := strings.TrimSpace(f["MemUsage"])
			if raw != "" && raw != "--" {
				a, b, ok := strings.Cut(raw, "/")
				if !ok {
					return out, parseFailure()
				}
				used, e = memoryBytes(a)
				if e != nil {
					return out, e
				}
				limit, e = memoryBytes(b)
				if e != nil {
					return out, e
				}
			}
			out.Runtime = append(out.Runtime, model.DockerRuntime{ID: f["ID"], CPUPercent: cpu, MemoryPercent: mem, MemoryUsedBytes: used, MemoryLimitBytes: limit})
		}
	case "docker_storage", "docker_image_sizes":
		for _, line := range rows {
			var f struct {
				ID      string          `json:"id"`
				ImageID string          `json:"image_id"`
				RW      json.RawMessage `json:"size_rw"`
				RootFS  json.RawMessage `json:"size_rootfs"`
				Size    json.RawMessage `json:"size"`
			}
			if jsonValue(line, &f) != nil || !resourceID.MatchString(f.ID) {
				return out, parseFailure()
			}
			if r.Kind == "docker_storage" && !resourceID.MatchString(f.ImageID) {
				return out, parseFailure()
			}
			rw, e := size(f.RW)
			if e != nil {
				return out, e
			}
			root, e := size(f.RootFS)
			if e != nil {
				return out, e
			}
			sz, e := size(f.Size)
			if e != nil {
				return out, e
			}
			out.Storage = append(out.Storage, model.DockerStorage{ID: f.ID, ImageID: f.ImageID, SizeRW: rw, SizeRootFS: root, Size: sz})
		}
	default:
		if IsAction(r) {
			out.Output = "操作已完成"
		} else {
			out.Output = text
			out.Title = fmt.Sprintf("%s %s", r.Kind, r.Target)
		}
	}
	return out, err
}
