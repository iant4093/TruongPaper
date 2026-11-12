// A responsive card grid with roving arrow-key navigation. Large lists are windowed: only the
// rows near the viewport are in the DOM. Small lists (or environments without layout) render fully.
import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';

/** Above this many items the grid renders only the visible rows. */
export const VIRTUALIZE_ABOVE = 60;
const ROW_GAP = 20;
const COL_GAP = 16;
const INFO_HEIGHT = 112; // keep in sync with .wallpaper-info
const OVERSCAN_ROWS = 2;

function scrollParent(node: HTMLElement | null): HTMLElement | null {
  for (let el = node?.parentElement ?? null; el; el = el.parentElement) {
    const overflowY = getComputedStyle(el).overflowY;
    if (overflowY === 'auto' || overflowY === 'scroll') return el;
  }
  return null;
}

export interface GridCell { index: number; tabIndex: 0 | -1 }

export function VirtualGrid<T>({ items, getKey, renderItem, label, minColumn = 220, className = '' }: {
  items: readonly T[];
  getKey: (item: T) => string;
  renderItem: (item: T, cell: GridCell) => React.ReactNode;
  label: string;
  minColumn?: number;
  className?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [viewport, setViewport] = useState({ top: 0, height: 0 });
  const [focusIndex, setFocusIndex] = useState(0);
  const pendingFocus = useRef<number | null>(null);

  const cols = Math.max(1, Math.floor((width + COL_GAP) / (minColumn + COL_GAP)));
  const colWidth = width > 0 ? (width - COL_GAP * (cols - 1)) / cols : 0;
  const rowHeight = colWidth * 0.625 + INFO_HEIGHT + ROW_GAP;
  const totalRows = Math.ceil(items.length / cols);
  const windowed = items.length > VIRTUALIZE_ABOVE && width > 0 && viewport.height > 0;

  const measure = useCallback(() => {
    const node = ref.current;
    if (!node) return;
    setWidth(node.clientWidth);
    const scroller = scrollParent(node);
    if (!scroller) { setViewport((v) => (v.height === 0 && v.top === 0 ? v : { top: 0, height: 0 })); return; }
    const offset = node.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
    const next = { top: Math.max(0, scroller.scrollTop - offset), height: scroller.clientHeight };
    setViewport((v) => (Math.abs(v.top - next.top) < 1 && v.height === next.height ? v : next));
  }, []);

  useLayoutEffect(() => {
    measure();
    const node = ref.current;
    const scroller = scrollParent(node);
    let frame = 0;
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(measure); };
    scroller?.addEventListener('scroll', schedule, { passive: true });
    window.addEventListener('resize', schedule);
    const observer = typeof ResizeObserver !== 'undefined' && node ? new ResizeObserver(schedule) : null;
    if (node) observer?.observe(node);
    return () => {
      cancelAnimationFrame(frame);
      scroller?.removeEventListener('scroll', schedule);
      window.removeEventListener('resize', schedule);
      observer?.disconnect();
    };
  }, [measure]);

  let startRow = 0;
  let endRow = totalRows;
  if (windowed) {
    startRow = Math.max(0, Math.floor(viewport.top / rowHeight) - OVERSCAN_ROWS);
    endRow = Math.min(totalRows, Math.ceil((viewport.top + viewport.height) / rowHeight) + OVERSCAN_ROWS);
  }
  const start = startRow * cols;
  const end = Math.min(items.length, endRow * cols);
  const visible = useMemo(() => items.slice(start, end), [items, start, end]);

  const lastIndex = Math.max(0, items.length - 1);
  const current = Math.min(focusIndex, lastIndex);
  const tabbableIndex = current >= start && current < end ? current : start;

  // After a keyboard jump into an unrendered row, focus the tile once it exists.
  useEffect(() => {
    const wanted = pendingFocus.current;
    if (wanted === null) return;
    const target = ref.current?.querySelector<HTMLElement>(`[data-grid-index="${wanted}"] [data-grid-select]`);
    if (target) { pendingFocus.current = null; target.focus(); }
  }, [start, end, items]);

  const move = (to: number) => {
    const index = Math.min(lastIndex, Math.max(0, to));
    setFocusIndex(index);
    const target = ref.current?.querySelector<HTMLElement>(`[data-grid-index="${index}"] [data-grid-select]`);
    if (target) { target.focus(); target.scrollIntoView?.({ block: 'nearest' }); return; }
    pendingFocus.current = index;
    const node = ref.current;
    const scroller = scrollParent(node);
    if (node && scroller && windowed) {
      const offset = node.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
      scroller.scrollTop = offset + Math.floor(index / cols) * rowHeight - 24;
    }
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    const select = (event.target as HTMLElement).closest?.('[data-grid-select]');
    const cell = select?.closest<HTMLElement>('[data-grid-index]');
    if (!select || !cell || event.altKey || event.ctrlKey || event.metaKey) return;
    const index = Number(cell.dataset.gridIndex);
    const delta: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -cols, ArrowDown: cols };
    if (event.key in delta) { event.preventDefault(); move(index + delta[event.key]); }
    else if (event.key === 'Home') { event.preventDefault(); move(0); }
    else if (event.key === 'End') { event.preventDefault(); move(lastIndex); }
  };

  const style: React.CSSProperties = {};
  if (width > 0) style.gridTemplateColumns = `repeat(${cols}, minmax(0, 1fr))`;
  if (windowed) {
    style.paddingTop = startRow * rowHeight;
    style.paddingBottom = (totalRows - endRow) * rowHeight;
    style.gridAutoRows = rowHeight - ROW_GAP;
  }

  return (
    <div ref={ref} role="group" aria-label={label} className={`wallpaper-grid ${className}`} style={style}
      onKeyDown={onKeyDown}
      onFocusCapture={(event) => {
        const target = event.target as HTMLElement;
        const cell = target.closest?.<HTMLElement>('[data-grid-index]');
        if (target.matches?.('[data-grid-select]') && cell) setFocusIndex(Number(cell.dataset.gridIndex));
      }}>
      {visible.map((item, i) => (
        <div key={getKey(item)} data-grid-index={start + i} style={{ display: 'contents' }}>
          {renderItem(item, { index: start + i, tabIndex: start + i === tabbableIndex ? 0 : -1 })}
        </div>
      ))}
    </div>
  );
}
