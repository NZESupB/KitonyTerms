import {
  useCallback,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import { useApp } from "./backend";

// 先同步登记布尔元数据，再提交局部状态；受保护的值始终只在组件内。
export function useProtectedState<T>(
  initial: T | (() => T),
  blocked: (value: T) => boolean = Boolean,
): [T, Dispatch<SetStateAction<T>>] {
  const [value, setValue] = useState(initial);
  const latest = useRef(value);
  const predicate = useRef(blocked);
  predicate.current = blocked;
  const live = useRef(true);
  const owner = useId();
  useLayoutEffect(() => {
    live.current = true;
    useApp.getState().setProtected(owner, predicate.current(latest.current));
    return () => {
      live.current = false;
      useApp.getState().setProtected(owner, false);
    };
  }, [owner]);
  const change = useCallback<Dispatch<SetStateAction<T>>>(
    (update) => {
      if (!live.current) return;
      const next =
        typeof update === "function"
          ? (update as (value: T) => T)(latest.current)
          : update;
      latest.current = next;
      useApp.getState().setProtected(owner, predicate.current(next));
      setValue(next);
    },
    [owner],
  );
  return [value, change];
}

export function useRestartProtection(blocked: boolean): void {
  const owner = useId();
  useLayoutEffect(() => {
    useApp.getState().setProtected(owner, blocked);
    return () => useApp.getState().setProtected(owner, false);
  }, [owner, blocked]);
}
