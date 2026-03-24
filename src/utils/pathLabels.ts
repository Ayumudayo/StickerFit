const COMPACT_PATH_LIMIT = 28;

export function compactPathLabel(value: string, maxLength = COMPACT_PATH_LIMIT) {
  const normalizedValue = value.trim();
  if (normalizedValue.length <= maxLength) {
    return normalizedValue;
  }

  const extensionMatch = /\.[^.\\/]+$/.exec(normalizedValue);
  const extension = extensionMatch?.[0] ?? "";
  const suffixLength = extension ? Math.min(extension.length + 4, 9) : 0;
  const prefixLength = Math.max(10, maxLength - 3 - suffixLength);

  if (prefixLength + 3 >= normalizedValue.length) {
    return `${normalizedValue.slice(0, maxLength - 3)}...`;
  }

  const suffix = suffixLength > 0 ? normalizedValue.slice(-suffixLength) : "";
  return suffix
    ? `${normalizedValue.slice(0, prefixLength)}...${suffix}`
    : `${normalizedValue.slice(0, maxLength - 3)}...`;
}
