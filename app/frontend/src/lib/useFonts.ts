import { useEffect, useState } from "react";
import { Backend, type FontCatalog } from "./backend";

export function useFonts() {
  const [catalog, setCatalog] = useState<FontCatalog | null>(null);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let stopped = false;
    setLoading(true);
    setFailed(false);
    const request = Backend.GetFonts();
    void request.then(
      (value) => {
        if (stopped) return;
        setCatalog(value);
        setLoading(false);
      },
      () => {
        if (stopped) return;
        setFailed(true);
        setLoading(false);
      },
    );
    return () => {
      stopped = true;
      void request.cancel().catch(() => undefined);
    };
  }, [attempt]);
  return {
    catalog,
    loading,
    failed,
    retry: () => setAttempt((value) => value + 1),
  };
}
