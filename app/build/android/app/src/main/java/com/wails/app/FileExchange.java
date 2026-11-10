package com.wails.app;

import android.app.Activity;
import android.content.ClipData;
import android.content.ContentResolver;
import android.content.Context;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.CancellationSignal;
import android.os.Handler;
import android.os.Looper;
import android.os.ParcelFileDescriptor;
import android.os.SystemClock;
import android.provider.DocumentsContract;
import android.system.Os;
import android.system.OsConstants;
import android.system.ErrnoException;
import android.system.StructPollfd;
import android.system.StructStat;

import org.json.JSONObject;
import java.io.FileDescriptor;
import java.security.MessageDigest;
import java.util.Arrays;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.lang.ref.WeakReference;
import java.nio.charset.StandardCharsets;
import java.text.Normalizer;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicBoolean;

/** Application 级 SAF 适配；URI 仅存于本对象，不进入 JS、配置或日志。 */
final class FileExchange {
    private static final long MAX_BYTES = 1L << 30;
    private static final int MAX_NODES = 50000;
    private static final int MAX_DEPTH = 64;
    private static final String DIRECTORY = DocumentsContract.Document.MIME_TYPE_DIR;
    private static final String[] COLUMNS = {
        DocumentsContract.Document.COLUMN_DOCUMENT_ID,
        DocumentsContract.Document.COLUMN_DISPLAY_NAME,
        DocumentsContract.Document.COLUMN_MIME_TYPE,
        DocumentsContract.Document.COLUMN_FLAGS,
        DocumentsContract.Document.COLUMN_SIZE
    };
    private final Context context;
    private final ContentResolver resolver;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private WeakReference<Activity> owner = new WeakReference<>(null);
    private final Map<String, Capability> capabilities = new HashMap<>();
    private volatile Work active;
    private int nextCode = 20000;

    private static native void nativeInitialize(FileExchange exchange);
    private static native void nativeComplete(String id, int code);
    private static native void nativeProgress(String id, long bytes, int files);
    private static native void nativeInvalidate(String stageID);
    private static native boolean nativeNonblocking(int descriptor);

    private static final class Failure extends Exception {
        final int code;
        Failure(int code) { this.code = code; }
    }
    private static final class Capability {
        final WeakReference<Activity> owner;
        final Uri uri;
        final boolean single;
        volatile boolean completed;
        volatile boolean removed;
        Capability(Activity owner, Uri uri, boolean single) {
            this.owner = new WeakReference<>(owner);
            this.uri = uri;
            this.single = single;
        }
    }
    private static final class Work {
        final String id, stageID, action, name;
        final File stage;
        final WeakReference<Activity> owner;
        final CancellationSignal signal = new CancellationSignal();
        final AtomicBoolean cancelled = new AtomicBoolean();
        final AtomicBoolean finished = new AtomicBoolean();
        final List<ParcelFileDescriptor> handles = new ArrayList<>();
        int pickerCode = -1;
        boolean copying;
        boolean closeFailed;
        long bytes, lastProgress;
        int files, nodes;
        int queriedNodes;
        final Map<String, Set<String>> exportNames = new HashMap<>();
        final Map<String, Set<String>> exportIDs = new HashMap<>();
        Thread cancellation;
        Uri incomplete;
        Capability exportCapability;
        Work(JSONObject json, Activity owner) throws Exception {
            id = json.getString("id"); stageID = json.getString("stageID");
            action = json.getString("action"); name = json.optString("name");
            stage = new File(json.getString("stage"));
            this.owner = new WeakReference<>(owner);
            if (!id.matches("[0-9a-f]{32}") || !stageID.matches("[0-9a-f]{32}")) throw new Failure(4);
        }
        void check() throws Failure { if (cancelled.get()) throw new Failure(1); }
        synchronized void hold(ParcelFileDescriptor descriptor) throws Exception {
            if (descriptor == null) throw new Failure(7);
            if (cancelled.get()) { descriptor.close(); throw new Failure(1); }
            handles.add(descriptor);
        }
        synchronized void drop(ParcelFileDescriptor descriptor) { handles.remove(descriptor); }
        synchronized void cancel() {
            cancelled.set(true);
            if (cancellation != null) return;
            cancellation = new Thread(() -> {
                signal.cancel();
                synchronized (this) {
                    for (ParcelFileDescriptor descriptor : handles) {
                        try { descriptor.close(); } catch (Exception error) { closeFailed = true; }
                    }
                }
            }, "file-cancel");
            cancellation.start();
        }
        void awaitCancellation() throws InterruptedException {
            Thread thread;
            synchronized (this) { thread = cancellation; }
            if (thread != null) thread.join();
        }
    }
    private static final class Doc {
        final String id, name, mime;
        final long size;
        final int flags;
        Doc(Cursor cursor) throws Exception {
            id = cursor.getString(0); name = cursor.getString(1); mime = cursor.getString(2);
            flags = cursor.getInt(3); size = cursor.isNull(4) ? -1 : cursor.getLong(4);
            if (id == null || id.length() > 4096 || mime == null || mime.length() > 256 || !safeName(name)
                || (flags & DocumentsContract.Document.FLAG_VIRTUAL_DOCUMENT) != 0) throw new Failure(4);
        }
        boolean directory() { return DIRECTORY.equals(mime); }
    }

    FileExchange(Context context) {
        this.context = context.getApplicationContext();
        resolver = this.context.getContentResolver();
        nativeInitialize(this);
    }
    void attach(Activity activity) { owner = new WeakReference<>(activity); }
    void detach(Activity activity) {
        if (owner.get() != activity) return;
        owner.clear();
        Work current = active;
        if (current != null && current.owner.get() == activity) cancel(current.id);
        for (Map.Entry<String, Capability> entry : new ArrayList<>(capabilities.entrySet())) {
            if (entry.getValue().owner.get() == activity) {
                entry.getValue().owner.clear();
                nativeInvalidate(entry.getKey());
            }
        }
    }

    // JNI 使用 UTF-8 byte[]，不会把非 BMP 文件名变为 Modified UTF-8。
    public void start(byte[] payload) {
        final JSONObject json;
        try { json = new JSONObject(new String(payload, StandardCharsets.UTF_8)); }
        catch (Exception error) { throw new IllegalArgumentException("文件请求结构无效"); }
        main.post(() -> begin(json));
    }
    private void begin(JSONObject json) {
        Work request;
        try { request = new Work(json, owner.get()); }
        catch (Exception error) { nativeComplete(json.optString("id"), 4); return; }
        if ("release".equals(request.action)) {
            Capability capability = capabilities.remove(request.stageID);
            worker.execute(() -> {
                int code = 0;
                if (capability != null && capability.single && !capability.completed && !capability.removed) {
                    try { remove(capability.uri); } catch (Exception error) { code = 8; }
                }
                nativeComplete(request.id, code);
            });
            return;
        }
        if (active != null) { nativeComplete(request.id, 3); return; }
        Activity activity = request.owner.get();
        if (activity == null || activity.isDestroyed()) { nativeComplete(request.id, 2); return; }
        try { validateStage(request); }
        catch (Exception error) { nativeComplete(request.id, 4); return; }
        active = request;
        if ("export".equals(request.action)) {
            Capability capability = capabilities.get(request.stageID);
            if (capability == null || capability.uri == null || capability.owner.get() != activity) { finish(request, 2); return; }
            request.copying = true;
            worker.execute(() -> perform(request, () -> export(request, capability)));
            return;
        }
        if (capabilities.containsKey(request.stageID)) { finish(request, 4); return; }
        if (nextCode > 65535) { finish(request, 2); return; }
        Intent intent;
        switch (request.action) {
            case "import_files":
                intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                intent.addCategory(Intent.CATEGORY_OPENABLE);
                intent.setType("*/*"); intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
                intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
                break;
            case "import_tree": case "select_tree":
                intent = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
                intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
                if ("select_tree".equals(request.action)) intent.addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
                break;
            case "select_file":
                if (!safeName(request.name)) { finish(request, 4); return; }
                intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
                intent.addCategory(Intent.CATEGORY_OPENABLE);
                intent.setType("application/octet-stream"); intent.putExtra(Intent.EXTRA_TITLE, request.name);
                intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
                break;
            default: finish(request, 4); return;
        }
        request.pickerCode = nextCode++;
        try { activity.startActivityForResult(intent, request.pickerCode); }
        catch (Exception error) { finish(request, 2); }
    }
    boolean onResult(Activity activity, int code, int result, Intent data) {
        if (code < 20000 || code > 65535) return false;
        Work request = active;
        if (request == null || request.owner.get() != activity || request.pickerCode != code) return true;
        request.pickerCode = -1;
        if (result != Activity.RESULT_OK || data == null) { finish(request, 1); return true; }
        List<Uri> uris = new ArrayList<>();
        ClipData clip = data.getClipData();
        if (clip != null) {
            if (clip.getItemCount() > 1024) { finish(request, 5); return true; }
            for (int i = 0; i < clip.getItemCount(); i++) uris.add(clip.getItemAt(i).getUri());
        } else if (data.getData() != null) uris.add(data.getData());
        if (uris.isEmpty() || (!"import_files".equals(request.action) && uris.size() != 1)) { finish(request, 4); return true; }
        for (Uri uri : uris) if (!contentUri(uri)) { finish(request, 4); return true; }
        request.copying = true;
        worker.execute(() -> perform(request, () -> {
            Uri selected = uris.get(0);
            boolean single = "select_file".equals(request.action);
            if ("import_files".equals(request.action)) {
                Set<String> names = new HashSet<>(), documents = new HashSet<>();
                for (Uri uri : uris) importDocument(request, uri, null, request.stage, 0, names, documents);
            } else if ("import_tree".equals(request.action)) {
                Uri root = treeRoot(selected);
                importDocument(request, root, selected, request.stage, 0, new HashSet<>(), new HashSet<>());
            } else {
                Uri document = single ? selected : treeRoot(selected);
                Doc doc = document(request, document);
                if ((!single && !doc.directory()) || (single && (doc.directory() || doc.size != 0))
                    || (single && (doc.flags & DocumentsContract.Document.FLAG_SUPPORTS_WRITE) == 0)
                    || (!single && (doc.flags & DocumentsContract.Document.FLAG_DIR_SUPPORTS_CREATE) == 0)) throw new Failure(4);
                if (single) {
                    request.incomplete = selected;
                    if (!doc.name.equals(request.name)) throw new Failure(6);
                }
            }
            request.check();
            final Capability capability = new Capability(activity, "import_files".equals(request.action) || "import_tree".equals(request.action) ? null : selected, single);
            // 主线程检查归属后发布能力，销毁中的选择不会变成新页面授权。
            main.post(() -> {
                if (owner.get() == activity && !activity.isDestroyed() && !request.cancelled.get() && !request.finished.get()) {
                    capabilities.put(request.stageID, capability);
                    request.incomplete = null;
                    finish(request, 0);
                } else {
                    request.cancel();
                    worker.execute(() -> perform(request, () -> { throw new Failure(1); }));
                }
            });
        }, false));
        return true;
    }
    public void cancel(String id) {
        main.post(() -> {
            Work request = active;
            if (request == null || !request.id.equals(id)) return;
            request.cancelled.set(true);
            if (request.pickerCode != -1) {
                int code = request.pickerCode; request.pickerCode = -1;
                Activity activity = request.owner.get();
                if (activity != null) activity.finishActivity(code);
                finish(request, 1);
            } else if (request.copying) request.cancel();
        });
    }
    private interface Operation { void run() throws Exception; }
    private void perform(Work request, Operation operation) { perform(request, operation, true); }
    private void perform(Work request, Operation operation, boolean complete) {
        int code = 0;
        try { request.check(); operation.run(); if (complete) request.check(); }
        catch (Failure error) { code = error.code; }
        catch (Exception error) { code = request.cancelled.get() ? 1 : 7; }
        if (code != 0 || complete) {
            try { request.awaitCancellation(); }
            catch (InterruptedException error) { Thread.currentThread().interrupt(); code = 8; }
            if (request.incomplete != null) {
                try {
                    remove(request.incomplete);
                    request.incomplete = null;
                    if (request.exportCapability != null && request.exportCapability.single) request.exportCapability.removed = true;
                }
                catch (Exception error) { code = 8; }
            }
            if (request.closeFailed) code = 8;
            finish(request, code);
        }
    }
    private void finish(Work request, int code) {
        Runnable completion = () -> {
            if (!request.finished.compareAndSet(false, true)) return;
            if (active == request) active = null;
            nativeComplete(request.id, code);
        };
        if (Looper.myLooper() == Looper.getMainLooper()) completion.run();
        else main.post(completion);
    }
    private static boolean contentUri(Uri uri) {
        return uri != null && "content".equals(uri.getScheme()) && uri.getAuthority() != null && uri.getAuthority().length() <= 256;
    }
    private static Uri treeRoot(Uri tree) throws Exception {
        if (!DocumentsContract.isTreeUri(tree)) throw new Failure(4);
        return DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree));
    }
    private static boolean safeName(String name) {
        if (name == null || name.isEmpty() || name.equals(".") || name.equals("..") || name.getBytes(StandardCharsets.UTF_8).length > 255
            || name.endsWith(".") || name.endsWith(" ")) return false;
        for (int offset = 0; offset < name.length();) {
            int cp = name.codePointAt(offset); offset += Character.charCount(cp);
            if (Character.isISOControl(cp) || "<>:\"/\\|?*".indexOf(cp) >= 0 || (cp >= 0xd800 && cp <= 0xdfff)) return false;
        }
        String stem = name.split("\\.", 2)[0].toUpperCase(Locale.ROOT);
        return !stem.matches("CON|PRN|AUX|NUL|CONIN\\$|CONOUT\\$|(?:COM|LPT)[1-9¹²³]");
    }
    private static String key(String name) { return Normalizer.normalize(name, Normalizer.Form.NFC).toLowerCase(Locale.ROOT); }
    private void validateStage(Work request) throws Exception {
        File root = new File(context.getFilesDir(), "data/mobile-files").getCanonicalFile();
        File expected = new File(root, request.stageID);
        if (!request.stage.getAbsoluteFile().equals(expected) || !request.stage.getCanonicalFile().equals(expected)
            || !OsConstants.S_ISDIR(Os.lstat(expected.getPath()).st_mode)) throw new Failure(4);
    }
    private Doc document(Work request, Uri uri) throws Exception {
        request.check();
        if (!contentUri(uri) || !DocumentsContract.isDocumentUri(context, uri)) throw new Failure(4);
        try (Cursor cursor = resolver.query(uri, COLUMNS, null, null, null, request.signal)) {
            if (cursor == null || !cursor.moveToFirst()) throw new Failure(7);
            Doc doc = new Doc(cursor);
            if (cursor.moveToNext()) throw new Failure(4);
            return doc;
        }
    }
    private List<Doc> children(Work request, Uri tree, String id) throws Exception {
        request.check();
        Uri uri = DocumentsContract.buildChildDocumentsUriUsingTree(tree, id);
        List<Doc> result = new ArrayList<>();
        Set<String> names = new HashSet<>();
        try (Cursor cursor = resolver.query(uri, COLUMNS, null, null, null, request.signal)) {
            if (cursor == null) throw new Failure(7);
            while (cursor.moveToNext()) {
                request.check();
                if (++request.queriedNodes > MAX_NODES) throw new Failure(5);
                Doc doc = new Doc(cursor);
                if (!names.add(key(doc.name))) throw new Failure(6);
                result.add(doc);
            }
        }
        return result;
    }
    private void importDocument(Work request, Uri uri, Uri tree, File parent, int depth, Set<String> names, Set<String> documents) throws Exception {
        request.check();
        if (depth > MAX_DEPTH || ++request.nodes > MAX_NODES) throw new Failure(5);
        Doc doc = document(request, uri);
        if (!names.add(key(doc.name)) || !documents.add(uri.getAuthority() + "/" + doc.id)) throw new Failure(4);
        File target = new File(parent, doc.name);
        if (!target.getCanonicalFile().equals(target.getAbsoluteFile())) throw new Failure(4);
        if (doc.directory()) {
            if (tree == null) throw new Failure(4);
            Os.mkdir(target.getPath(), 0700);
            Set<String> childNames = new HashSet<>();
            for (Doc child : children(request, tree, doc.id)) {
                importDocument(request, DocumentsContract.buildDocumentUriUsingTree(tree, child.id), tree, target, depth+1, childNames, documents);
            }
            return;
        }
        if (doc.size > MAX_BYTES - request.bytes) throw new Failure(5);
        ParcelFileDescriptor descriptor = resolver.openFileDescriptor(uri, "r", request.signal);
        request.hold(descriptor);
        try (ParcelFileDescriptor handle = descriptor;
             FileOutputStream destination = new FileOutputStream(Os.open(target.getPath(), OsConstants.O_WRONLY | OsConstants.O_CREAT | OsConstants.O_EXCL | OsConstants.O_NOFOLLOW, 0600))) {
            long before = request.bytes;
            copy(request, handle.getFileDescriptor(), destination.getFD());
            if (handle.canDetectErrors()) handle.checkError();
            if (doc.size >= 0 && request.bytes - before != doc.size) throw new Failure(7);
            destination.getFD().sync();
        } finally { request.drop(descriptor); }
        request.files++;
    }
    private static void nonblocking(FileDescriptor fd) throws Exception {
        int mode = Os.fstat(fd).st_mode;
        if (!OsConstants.S_ISREG(mode) && !OsConstants.S_ISFIFO(mode)) throw new Failure(4);
        try (ParcelFileDescriptor duplicate = ParcelFileDescriptor.dup(fd)) {
            if (!nativeNonblocking(duplicate.getFd())) throw new Failure(7);
        }
    }
    private static void ready(Work request, FileDescriptor fd, short events) throws Exception {
        StructPollfd poll = new StructPollfd(); poll.fd = fd; poll.events = events;
        while (true) {
            request.check();
            try { if (Os.poll(new StructPollfd[]{poll}, 100) > 0) return; }
            catch (ErrnoException error) { if (error.errno != OsConstants.EINTR) throw error; }
        }
    }
    private static int read(Work request, FileDescriptor fd, byte[] buffer) throws Exception {
        while (true) {
            ready(request, fd, (short)OsConstants.POLLIN);
            try { return Os.read(fd, buffer, 0, buffer.length); }
            catch (ErrnoException error) { if (error.errno != OsConstants.EAGAIN && error.errno != OsConstants.EINTR) throw error; }
        }
    }
    private void copy(Work request, FileDescriptor source, FileDescriptor destination) throws Exception {
        nonblocking(source); nonblocking(destination);
        byte[] buffer = new byte[64 * 1024];
        while (true) {
            int count = read(request, source, buffer);
            if (count == 0) return;
            if (count > MAX_BYTES - request.bytes) throw new Failure(5);
            int offset = 0;
            while (offset < count) {
                ready(request, destination, (short)OsConstants.POLLOUT);
                try {
                    int written = Os.write(destination, buffer, offset, count - offset);
                    if (written <= 0) throw new Failure(7);
                    offset += written; request.bytes += written;
                } catch (ErrnoException error) { if (error.errno != OsConstants.EAGAIN && error.errno != OsConstants.EINTR) throw error; }
            }
            long now = SystemClock.uptimeMillis();
            if (now - request.lastProgress >= 100) {
                request.lastProgress = now;
                nativeProgress(request.id, request.bytes, request.files);
            }
        }
    }
    private static File[] list(File directory) throws Exception {
        File[] children = directory.listFiles();
        if (children == null) throw new Failure(7);
        if (children.length > MAX_NODES) throw new Failure(5);
        return children;
    }
    private void export(Work request, Capability capability) throws Exception {
        request.exportCapability = capability;
        File[] roots = list(request.stage);
        if (roots.length == 0 || roots.length > 1024) throw new Failure(5);
        if (capability.single) {
            if (roots.length != 1 || !OsConstants.S_ISREG(Os.lstat(roots[0].getPath()).st_mode)) throw new Failure(4);
            request.incomplete = capability.uri;
            writeDocument(request, roots[0], capability.uri);
            capability.completed = true;
            request.incomplete = null;
            return;
        }
        Uri tree = capability.uri;
        Uri parent = treeRoot(tree);
        Doc parentDoc = document(request, parent);
        Set<String> names = targetNames(request, tree, parentDoc.id);
        Set<String> rootsSeen = new HashSet<>();
        for (File root : roots) if (!safeName(root.getName()) || names.contains(key(root.getName())) || !rootsSeen.add(key(root.getName()))) throw new Failure(6);
        for (File root : roots) exportNode(request, tree, parent, root, 0);
        capability.completed = true;
    }
    private Set<String> targetNames(Work request, Uri tree, String parentID) throws Exception {
        Set<String> names = request.exportNames.get(parentID);
        if (names == null) {
            names = new HashSet<>();
            Set<String> ids = new HashSet<>();
            for (Doc doc : children(request, tree, parentID)) { names.add(key(doc.name)); ids.add(doc.id); }
            request.exportNames.put(parentID, names); request.exportIDs.put(parentID, ids);
        }
        return names;
    }
    private void exportNode(Work request, Uri tree, Uri parent, File local, int depth) throws Exception {
        request.check();
        if (depth > MAX_DEPTH || ++request.nodes > MAX_NODES || !safeName(local.getName())
            || !local.getCanonicalFile().equals(local.getAbsoluteFile())) throw new Failure(4);
        StructStat info = Os.lstat(local.getPath());
        boolean directory = OsConstants.S_ISDIR(info.st_mode);
        if (!directory && !OsConstants.S_ISREG(info.st_mode)) throw new Failure(4);
        String parentID = DocumentsContract.getDocumentId(parent);
        if (!targetNames(request, tree, parentID).add(key(local.getName()))) throw new Failure(6);
        Uri target = DocumentsContract.createDocument(resolver, parent, directory ? DIRECTORY : "application/octet-stream", local.getName());
        if (target == null || !contentUri(target) || !tree.getAuthority().equals(target.getAuthority())
            || request.exportIDs.get(parentID).contains(DocumentsContract.getDocumentId(target))
            || !DocumentsContract.isTreeUri(target)
            || !DocumentsContract.getTreeDocumentId(tree).equals(DocumentsContract.getTreeDocumentId(target))) throw new Failure(7);
        request.exportIDs.get(parentID).add(DocumentsContract.getDocumentId(target));
        request.incomplete = target;
        Doc created = document(request, target);
        if (!created.name.equals(local.getName()) || created.directory() != directory) throw new Failure(6);
        if (directory) {
            request.incomplete = null; // 保留已创建的目录及其中已完成文件，不递归删除外部数据。
            for (File child : list(local)) exportNode(request, tree, target, child, depth+1);
        } else {
            writeDocument(request, local, target);
            request.incomplete = null;
        }
    }
    private void writeDocument(Work request, File local, Uri target) throws Exception {
        java.io.FileDescriptor localFD = Os.open(local.getPath(), OsConstants.O_RDONLY | OsConstants.O_NOFOLLOW, 0);
        try (FileInputStream source = new FileInputStream(localFD)) {
            StructStat before = Os.fstat(localFD);
            if (!OsConstants.S_ISREG(before.st_mode) || before.st_size > MAX_BYTES - request.bytes) throw new Failure(5);
            ParcelFileDescriptor descriptor = resolver.openFileDescriptor(target, "w", request.signal);
            request.hold(descriptor);
            long previous = request.bytes;
            try (ParcelFileDescriptor handle = descriptor) {
                copy(request, source.getFD(), handle.getFileDescriptor());
                if (OsConstants.S_ISREG(Os.fstat(handle.getFileDescriptor()).st_mode)) Os.fsync(handle.getFileDescriptor());
                if (handle.canDetectErrors()) handle.checkError();
            } finally { request.drop(descriptor); }
            StructStat after = Os.fstat(localFD);
            if (request.bytes - previous != before.st_size || after.st_size != before.st_size || after.st_mtime != before.st_mtime) throw new Failure(7);
            verifyDocument(request, localFD, target, before.st_size);
        }
        request.check();
        request.files++;
        nativeProgress(request.id, request.bytes, request.files);
    }
    private void verifyDocument(Work request, FileDescriptor local, Uri target, long expected) throws Exception {
        MessageDigest original = MessageDigest.getInstance("SHA-256"), exported = MessageDigest.getInstance("SHA-256");
        Os.lseek(local, 0, OsConstants.SEEK_SET);
        byte[] buffer = new byte[64 * 1024];
        long count = 0;
        while (true) {
            int n = read(request, local, buffer); if (n == 0) break;
            count += n; if (count > expected) throw new Failure(7); original.update(buffer, 0, n);
        }
        if (count != expected) throw new Failure(7);
        ParcelFileDescriptor descriptor = resolver.openFileDescriptor(target, "r", request.signal);
        request.hold(descriptor);
        try (ParcelFileDescriptor handle = descriptor) {
            nonblocking(handle.getFileDescriptor()); count = 0;
            while (true) {
                int n = read(request, handle.getFileDescriptor(), buffer); if (n == 0) break;
                count += n; if (count > expected) throw new Failure(7); exported.update(buffer, 0, n);
            }
            if (handle.canDetectErrors()) handle.checkError();
            if (count != expected || !Arrays.equals(original.digest(), exported.digest())) throw new Failure(7);
        } finally { request.drop(descriptor); }
    }
    private void remove(Uri uri) throws Exception {
        if (!DocumentsContract.deleteDocument(resolver, uri)) throw new Failure(8);
    }
}
