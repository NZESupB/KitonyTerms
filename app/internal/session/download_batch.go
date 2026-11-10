package session

import (
	"context"
	"fmt"
	"io"
	"kitonyterms/internal/filepolicy"
	"os"
	"path"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/pkg/sftp"
	"kitonyterms/internal/model"
)

func portableName(name string) bool { return filepolicy.Name(name) }

func validateDownload(root *os.Root, nodes []remoteNode) error {
	names := map[string]bool{}
	for _, node := range nodes {
		for _, name := range strings.Split(node.relative, "/") {
			if !portableName(name) {
				return fmt.Errorf("名称不能安全保存到本地: %s", node.relative)
			}
		}
		key := strings.ToLower(node.relative)
		if names[key] {
			return fmt.Errorf("本地名称冲突: %s", node.relative)
		}
		names[key] = true
		// 完整预检也拒绝已有的符号链接和目录，不合并现有目录。
		if _, err := root.Lstat(filepath.FromSlash(node.relative)); err == nil {
			return fmt.Errorf("本地目标已存在: %s", node.relative)
		} else if !os.IsNotExist(err) {
			return err
		}
	}
	return nil
}

func copyRemoteFile(ctx context.Context, client *sftp.Client, root *os.Root, node remoteNode) (int64, error) {
	current, err := client.Lstat(node.remote)
	if err != nil {
		return 0, err
	}
	if !current.Mode().IsRegular() {
		return 0, fmt.Errorf("远端类型已改变: %s", node.remote)
	}
	source, err := client.Open(node.remote)
	if err != nil {
		return 0, err
	}
	defer source.Close()
	stop := context.AfterFunc(ctx, func() { _ = source.Close() })
	defer stop()
	temporary := filepath.FromSlash(path.Join(path.Dir(node.relative), ".kitony-download-"+randomID()))
	target := filepath.FromSlash(node.relative)
	private, _ := ctx.Value(privateDownloadStagingKey{}).(bool)
	if private {
		temporary = target
	}
	file, err := root.OpenFile(temporary, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return 0, err
	}
	removeTemporary := true
	defer func() {
		if removeTemporary {
			_ = root.Remove(temporary)
		}
	}()
	count, copyErr := io.Copy(budgetWriter{progressWriter{file, transferFrom(ctx)}, downloadBudgetFrom(ctx)}, contextReader{ctx, source})
	if count != node.info.Size() {
		transferFrom(ctx).unknownTotal()
	}
	if copyErr == nil {
		copyErr = file.Sync()
	}
	closeErr := file.Close()
	if copyErr != nil {
		return 0, copyErr
	}
	if closeErr != nil {
		return 0, closeErr
	}
	if err = ctx.Err(); err != nil {
		return 0, err
	}
	if private {
		removeTemporary = false
		return count, nil
	}
	// 硬链接提交具有“不覆盖”语义；Root 将所有本地操作约束在所选目录。
	if err = root.Link(temporary, target); err != nil {
		return 0, fmt.Errorf("提交下载失败，目标未覆盖: %w", err)
	}
	if err = root.Remove(temporary); err != nil {
		return count, fmt.Errorf("下载已提交，临时文件清理失败: %w", err)
	}
	return count, nil
}

func (m *Manager) DownloadBatch(ctx context.Context, id, generation string, remotes []string, destination string) (model.TransferSummary, error) {
	result := model.TransferSummary{Bytes: "0"}
	s, err := m.Get(id, generation)
	if err != nil {
		return result, err
	}
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	stop := context.AfterFunc(s.ctx, cancel)
	defer stop()
	client, release, err := s.fileScope(ctx)
	if err != nil {
		return result, err
	}
	defer release()
	nodes, err := scanRemote(ctx, client, remotes)
	if err != nil {
		return result, err
	}
	if err = checkDownloadBudget(ctx, nodes); err != nil {
		return result, err
	}
	root, err := os.OpenRoot(destination)
	if err != nil {
		return result, err
	}
	defer root.Close()
	if err = validateDownload(root, nodes); err != nil {
		return result, err
	}
	job := transferFrom(ctx)
	sizes := []int64{}
	for _, node := range nodes {
		if !node.info.IsDir() {
			sizes = append(sizes, node.info.Size())
		}
	}
	job.plan(sizes)
	var total uint64
	for _, node := range nodes {
		if err = ctx.Err(); err != nil {
			return result, err
		}
		job.current(node.relative)
		if node.info.IsDir() {
			err = root.Mkdir(filepath.FromSlash(node.relative), 0700)
		} else {
			var count int64
			count, err = copyRemoteFile(ctx, client, root, node)
			if err == nil {
				result.Files++
				job.completed()
				total += uint64(count)
				result.Bytes = strconv.FormatUint(total, 10)
			}
		}
		if err != nil {
			return result, fmt.Errorf("下载停止于 %s，已完成文件保留: %w", node.relative, err)
		}
	}
	return result, nil
}
