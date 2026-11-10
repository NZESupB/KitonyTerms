import { useEffect, useRef, useState } from "react";

export function Splitter({
  orientation,
  value,
  min,
  max,
  onChange,
  label,
  controls,
}: {
  orientation: "vertical" | "horizontal";
  value: number;
  min: number;
  max: number;
  onChange: (value: number) => void;
  label: string;
  controls: string;
}) {
  const origin = useRef<{ coordinate: number; value: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  const change = useRef(onChange);
  change.current = onChange;
  useEffect(() => {
    const cancel = () => {
      if (origin.current) change.current(origin.current.value);
      origin.current = null;
      setDragging(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") cancel();
    };
    window.addEventListener("blur", cancel);
    window.addEventListener("keydown", escape);
    return () => {
      window.removeEventListener("blur", cancel);
      window.removeEventListener("keydown", escape);
    };
  }, []);
  const clamp = (next: number) => Math.max(min, Math.min(max, next));
  const cancel = () => {
    if (origin.current) onChange(origin.current.value);
    origin.current = null;
    setDragging(false);
  };
  return (
    <div
      role="separator"
      tabIndex={0}
      aria-label={label}
      aria-controls={controls}
      aria-orientation={orientation}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuenow={Math.round(value)}
      className={`splitter splitter-${orientation} ${dragging ? "is-dragging" : ""}`}
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        event.preventDefault();
        event.currentTarget.setPointerCapture(event.pointerId);
        origin.current = {
          coordinate:
            orientation === "vertical" ? event.clientX : event.clientY,
          value,
        };
        setDragging(true);
      }}
      onPointerMove={(event) => {
        if (!origin.current) return;
        const coordinate =
          orientation === "vertical" ? event.clientX : event.clientY;
        onChange(
          clamp(
            origin.current.value +
              (coordinate - origin.current.coordinate) *
                (orientation === "horizontal" ? -1 : 1),
          ),
        );
      }}
      onPointerUp={() => {
        origin.current = null;
        setDragging(false);
      }}
      onPointerCancel={cancel}
      onLostPointerCapture={cancel}
      onKeyDown={(event) => {
        const increase = orientation === "vertical" ? "ArrowRight" : "ArrowUp";
        const decrease = orientation === "vertical" ? "ArrowLeft" : "ArrowDown";
        if (event.key === increase || event.key === decrease) {
          event.preventDefault();
          onChange(clamp(value + (event.key === increase ? 10 : -10)));
        } else if (event.key === "Home" || event.key === "End") {
          event.preventDefault();
          onChange(event.key === "Home" ? min : max);
        }
      }}
    />
  );
}
