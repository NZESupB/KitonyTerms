import { useCallback } from "react";
import { useApp } from "./backend";
import { translate, type TextKey, type TextValues } from "./i18n";
export function useText() {
  const language = useApp(
    (state) => state.config?.config.settings.language ?? "chinese",
  );
  return useCallback(
    (key: TextKey, values?: TextValues) => translate(language, key, values),
    [language],
  );
}
