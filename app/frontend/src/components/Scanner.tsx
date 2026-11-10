import { useEffect, useRef, useState } from "react";
import { Backend, errorText } from "../lib/backend";
import { bytesToBase64 } from "../lib/protocol";
import { Modal } from "./Dialogs";
import { useText } from "../lib/useText";

export function Scanner({
  onValue,
  onClose,
}: {
  onValue: (value: string) => void;
  onClose: () => void;
}) {
  const t = useText();
  const text = useRef(t);
  text.current = t;
  const video = useRef<HTMLVideoElement>(null);
  const callbacks = useRef({ onValue, onClose });
  callbacks.current = { onValue, onClose };
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    let stream: MediaStream | undefined;
    let timer: ReturnType<typeof setTimeout>;
    let request: ReturnType<typeof Backend.DecodeQR> | undefined;
    const stop = () => {
      active = false;
      clearTimeout(timer);
      stream?.getTracks().forEach((track) => track.stop());
      void request?.cancel();
    };
    const canvas = document.createElement("canvas");
    const context = canvas.getContext("2d", { willReadFrequently: true });
    const scan = async () => {
      if (!active || !video.current || !context) return;
      try {
        const source = video.current;
        if (source.readyState >= 2 && source.videoWidth && source.videoHeight) {
          const scale = Math.min(
            480 / source.videoWidth,
            480 / source.videoHeight,
            1,
          );
          canvas.width = Math.floor(source.videoWidth * scale);
          canvas.height = Math.floor(source.videoHeight * scale);
          context.drawImage(source, 0, 0, canvas.width, canvas.height);
          const rgba = context.getImageData(
            0,
            0,
            canvas.width,
            canvas.height,
          ).data;
          const gray = new Uint8Array(canvas.width * canvas.height);
          for (let index = 0; index < gray.length; index++)
            gray[index] =
              (rgba[index * 4] * 77 +
                rgba[index * 4 + 1] * 150 +
                rgba[index * 4 + 2] * 29) >>
              8;
          request = Backend.DecodeQR({
            width: canvas.width,
            height: canvas.height,
            data: bytesToBase64(gray),
          });
          const value = await request;
          if (active && value) {
            stop();
            callbacks.current.onValue(value);
            return;
          }
        }
        if (active)
          timer = setTimeout(() => {
            void scan();
          }, 500);
      } catch (error) {
        if (active) {
          setError(errorText(error));
          stop();
        }
      }
    };
    async function start() {
      try {
        if (!context || !navigator.mediaDevices?.getUserMedia)
          throw new Error(
            text.current("当前环境不支持相机，请使用分享地址和配对码。"),
          );
        stream = await navigator.mediaDevices.getUserMedia({
          video: {
            facingMode: { ideal: "environment" },
            width: { ideal: 640 },
            height: { ideal: 480 },
          },
          audio: false,
        });
        if (!active) {
          stream.getTracks().forEach((track) => track.stop());
          return;
        }
        if (!video.current) {
          stop();
          return;
        }
        video.current.srcObject = stream;
        await video.current.play();
        if (active) void scan();
      } catch (error) {
        if (active) {
          setError(errorText(error));
          stop();
        }
      }
    }
    const visibility = () => {
      if (document.visibilityState !== "visible") {
        stop();
        setError(text.current("扫码已暂停，请关闭后重新打开。"));
      }
    };
    document.addEventListener("visibilitychange", visibility);
    void start();
    return () => {
      stop();
      document.removeEventListener("visibilitychange", visibility);
    };
  }, []);
  return (
    <Modal
      open
      title={t("扫描分享二维码")}
      description={t("将另一台设备的 KitonyTerms 分享二维码放在画面中。")}
      onOpenChange={(open) => {
        if (!open) callbacks.current.onClose();
      }}
    >
      <video
        ref={video}
        className="scanner-video"
        muted
        playsInline
        aria-label={t("扫码相机画面")}
      />
      {error ? (
        <p role="alert" className="error">
          {error}
        </p>
      ) : (
        <p role="status" className="muted">
          {t("正在识别…")}
        </p>
      )}
      <footer>
        <button onClick={() => callbacks.current.onClose()}>
          {t("关闭相机")}
        </button>
      </footer>
    </Modal>
  );
}
