package fonts

import (
	"bytes"
	"context"
	"errors"
	"os/exec"
	"strings"
	"time"
	"unicode/utf8"
)

type limitedOutput struct {
	buffer bytes.Buffer
	limit  int
}

func (out *limitedOutput) Bytes() []byte  { return out.buffer.Bytes() }
func (out *limitedOutput) String() string { return out.buffer.String() }

func (out *limitedOutput) Write(data []byte) (int, error) {
	if len(data) > out.limit-out.buffer.Len() {
		return 0, errEnumeration
	}
	return out.buffer.Write(data)
}

// 固定argv与首个族名投影，不经过shell，也不把字体文件路径带回界面。
func fontconfigFamilies(ctx context.Context, program string) ([]string, error) {
	cmd := exec.CommandContext(ctx, program, "--format=%{family[0]}\\n")
	cmd.WaitDelay = 250 * time.Millisecond
	stdout := &limitedOutput{limit: 4 << 20}
	stderr := &limitedOutput{limit: 16 << 10}
	cmd.Stdout, cmd.Stderr = stdout, stderr
	if err := cmd.Run(); err != nil {
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}
		if errors.Is(err, exec.ErrNotFound) {
			return nil, errUnavailable
		}
		return nil, errEnumeration
	}
	if !utf8.Valid(stdout.Bytes()) {
		return nil, errEnumeration
	}
	return strings.Split(stdout.String(), "\n"), nil
}
