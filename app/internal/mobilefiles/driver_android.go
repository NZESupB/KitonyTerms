//go:build android && cgo

package mobilefiles

/*
#include <jni.h>
#include <stdlib.h>
int KTFilesReady(void);
int KTFilesStart(const char *, int);
int KTFilesCancel(const char *);
const char *KTFilesString(JNIEnv *, jstring);
void KTFilesStringRelease(JNIEnv *, jstring, const char *);
void KTFilesInitialize(JNIEnv *, jobject);
*/
import "C"

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sync"
	"unsafe"

	"golang.org/x/sys/unix"
)

type nativeRequest struct {
	done     chan int
	progress Progress
}

var nativeState = struct {
	sync.Mutex
	pending map[string]*nativeRequest
	owners  map[string]context.CancelFunc
}{pending: map[string]*nativeRequest{}, owners: map[string]context.CancelFunc{}}

type androidDriver struct{}

func platformDriver() Driver          { return androidDriver{} }
func (androidDriver) Available() bool { return C.KTFilesReady() != 0 }
func (androidDriver) Invalidate(id string, cancel context.CancelFunc) {
	nativeState.Lock()
	defer nativeState.Unlock()
	if cancel == nil {
		delete(nativeState.owners, id)
	} else {
		nativeState.owners[id] = cancel
	}
}
func (androidDriver) Run(ctx context.Context, request Request, progress Progress) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	payload, err := json.Marshal(request)
	if err != nil {
		return err
	}
	r := &nativeRequest{done: make(chan int, 1), progress: progress}
	nativeState.Lock()
	nativeState.pending[request.ID] = r
	nativeState.Unlock()
	defer func() { nativeState.Lock(); delete(nativeState.pending, request.ID); nativeState.Unlock() }()
	bytes := C.CBytes(payload)
	started := C.KTFilesStart((*C.char)(bytes), C.int(len(payload)))
	C.free(bytes)
	if started == 0 {
		return ErrUnavailable
	}
	var code int
	select {
	case code = <-r.done:
	case <-ctx.Done():
		id := C.CString(request.ID)
		C.KTFilesCancel(id)
		C.free(unsafe.Pointer(id))
		// 必须等待原生线程真正关闭描述符和清理未完成文档。
		code = <-r.done
	}
	err = nativeError(code)
	if ctx.Err() != nil {
		err = errors.Join(ctx.Err(), err)
	}
	return err
}
func nativeError(code int) error {
	switch code {
	case 0:
		return nil
	case 1:
		return errors.Join(context.Canceled, ErrCancelled)
	case 2:
		return ErrUnavailable
	case 3:
		return fmt.Errorf("请先完成当前文件选择或导出")
	case 4:
		return fmt.Errorf("所选文件类型、名称或目录无效")
	case 5:
		return fmt.Errorf("移动文件超过 1GiB、50000 项或 64 层限制")
	case 6:
		return fmt.Errorf("导出目标已存在或名称改变，未覆盖")
	case 8:
		return fmt.Errorf("文件导出未完成，原生文档清理失败；已完成外部文件保留")
	default:
		return fmt.Errorf("原生文件读取或导出失败；已完成外部文件保留")
	}
}
func jniString(env *C.JNIEnv, value C.jstring) string {
	text := C.KTFilesString(env, value)
	if text == nil {
		return ""
	}
	defer C.KTFilesStringRelease(env, value, text)
	return C.GoString(text)
}

//export Java_com_wails_app_FileExchange_nativeInitialize
func Java_com_wails_app_FileExchange_nativeInitialize(env *C.JNIEnv, _ C.jclass, object C.jobject) {
	C.KTFilesInitialize(env, object)
}

//export Java_com_wails_app_FileExchange_nativeComplete
func Java_com_wails_app_FileExchange_nativeComplete(env *C.JNIEnv, _ C.jclass, value C.jstring, code C.jint) {
	id := jniString(env, value)
	nativeState.Lock()
	r := nativeState.pending[id]
	if r != nil {
		select {
		case r.done <- int(code):
		default:
		}
	}
	nativeState.Unlock()
}

//export Java_com_wails_app_FileExchange_nativeProgress
func Java_com_wails_app_FileExchange_nativeProgress(env *C.JNIEnv, _ C.jclass, value C.jstring, bytes C.jlong, files C.jint) {
	id := jniString(env, value)
	nativeState.Lock()
	defer nativeState.Unlock()
	if r := nativeState.pending[id]; r != nil && r.progress != nil && bytes >= 0 && files >= 0 {
		r.progress(uint64(bytes), int(files))
	}
}

//export Java_com_wails_app_FileExchange_nativeInvalidate
func Java_com_wails_app_FileExchange_nativeInvalidate(env *C.JNIEnv, _ C.jclass, value C.jstring) {
	id := jniString(env, value)
	nativeState.Lock()
	cancel := nativeState.owners[id]
	nativeState.Unlock()
	if cancel != nil {
		cancel()
	}
}

//export Java_com_wails_app_FileExchange_nativeNonblocking
func Java_com_wails_app_FileExchange_nativeNonblocking(env *C.JNIEnv, clazz C.jclass, fd C.jint) C.jboolean {
	if err := unix.SetNonblock(int(fd), true); err != nil {
		return 0
	}
	return 1
}
