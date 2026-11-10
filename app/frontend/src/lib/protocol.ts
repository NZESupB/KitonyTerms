export function newerRevision(
  current: string | undefined,
  incoming: string,
): boolean {
  if (!/^\d+$/.test(incoming)) throw new Error("配置版本无效");
  return current === undefined || BigInt(incoming) >= BigInt(current);
}

export function bytesToBase64(bytes: Uint8Array): string {
  let text = "";
  for (const value of bytes) text += String.fromCharCode(value);
  return btoa(text);
}

export function base64ToBytes(value: string): Uint8Array {
  return Uint8Array.from(atob(value), (char) => char.charCodeAt(0));
}

// 失败阻断整个队列；计数随每个任务完成释放，包括被前序失败阻断的任务。
export class OrderedInput {
  private tail: Promise<void>;
  private sequence = 0n;
  private bytes = 0;
  constructor(
    private readonly send: (
      sequence: string,
      data: Uint8Array,
    ) => Promise<void>,
    initial: Promise<string> = Promise.resolve("0"),
  ) {
    this.tail = initial.then((value) => {
      this.sequence = BigInt(value);
    });
    void this.tail.catch(() => undefined);
  }
  enqueue(data: Uint8Array): Promise<void> {
    if (this.bytes + data.length > 65536)
      return Promise.reject(new Error("输入队列已满，请等待或重新连接"));
    this.bytes += data.length;
    const copy = data.slice();
    const request = this.tail
      .then(async () => {
        await this.send((this.sequence + 1n).toString(), copy);
        this.sequence++;
      })
      .finally(() => {
        this.bytes -= copy.length;
      });
    this.tail = request;
    return request;
  }
}
