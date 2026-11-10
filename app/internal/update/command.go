package update

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
)

// EncodeBuildRecord 沿用正式流水线的schema1记录，仅编码已校验的公开构建身份。
func EncodeBuildRecord(data []byte, expectedVersion string) (string, error) {
	if len(data) > 8192 {
		return "", fmt.Errorf("构建记录过大")
	}
	var record struct {
		BuildInfo
		Schema int    `json:"schema"`
		Label  string `json:"label"`
	}
	if err := decodeJSON(data, &record, true); err != nil {
		return "", fmt.Errorf("构建记录JSON无效")
	}
	if err := record.BuildInfo.Validate(); err != nil {
		return "", err
	}
	if record.Schema != 1 || record.Version != expectedVersion {
		return "", fmt.Errorf("构建记录协议或应用版本不匹配")
	}
	label := "alpha"
	if record.Channel == "stable" {
		label = "v" + record.Version
	}
	if record.Label != label {
		return "", fmt.Errorf("构建记录标签不匹配")
	}
	encoded, err := json.Marshal(record.BuildInfo)
	if err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(encoded), nil
}

func HandleCommand(args []string, output io.Writer) (bool, error) {
	if len(args) == 0 {
		return false, nil
	}
	switch args[0] {
	case "--update-info":
		if len(args) != 1 {
			return true, fmt.Errorf("更新身份命令不接受其他参数")
		}
		info, err := CurrentBuild()
		if err != nil {
			return true, err
		}
		return true, json.NewEncoder(output).Encode(info)
	case "--apply-update", "--relaunch-update", "--elevated-replace", "--restore-update":
		return true, fmt.Errorf("此构建尚未提供更新安装辅助入口")
	default:
		return false, nil
	}
}
