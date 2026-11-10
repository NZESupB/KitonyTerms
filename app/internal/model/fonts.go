package model

// FontCatalog是本机临时目录，不进入配置或同步；families包含兼容回退名。
type FontCatalog struct {
	Families []string `json:"families"`
	Source   string   `json:"source"`
	Reason   string   `json:"reason,omitempty"`
}
