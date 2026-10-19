// Fixed-row-height windowed list. Renders only the visible rows (+ overscan),
// so 100k-line logs and diffs stay at a few dozen DOM nodes.

import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";

export interface VirtualListHandle {
  scrollToIndex: (i: number, align?: "start" | "end") => void;
  scrollToEnd: () => void;
  isAtEnd: () => boolean;
}

interface Props {
  count: number;
  rowHeight: number;
  renderRow: (index: number) => ReactNode;
  overscan?: number;
  className?: string;
  /** Keep the view pinned to the last row when new rows arrive. */
  stickToEnd?: boolean;
  /** Called when the user scrolls away from / back to the end. */
  onAtEndChange?: (atEnd: boolean) => void;
  ariaLabel?: string;
  role?: string;
  /** Height used before the container is measured. */
  initialHeight?: number;
}

export const VirtualList = forwardRef<VirtualListHandle, Props>(function VirtualList(
  {
    count,
    rowHeight,
    renderRow,
    overscan = 12,
    className,
    stickToEnd,
    onAtEndChange,
    ariaLabel,
    role = "list",
    initialHeight = 600,
  },
  ref,
) {
  const el = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(initialHeight);
  const frame = useRef(0);
  const atEnd = useRef(true);

  const computeAtEnd = useCallback(() => {
    const n = el.current;
    if (!n) return true;
    return n.scrollHeight - n.scrollTop - n.clientHeight < rowHeight * 1.5;
  }, [rowHeight]);

  const onScroll = useCallback(() => {
    if (frame.current) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = 0;
      const n = el.current;
      if (!n) return;
      setScrollTop(n.scrollTop);
      const e = computeAtEnd();
      if (e !== atEnd.current) {
        atEnd.current = e;
        onAtEndChange?.(e);
      }
    });
  }, [computeAtEnd, onAtEndChange]);

  useEffect(() => () => cancelAnimationFrame(frame.current), []);

  useLayoutEffect(() => {
    const n = el.current;
    if (!n) return;
    setHeight(n.clientHeight || initialHeight);
    const ro = new ResizeObserver(() => setHeight(n.clientHeight || initialHeight));
    ro.observe(n);
    return () => ro.disconnect();
  }, [initialHeight]);

  // Pin to the end when rows are appended.
  useLayoutEffect(() => {
    const n = el.current;
    if (stickToEnd && n) {
      n.scrollTop = n.scrollHeight;
      setScrollTop(n.scrollTop);
    }
  }, [count, stickToEnd]);

  useImperativeHandle(
    ref,
    () => ({
      scrollToIndex: (i, align = "start") => {
        const n = el.current;
        if (!n) return;
        n.scrollTop = align === "start" ? i * rowHeight : (i + 1) * rowHeight - n.clientHeight;
      },
      scrollToEnd: () => {
        const n = el.current;
        if (n) n.scrollTop = n.scrollHeight;
      },
      isAtEnd: computeAtEnd,
    }),
    [rowHeight, computeAtEnd],
  );

  const first = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const last = Math.min(count, Math.ceil((scrollTop + height) / rowHeight) + overscan);
  const rows: ReactNode[] = [];
  for (let i = first; i < last; i++) {
    rows.push(
      <div
        key={i}
        className="vrow"
        style={{ position: "absolute", top: i * rowHeight, height: rowHeight, left: 0, right: 0 }}
      >
        {renderRow(i)}
      </div>,
    );
  }

  return (
    <div
      ref={el}
      className={`vlist ${className ?? ""}`}
      onScroll={onScroll}
      role={role}
      aria-label={ariaLabel}
      aria-rowcount={role === "grid" || role === "table" ? count : undefined}
      tabIndex={0}
    >
      <div style={{ position: "relative", height: count * rowHeight, width: "100%" }}>{rows}</div>
    </div>
  );
});
