export function validFileName(name: string): boolean {
  if (!name.trim() || name === "." || name === "..") return false;
  for (let index = 0; index < name.length; index++) {
    const code = name.charCodeAt(index);
    if (code < 32 || code === 127 || code === 47 || code === 92) return false;
  }
  return true;
}
