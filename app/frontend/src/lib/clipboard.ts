import { Backend } from "./backend";

export async function writeClipboard(text: string): Promise<void> {
  try {
    await Backend.WriteClipboard(text);
    return;
  } catch {
    // 原生剪贴板不可用时沿用WebView路径，复制内容不进入错误或日志。
  }
  try {
    if (!navigator.clipboard?.writeText) throw new Error("剪贴板不可用");
    await navigator.clipboard.writeText(text);
  } catch {
    throw new Error("复制失败，原生与 WebView 剪贴板均不可用");
  }
}

export async function readClipboard(): Promise<string> {
  try {
    return await Backend.ReadClipboard();
  } catch {
    // 平台未提供原生接口时允许WebView读取，失败只返回类别。
  }
  try {
    if (!navigator.clipboard?.readText) throw new Error("剪贴板不可用");
    return await navigator.clipboard.readText();
  } catch {
    throw new Error("读取剪贴板失败，原生与 WebView 剪贴板均不可用");
  }
}
