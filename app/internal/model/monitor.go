package model

// 空字符串或 nil 表示未知；累计字节和速率不经过 JavaScript 浮点数。
type MonitorStats struct {
	Hostname     string             `json:"hostname"`
	Distro       string             `json:"distro"`
	Architecture string             `json:"architecture"`
	CPUPercent   *float64           `json:"cpuPercent"`
	CPUCores     uint32             `json:"cpuCores"`
	PerCore      []MonitorCore      `json:"perCore"`
	MemUsed      string             `json:"memUsed"`
	MemTotal     string             `json:"memTotal"`
	SwapUsed     string             `json:"swapUsed"`
	SwapTotal    string             `json:"swapTotal"`
	NetRXRate    string             `json:"netRxRate"`
	NetTXRate    string             `json:"netTxRate"`
	Interfaces   []MonitorInterface `json:"interfaces"`
	Disks        []MonitorDisk      `json:"disks"`
	Load1        *float64           `json:"load1"`
	UptimeSecs   string             `json:"uptimeSecs"`
	LatencyMS    string             `json:"latencyMs"`
}

type MonitorCore struct {
	ID      uint32   `json:"id"`
	Percent *float64 `json:"percent"`
}
type MonitorInterface struct {
	Name      string `json:"name"`
	RXRate    string `json:"rxRate"`
	TXRate    string `json:"txRate"`
	IsVirtual bool   `json:"isVirtual"`
}
type MonitorDisk struct {
	Mount string `json:"mount"`
	Used  string `json:"used"`
	Total string `json:"total"`
}
type MonitorSnapshot struct {
	SessionID  string        `json:"sessionId"`
	Generation string        `json:"generation"`
	Sequence   string        `json:"sequence"`
	Status     string        `json:"status"`
	Error      string        `json:"error"`
	SampledAt  string        `json:"sampledAt"`
	Stats      *MonitorStats `json:"stats"`
}
