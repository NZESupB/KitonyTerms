import { useEffect, useState } from "react";
import {
  Backend,
  useApp,
  type SessionSnapshot,
  type ShellState,
} from "./backend";

export function useShellState(
  session: SessionSnapshot,
  enabled: boolean,
): ShellState | null {
  const owner = `${session.id}:${session.generation}`;
  const [latest, setLatest] = useState<{
    owner: string;
    value: ShellState;
  } | null>(null);
  const quitting = useApp((state) => state.quitting);
  useEffect(() => {
    if (!enabled || session.status !== "connected" || quitting) return;
    let stopped = false;
    let revision = 0;
    let running = false;
    let timer: ReturnType<typeof setTimeout>;
    let query: ReturnType<typeof Backend.GetShellState> | undefined;
    const visible = () => document.visibilityState === "visible";
    const poll = async () => {
      if (stopped || running || !visible()) return;
      running = true;
      const requestRevision = revision;
      try {
        query = Backend.GetShellState(session.id, session.generation);
        const value = await query;
        if (!stopped && requestRevision === revision)
          setLatest({ owner, value });
      } catch (error) {
        if (!stopped && requestRevision === revision)
          useApp.getState().report(error);
      } finally {
        running = false;
        query = undefined;
        if (!stopped && visible()) timer = setTimeout(() => void poll(), 500);
      }
    };
    const visibility = () => {
      clearTimeout(timer);
      if (visible()) void poll();
      else {
        revision++;
        if (query) void query.cancel().catch(() => {});
      }
    };
    document.addEventListener("visibilitychange", visibility);
    void poll();
    return () => {
      stopped = true;
      revision++;
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", visibility);
      if (query) void query.cancel().catch(() => {});
    };
  }, [
    session.id,
    session.generation,
    session.status,
    owner,
    enabled,
    quitting,
  ]);
  return latest?.owner === owner ? latest.value : null;
}
