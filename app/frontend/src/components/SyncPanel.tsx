import { useEffect, useRef, useState } from "react";
import { Backend, errorText, useApp } from "../lib/backend";
import type {
  RemoteConfig,
  ShareInfo,
} from "../../bindings/kitonyterms/internal/syncconfig/models";
import { Scanner } from "./Scanner";
import { useText } from "../lib/useText";
import { useProtectedState } from "../lib/useProtectedState";

type Request<T> = Promise<T> & { cancel: () => Promise<unknown> };
export function SyncPanel() {
  const t = useText();
  const [url, setURL] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [remote, setRemote] = useState<RemoteConfig | null>(null);
  const [etag, setETag] = useState("");
  const [createOnly, setCreateOnly] = useState(true);
  const [address, setAddress] = useState("");
  const [code, setCode] = useState("");
  const [bindIP, setBindIP] = useState("");
  const [share, setShare] = useProtectedState<ShareInfo | null>(null);
  const [scanning, setScanning] = useProtectedState(false);
  const [qr, setQR] = useState("");
  const [busy, setBusy] = useProtectedState(false);
  const [message, setMessage] = useState("");
  const current = useRef<{ cancel: () => Promise<unknown> }>();
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    const refresh = () => {
      void Backend.GetShare()
        .then((info) => {
          if (mounted.current) setShare(info);
        })
        .catch(useApp.getState().report);
    };
    refresh();
    const timer = setInterval(refresh, 1000);
    return () => {
      mounted.current = false;
      clearInterval(timer);
      void current.current?.cancel();
    };
  }, [setShare]);
  const shareURL = share?.url;
  useEffect(() => {
    let disposed = false;
    setQR("");
    if (shareURL)
      void Backend.ShareQRCode()
        .then((value) => {
          if (!disposed) setQR(value);
        })
        .catch(useApp.getState().report);
    return () => {
      disposed = true;
    };
  }, [shareURL]);
  async function run<T>(request: Request<T>, finish: (value: T) => void) {
    current.current = request;
    setBusy(true);
    setMessage("");
    try {
      const value = await request;
      if (mounted.current) finish(value);
    } catch (error) {
      if (mounted.current) setMessage(errorText(error));
    } finally {
      if (mounted.current) {
        setBusy(false);
        current.current = undefined;
      }
    }
  }
  function canImport() {
    if (Object.values(useApp.getState().dirtyEditors).some(Boolean)) {
      setMessage(t("请先保存或关闭未保存的远端文件。"));
      return false;
    }
    return window.confirm(
      t("将替换本机连接和设置；本机 vault 与信任库保留。继续导入？"),
    );
  }
  const endpoint = { url, username, password };
  return (
    <section className="sync-panel">
      <h3>{t("配置同步")}</h3>
      <p className="muted">
        {t("同步连接和设置；密码、vault、本机密钥与主机信任库留在当前设备。")}
      </p>
      <fieldset disabled={busy} className="form-grid">
        <legend>WebDAV</legend>
        <label>
          {t("配置文件完整 URL")}
          <input
            type="url"
            value={url}
            autoCapitalize="none"
            onChange={(event) => {
              setURL(event.target.value);
              setRemote(null);
              setETag("");
              setCreateOnly(true);
            }}
            placeholder="https://server.example/config.json"
          />
        </label>
        <div className="form-row">
          <label>
            {t("用户名")}
            <input
              value={username}
              autoCapitalize="none"
              autoComplete="off"
              onChange={(event) => setUsername(event.target.value)}
            />
          </label>
          <label>
            {t("密码")}
            <input
              type="password"
              autoComplete="off"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
            />
          </label>
        </div>
        <label className="checkbox">
          <input
            type="checkbox"
            checked={createOnly}
            onChange={(event) => setCreateOnly(event.target.checked)}
          />
          {t("仅创建远端文件")}
        </label>
        <div className="form-row">
          <button
            disabled={!url}
            onClick={() => {
              void run(Backend.DownloadWebDAV(endpoint), (value) => {
                setRemote(value);
                setETag(value.etag);
                setCreateOnly(false);
                setMessage(t("远端配置已下载，尚未导入"));
              });
            }}
          >
            {t("下载预览")}
          </button>
          <button
            disabled={!url || (!createOnly && !etag)}
            onClick={() => {
              void run(
                Backend.UploadWebDAV(endpoint, etag, createOnly),
                (value) => {
                  setETag(value);
                  setCreateOnly(false);
                  setMessage(t("配置已上传"));
                },
              );
            }}
          >
            {t("上传本机配置")}
          </button>
        </div>
        {remote && (
          <div className="notice">
            <p>
              {t("{count} 个远端连接", {
                count: remote.envelope.config.sessions?.length ?? 0,
              })}{" "}
              · {t(remote.etag ? "支持 ETag 冲突保护" : "远端未提供 ETag")}
            </p>
            <button
              onClick={() => {
                const config = useApp.getState().config;
                if (config && canImport())
                  void run(
                    Backend.ImportConfig(
                      config.revision,
                      remote.envelope.config,
                    ),
                    (value) => {
                      useApp.getState().apply(value);
                      setMessage(t("配置已导入"));
                    },
                  );
              }}
            >
              {t("导入这份配置")}
            </button>
          </div>
        )}
      </fieldset>
      <fieldset disabled={busy} className="form-grid">
        <legend>{t("局域网分享")}</legend>
        {share && qr && (
          <img
            className="share-qr"
            src={`data:image/png;base64,${qr}`}
            alt={t("KitonyTerms 配置分享二维码")}
            width={256}
            height={256}
          />
        )}
        {!share ? (
          <>
            <label>
              {t("本机监听 IP")}
              <input
                placeholder={t("留空自动选择局域网 IPv4")}
                value={bindIP}
                onChange={(event) => setBindIP(event.target.value)}
              />
            </label>
            <button
              onClick={() => {
                void run(Backend.StartShare(bindIP), (value) => {
                  setShare(value);
                  setMessage(t("分享已启动，有效期 10 分钟"));
                });
              }}
            >
              {t("开始分享本机配置")}
            </button>
          </>
        ) : (
          <div className="notice">
            <p className="path-text">{share.url}</p>
            <p>
              {t("配对码")} <code>{share.pairingCode}</code>
            </p>
            <p className="muted">
              {new Date(share.expiresAtUnix * 1000).toLocaleTimeString()}{" "}
              {t("到期，成功接收后关闭。")}
            </p>
            <button
              onClick={() => {
                void run(Backend.StopShare(), () => {
                  setShare(null);
                  setMessage(t("分享已停止"));
                });
              }}
            >
              {t("停止分享")}
            </button>
          </div>
        )}
        <label>
          {t("分享地址或扫码负载")}
          <input
            autoCapitalize="none"
            value={address}
            onChange={(event) => setAddress(event.target.value)}
            placeholder="http://192.168.1.10:12345/v2/config"
          />
        </label>
        <button onClick={() => setScanning(true)}>{t("扫描分享二维码")}</button>
        <label>
          {t("26 位配对码")}
          <input
            autoComplete="off"
            autoCapitalize="characters"
            value={code}
            onChange={(event) => setCode(event.target.value)}
          />
        </label>
        <button
          disabled={!address}
          onClick={() => {
            const config = useApp.getState().config;
            if (config && canImport())
              void run(
                Backend.ImportLAN(address, code, config.revision),
                (value) => {
                  useApp.getState().apply(value);
                  setCode("");
                  setMessage(t("配置已保存，分享已确认"));
                },
              );
          }}
        >
          {t("接收并导入")}
        </button>
      </fieldset>
      {busy && (
        <button
          onClick={() => {
            void current.current?.cancel();
          }}
        >
          {t("取消当前请求")}
        </button>
      )}
      <p role="status" className="notice">
        {message}
      </p>
      {scanning && (
        <Scanner
          onClose={() => setScanning(false)}
          onValue={(value) => {
            setAddress(value);
            setScanning(false);
            setMessage(t("二维码已识别，请确认后导入"));
          }}
        />
      )}
    </section>
  );
}
