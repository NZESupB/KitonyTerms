package main

import (
	"bytes"
	"context"
	"strings"
	"testing"
)

func TestArgumentsRejectUnknownDuplicateAndPrivateValues(t *testing.T) {
	for _, args := range [][]string{nil, {"unknown", "private-marker"}, {"verify", "--snapshot", "private-marker", "--snapshot", "other"}, {"snapshot", "--config", "private-marker", "--data", "other"}, {"restore", "--snapshot", "private-marker", "--output", "relative"}} {
		var output bytes.Buffer
		err := run(context.Background(), args, &output)
		if err == nil || strings.Contains(err.Error(), "private-marker") || output.Len() != 0 {
			t.Fatal("参数失败必须拒绝且不回显用户路径")
		}
	}
}
