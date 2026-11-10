const genericFamilies = new Set([
  "serif",
  "sans-serif",
  "monospace",
  "cursive",
  "fantasy",
  "system-ui",
  "ui-serif",
  "ui-sans-serif",
  "ui-monospace",
  "ui-rounded",
  "math",
  "fangsong",
]);

// 界面设置保存单个字体名；终端的字体栈不经过此函数。
export function uiFontFamily(value: string | undefined): string | undefined {
  const name = value?.trim();
  if (!name) return undefined;
  let family = name;
  if (!genericFamilies.has(name.toLowerCase())) {
    let escaped = "";
    for (const character of name) {
      const point = character.codePointAt(0)!;
      if (character === '"' || character === "\\") escaped += `\\${character}`;
      else if (point === 0) escaped += "\ufffd";
      else if (point < 32 || point === 127)
        escaped += `\\${point.toString(16)} `;
      else escaped += character;
    }
    family = `"${escaped}"`;
  }
  return `${family}, var(--ui-font-default)`;
}

export function accentForeground(color: string): string {
  if (!/^#[0-9a-f]{6}$/i.test(color))
    throw new Error("强调色必须为六位十六进制颜色");
  const channels = [1, 3, 5].map((offset) => {
    const value = parseInt(color.slice(offset, offset + 2), 16) / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  const luminance =
    channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
  return (luminance + 0.05) / 0.05 >= 1.05 / (luminance + 0.05)
    ? "#000000"
    : "#ffffff";
}
