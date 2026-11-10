package update

import (
	"archive/tar"
	"archive/zip"
	"bytes"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"testing"
	"time"
)

func archiveTestBuild() BuildInfo {
	return BuildInfo{Channel: "stable", Version: "1.2.3", Commit: strings.Repeat("a", 40), Build: 123, Attempt: 1, Target: desktopTarget()}
}

func requireUpdateTestSymlink(t *testing.T, err error) {
	t.Helper()
	// ERROR_PRIVILEGE_NOT_HELD：不把缺少系统链接权限当作产品失败或验收通过。
	if runtime.GOOS == "windows" && errors.Is(err, syscall.Errno(1314)) {
		t.Skip("当前Windows账户缺少符号链接权限，此链接分支未验收")
	}
	if err != nil {
		t.Fatal(err)
	}
}

func TestMain(m *testing.M) {
	if os.Getenv("KITONYTERMS_UPDATE_TEST_HELPER") == "1" && len(os.Args) > 1 {
		switch os.Args[1] {
		case "--relaunch-update":
			os.Exit(runTestRecoveryRelaunch())
		case "--relaunch-old-process":
			os.Exit(runRelaunchOldProcess(os.Args[2:]))
		case "--apply-update":
			os.Exit(runTestApplyHelper())
		case "--install-lock-probe":
			os.Exit(runInstallLockProbe(os.Args[2:]))
		case "--install-crash-probe":
			os.Exit(runInstallCrashProbe(os.Args[2:]))
		case "--update-info":
			info, err := testFixtureBuildInfo()
			if err != nil {
				os.Exit(2)
			}
			if err := json.NewEncoder(os.Stdout).Encode(info); err != nil {
				os.Exit(2)
			}
			os.Exit(0)
		case "--overflow":
			_, _ = os.Stdout.Write(bytes.Repeat([]byte("x"), 1<<20))
			os.Exit(0)
		case "--wait":
			time.Sleep(20 * time.Second)
			os.Exit(0)
		}
	}
	os.Exit(m.Run())
}

func archiveTestRoot(t *testing.T) *os.Root {
	t.Helper()
	root, err := os.OpenRoot(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := root.Close(); err != nil {
			t.Error(err)
		}
	})
	return root
}

type testArchiveEntry struct {
	name, body string
	mode       fs.FileMode
	kind       byte
}

func testZIP(t *testing.T, entries []testArchiveEntry) []byte {
	t.Helper()
	var buffer bytes.Buffer
	writer := zip.NewWriter(&buffer)
	for _, entry := range entries {
		header := &zip.FileHeader{Name: entry.name, Method: zip.Store}
		mode := entry.mode
		if mode == 0 {
			mode = 0644
		}
		header.SetMode(mode)
		file, err := writer.CreateHeader(header)
		if err != nil {
			t.Fatal(err)
		}
		if _, err = io.WriteString(file, entry.body); err != nil {
			t.Fatal(err)
		}
	}
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	return buffer.Bytes()
}

func testTAR(t *testing.T, entries []testArchiveEntry) []byte {
	t.Helper()
	var buffer bytes.Buffer
	compressed := gzip.NewWriter(&buffer)
	writer := tar.NewWriter(compressed)
	for _, entry := range entries {
		kind := entry.kind
		if kind == 0 {
			kind = tar.TypeReg
		}
		size := int64(len(entry.body))
		if kind != tar.TypeReg {
			size = 0
		}
		if err := writer.WriteHeader(&tar.Header{Name: entry.name, Mode: 0755, Size: size, Typeflag: kind, Linkname: "outside"}); err != nil {
			t.Fatal(err)
		}
		if size > 0 {
			if _, err := io.WriteString(writer, entry.body); err != nil {
				t.Fatal(err)
			}
		}
	}
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	if err := compressed.Close(); err != nil {
		t.Fatal(err)
	}
	return buffer.Bytes()
}

func extractTest(t *testing.T, kind string, data []byte, limits archiveLimits, ctx context.Context) (*os.Root, error) {
	t.Helper()
	root := archiveTestRoot(t)
	name := filepath.Join(t.TempDir(), "input")
	if err := os.WriteFile(name, data, 0600); err != nil {
		t.Fatal(err)
	}
	file, err := os.Open(name)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	tree := newArchiveTree(ctx, root, limits)
	if kind == "zip" {
		err = extractZIP(tree, file, int64(len(data)))
	} else {
		err = extractTAR(tree, file)
	}
	return root, err
}

func TestArchivePathsAndCollisions(t *testing.T) {
	bad := []string{"../bad", "/bad", "a/../bad", "a\\b", "a/b\\c", "a\\b/c", "a//b", "./a", "C:bad", "a.", "a ", "a\x00b", "a\nb", "CON", "con.txt", "LPT1.txt", "COM¹.txt", strings.Repeat("a/", 32) + "b", string([]byte{255})}
	for _, name := range bad {
		t.Run(name, func(t *testing.T) {
			tree := newArchiveTree(context.Background(), archiveTestRoot(t), archiveLimits{1024, 100})
			if _, err := tree.claim(name, false); err == nil {
				t.Fatalf("接受危险路径 %q", name)
			}
		})
	}
	for _, pair := range [][2]string{{"A/b", "a/c"}, {"é/a", "e\u0301/b"}, {"a", "a/b"}, {"a/b", "a"}, {"a/b", "a/b"}} {
		tree := newArchiveTree(context.Background(), archiveTestRoot(t), archiveLimits{1024, 100})
		if _, err := tree.claim(pair[0], false); err != nil {
			t.Fatal(err)
		}
		if _, err := tree.claim(pair[1], false); err == nil {
			t.Fatalf("接受冲突 %q", pair)
		}
	}
	tree := newArchiveTree(context.Background(), archiveTestRoot(t), archiveLimits{1024, 100})
	if _, err := tree.claim("parent/file", false); err != nil {
		t.Fatal(err)
	}
	if _, err := tree.claim("parent/", true); err != nil {
		t.Fatal(err)
	}
	if _, err := tree.claim("parent/", true); err == nil {
		t.Fatal("接受重复目录")
	}
}

func TestArchiveExtractionBoundsAndIntegrity(t *testing.T) {
	limits := archiveLimits{16, 4}
	for _, kind := range []string{"zip", "tar_gz"} {
		t.Run(kind, func(t *testing.T) {
			makeArchive := testZIP
			if kind != "zip" {
				makeArchive = testTAR
			}
			data := makeArchive(t, []testArchiveEntry{{name: "folder/file", body: "内容"}, {name: "empty/", kind: tar.TypeDir, mode: fs.ModeDir | 0755}})
			root, err := extractTest(t, kind, data, limits, context.Background())
			if err != nil {
				t.Fatal(err)
			}
			body, err := root.ReadFile("folder/file")
			if err != nil || string(body) != "内容" {
				t.Fatalf("内容错误 %q %v", body, err)
			}
			info, err := root.Stat("empty")
			if err != nil || !info.IsDir() {
				t.Fatal("空目录丢失", err)
			}
			for _, entries := range [][]testArchiveEntry{
				{{name: "../escape", body: "bad"}},
				{{name: "a", body: strings.Repeat("x", 17)}},
				{{name: "1"}, {name: "2"}, {name: "3"}, {name: "4"}, {name: "5"}},
				{{name: "same", body: "a"}, {name: "SAME", body: "b"}},
			} {
				if _, err = extractTest(t, kind, makeArchive(t, entries), limits, context.Background()); err == nil {
					t.Fatal("接受恶意归档", entries)
				}
			}
			ctx, cancel := context.WithCancel(context.Background())
			cancel()
			if _, err = extractTest(t, kind, data, limits, ctx); !errors.Is(err, context.Canceled) {
				t.Fatal("取消未传播", err)
			}
		})
	}
	for _, mode := range []fs.FileMode{fs.ModeSymlink | 0777, fs.ModeNamedPipe | 0600, fs.ModeDevice | 0600} {
		if _, err := extractTest(t, "zip", testZIP(t, []testArchiveEntry{{name: "link", body: "target", mode: mode}}), limits, context.Background()); err == nil {
			t.Fatal("ZIP接受特殊文件", mode)
		}
	}
	for _, kind := range []byte{tar.TypeSymlink, tar.TypeLink, tar.TypeFifo, tar.TypeChar} {
		if _, err := extractTest(t, "tar_gz", testTAR(t, []testArchiveEntry{{name: "link", kind: kind}}), limits, context.Background()); err == nil {
			t.Fatal("TAR接受特殊文件", kind)
		}
	}
	data := testZIP(t, []testArchiveEntry{{name: "file", body: "PAYLOAD"}})
	data[bytes.Index(data, []byte("PAYLOAD"))] = 'x'
	if _, err := extractTest(t, "zip", data, limits, context.Background()); err == nil {
		t.Fatal("CRC损坏未拒绝")
	}
	data = testTAR(t, []testArchiveEntry{{name: "file", body: "ok"}})
	data[len(data)-8] ^= 1
	if _, err := extractTest(t, "tar_gz", data, limits, context.Background()); err == nil {
		t.Fatal("gzip校验损坏未拒绝")
	}
	data = testTAR(t, []testArchiveEntry{{name: "file", body: "ok"}})
	data = append(data, testTAR(t, nil)...)
	if _, err := extractTest(t, "tar_gz", data, limits, context.Background()); err == nil {
		t.Fatal("拼接gzip未拒绝")
	}
}

func cacheTestPackage(t *testing.T, root *os.Root, kind string, data []byte) Package {
	t.Helper()
	hash := sha256.Sum256(data)
	suffix := map[string]string{"zip": ".zip", "tar_gz": ".tar.gz", "dmg": ".dmg"}[kind]
	pkg := Package{Name: "kitonyterms-fixture" + suffix, Size: uint64(len(data)), SHA256: hex.EncodeToString(hash[:]), Kind: kind}
	if err := root.WriteFile("package-"+pkg.SHA256, data, 0600); err != nil {
		t.Fatal(err)
	}
	return pkg
}

func TestArchiveVerifiedHandleAndCacheRejection(t *testing.T) {
	root := archiveTestRoot(t)
	data := testZIP(t, []testArchiveEntry{{name: "file", body: "ok"}})
	pkg := cacheTestPackage(t, root, "zip", data)
	file, err := openVerifiedPackage(context.Background(), root, pkg)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	name := "package-" + pkg.SHA256
	if err = root.Rename(name, "old-cache"); err != nil {
		t.Fatal(err)
	}
	if err = root.WriteFile(name, bytes.Repeat([]byte("x"), len(data)), 0600); err != nil {
		t.Fatal(err)
	}
	pinned, err := io.ReadAll(file)
	if err != nil || !bytes.Equal(pinned, data) {
		t.Fatal("丢失已验证句柄", err)
	}
	if _, err = openVerifiedPackage(context.Background(), root, pkg); err == nil {
		t.Fatal("接受损坏缓存")
	}
	if err = root.Remove(name); err != nil {
		t.Fatal(err)
	}
	requireUpdateTestSymlink(t, root.Symlink("old-cache", name))
	if _, err = openVerifiedPackage(context.Background(), root, pkg); err == nil {
		t.Fatal("接受链接缓存")
	}
}

func TestArchiveBinaryPlatformHeaders(t *testing.T) {
	root := archiveTestRoot(t)
	for _, target := range []string{"aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc", "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"} {
		data := make([]byte, 256)
		switch {
		case strings.Contains(target, "apple"):
			binary.LittleEndian.PutUint32(data[:4], 0xfeedfacf)
			binary.LittleEndian.PutUint32(data[4:8], 0x0100000c)
			if strings.HasPrefix(target, "x86_64") {
				binary.LittleEndian.PutUint32(data[4:8], 0x01000007)
			}
			binary.LittleEndian.PutUint32(data[12:16], 2)
		case strings.Contains(target, "linux"):
			copy(data, "\x7fELF")
			data[4], data[5], data[6] = 2, 1, 1
			binary.LittleEndian.PutUint16(data[16:18], 2)
			machine := uint16(62)
			if strings.HasPrefix(target, "aarch64") {
				machine = 183
			}
			binary.LittleEndian.PutUint16(data[18:20], machine)
		default:
			copy(data, "MZ")
			binary.LittleEndian.PutUint32(data[60:64], 64)
			copy(data[64:], "PE\x00\x00")
			binary.LittleEndian.PutUint16(data[68:70], 0x8664)
			if strings.HasPrefix(target, "aarch64") {
				binary.LittleEndian.PutUint16(data[68:70], 0xaa64)
			}
			binary.LittleEndian.PutUint16(data[84:86], 112)
			binary.LittleEndian.PutUint16(data[86:88], 2)
			binary.LittleEndian.PutUint16(data[88:90], 0x20b)
		}
		if err := root.WriteFile("binary", data, 0700); err != nil {
			t.Fatal(err)
		}
		if err := validateBinary(root, "binary", target); err != nil {
			t.Fatal(target, err)
		}
		data[0] ^= 1
		if err := root.WriteFile("binary", data, 0700); err != nil {
			t.Fatal(err)
		}
		if err := validateBinary(root, "binary", target); err == nil {
			t.Fatal("接受损坏平台头", target)
		}
	}
}

func TestUpdateToolBoundsAndBuildIdentity(t *testing.T) {
	t.Setenv("KITONYTERMS_UPDATE_TEST_HELPER", "1")
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	if err = verifyBuildCommand(context.Background(), executable, archiveTestBuild()); err != nil {
		t.Fatal(err)
	}
	wrong := archiveTestBuild()
	wrong.Build++
	if err = verifyBuildCommand(context.Background(), executable, wrong); err == nil {
		t.Fatal("接受不同构建身份")
	}
	if _, err = runUpdateTool(context.Background(), 5*time.Second, 64, executable, "--overflow"); err == nil || !strings.Contains(err.Error(), "超过上限") {
		t.Fatal("输出上限未生效", err)
	}
	started := time.Now()
	if _, err = runUpdateTool(context.Background(), 50*time.Millisecond, 64, executable, "--wait"); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatal("超时未传播", err)
	}
	if time.Since(started) > 3*time.Second {
		t.Fatal("子进程未及时回收")
	}
}
