// 独立离线工具；不会从系统配置自动发现或启动正式应用。
package main

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"os"
	"os/signal"

	"kitonyterms/internal/datasnapshot"
)

func run(ctx context.Context, args []string, output io.Writer) error {
	invalid := func() error {
		return errors.New("参数无效：snapshot --config 绝对路径 --data 绝对路径 --output 新绝对路径；verify --snapshot 绝对路径；restore --snapshot 绝对路径 --output 新绝对路径")
	}
	if len(args) == 0 {
		return invalid()
	}
	values := map[string]string{}
	for i := 1; i < len(args); i += 2 {
		if i+1 >= len(args) || args[i+1] == "" {
			return invalid()
		}
		if _, ok := values[args[i]]; ok {
			return invalid()
		}
		values[args[i]] = args[i+1]
	}
	var result datasnapshot.Summary
	var err error
	switch args[0] {
	case "snapshot":
		if len(values) != 3 || values["--config"] == "" || values["--data"] == "" || values["--output"] == "" {
			return invalid()
		}
		result, err = datasnapshot.Create(ctx, values["--config"], values["--data"], values["--output"])
	case "verify":
		if len(values) != 1 || values["--snapshot"] == "" {
			return invalid()
		}
		result, err = datasnapshot.Verify(ctx, values["--snapshot"])
	case "restore":
		if len(values) != 2 || values["--snapshot"] == "" || values["--output"] == "" {
			return invalid()
		}
		result, err = datasnapshot.Restore(ctx, values["--snapshot"], values["--output"])
	default:
		return invalid()
	}
	if err != nil {
		return err
	}
	return json.NewEncoder(output).Encode(result)
}
func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()
	if err := run(ctx, os.Args[1:], os.Stdout); err != nil {
		os.Stderr.WriteString(err.Error() + "\n")
		os.Exit(1)
	}
}
