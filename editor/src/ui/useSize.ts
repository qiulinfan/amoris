// The size of an element, kept current with a ResizeObserver. A callback ref, so it also works for
// elements that mount later (after an empty state, for instance).

import { useCallback, useRef, useState } from "react";

export function useSize<T extends HTMLElement>(): [(el: T | null) => void, { width: number; height: number }] {
  const [size, setSize] = useState({ width: 0, height: 0 });
  const observer = useRef<ResizeObserver | null>(null);
  const ref = useCallback((el: T | null) => {
    observer.current?.disconnect();
    observer.current = null;
    if (!el) return;
    const update = () => setSize((s) => (s.width === el.clientWidth && s.height === el.clientHeight ? s : { width: el.clientWidth, height: el.clientHeight }));
    observer.current = new ResizeObserver(update);
    observer.current.observe(el);
    update();
  }, []);
  return [ref, size];
}
