package main

import (
	"encoding/json"
	"fmt"
	"io"
	"os"

	"kitonyterms/internal/update"
)

func main() {
	if err := encode(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
func encode() error {
	if len(os.Args) != 1 {
		return fmt.Errorf("请从标准输入传入schema1构建记录")
	}
	data, err := io.ReadAll(io.LimitReader(os.Stdin, 8193))
	if err != nil {
		return err
	}
	packageData, err := os.ReadFile("frontend/package.json")
	if err != nil {
		return fmt.Errorf("请在app目录运行构建身份工具")
	}
	var pkg struct {
		Version string `json:"version"`
	}
	if err = json.Unmarshal(packageData, &pkg); err != nil {
		return err
	}
	identity, err := update.EncodeBuildRecord(data, pkg.Version)
	if err != nil {
		return err
	}
	_, err = fmt.Fprintln(os.Stdout, identity)
	return err
}
