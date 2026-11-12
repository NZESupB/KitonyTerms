import type { Plugin } from "vite";

// WKWebView 的 OffscreenCanvas 可能无法识别已安装字体；统一使用 xterm 自带的 DOM 度量。
// 此转换针对锁定的 xterm 5.5.0，升级依赖后须重新核对，不能静默跳过。
export function xtermFontMetrics(): Plugin {
  return {
    name: "xterm-dom-font-metrics",
    enforce: "pre",
    transform(code, id) {
      if (
        !id
          .split("?")[0]
          .replace(/\\/g, "/")
          .endsWith("/@xterm/xterm/lib/xterm.js")
      )
        return;
      const strategy =
        /try\{this\._measureStrategy=this\.register\(new [\w$]+\(this\._optionsService\)\)\}catch(?:\([\w$]+\))?\{(this\._measureStrategy=this\.register\(new [\w$]+\([^)]*\)\))\}/g;
      if ([...code.matchAll(strategy)].length !== 1)
        throw new Error("xterm 字体测量源码已变化，请核对 DOM 度量适配");
      return { code: code.replace(strategy, "$1;"), map: null };
    },
  };
}
