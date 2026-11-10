//go:build android && cgo

#include <jni.h>
#include <pthread.h>
#include <string.h>

// 独立于 Wails 的 static JVM/bridge，只持有 Application 级文件适配对象。
static JavaVM *kt_files_vm;
static jobject kt_files_object;
static jmethodID kt_files_start, kt_files_cancel;
static pthread_mutex_t kt_files_mutex = PTHREAD_MUTEX_INITIALIZER;

void KTFilesInitialize(JNIEnv *env, jobject object) {
    pthread_mutex_lock(&kt_files_mutex);
    if (!kt_files_object) {
        (*env)->GetJavaVM(env, &kt_files_vm);
        kt_files_object = (*env)->NewGlobalRef(env, object);
        jclass cls = (*env)->GetObjectClass(env, object);
        kt_files_start = (*env)->GetMethodID(env, cls, "start", "([B)V");
        kt_files_cancel = (*env)->GetMethodID(env, cls, "cancel", "(Ljava/lang/String;)V");
        (*env)->DeleteLocalRef(env, cls);
    }
    pthread_mutex_unlock(&kt_files_mutex);
}
int KTFilesReady(void) {
    pthread_mutex_lock(&kt_files_mutex);
    int ready = kt_files_object && kt_files_start && kt_files_cancel;
    pthread_mutex_unlock(&kt_files_mutex);
    return ready;
}
static JNIEnv *kt_env(int *attached) {
    JNIEnv *env = NULL;
    *attached = 0;
    if ((*kt_files_vm)->GetEnv(kt_files_vm, (void **)&env, JNI_VERSION_1_6) == JNI_EDETACHED) {
        if ((*kt_files_vm)->AttachCurrentThread(kt_files_vm, &env, NULL) != JNI_OK) return NULL;
        *attached = 1;
    }
    return env;
}
int KTFilesStart(const char *bytes, int length) {
    if (!KTFilesReady()) return 0;
    int attached;
    JNIEnv *env = kt_env(&attached);
    if (!env) return 0;
    jbyteArray payload = (*env)->NewByteArray(env, length);
    if (payload) {
        (*env)->SetByteArrayRegion(env, payload, 0, length, (const jbyte *)bytes);
        (*env)->CallVoidMethod(env, kt_files_object, kt_files_start, payload);
        (*env)->DeleteLocalRef(env, payload);
    }
    int ok = !(*env)->ExceptionCheck(env) && payload != NULL;
    if (!ok) (*env)->ExceptionClear(env);
    if (attached) (*kt_files_vm)->DetachCurrentThread(kt_files_vm);
    return ok;
}
int KTFilesCancel(const char *id) {
    if (!KTFilesReady()) return 0;
    int attached;
    JNIEnv *env = kt_env(&attached);
    if (!env) return 0;
    jstring value = (*env)->NewStringUTF(env, id);
    (*env)->CallVoidMethod(env, kt_files_object, kt_files_cancel, value);
    (*env)->DeleteLocalRef(env, value);
    int ok = !(*env)->ExceptionCheck(env);
    if (!ok) (*env)->ExceptionClear(env);
    if (attached) (*kt_files_vm)->DetachCurrentThread(kt_files_vm);
    return ok;
}
const char *KTFilesString(JNIEnv *env, jstring value) { return value ? (*env)->GetStringUTFChars(env, value, NULL) : NULL; }
void KTFilesStringRelease(JNIEnv *env, jstring value, const char *text) { (*env)->ReleaseStringUTFChars(env, value, text); }
