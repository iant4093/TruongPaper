import { Dropdown } from './Dropdown';
import React, { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ChevronLeft, ChevronRight, Search, SlidersHorizontal, Star } from 'lucide-react';
import type { WorkshopBrowsePage, WorkshopBrowseQuery, WorkshopSort } from '../types/ipc';
import { useAppStore } from '../store/useAppStore';
import { parseImportLink } from '../lib/importLinks';
import { Thumb } from './ui';
import { workshopTypeFromTags } from '../lib/format';

const DEFAULT_QUERY: WorkshopBrowseQuery = { search: '', sort: 'trending', page: 1, days: 7, tags: [] };
const count = (value: number) => new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 }).format(value);

export const WorkshopBrowser: React.FC = () => {
  const [query, setQuery] = useState(DEFAULT_QUERY);
  const [input, setInput] = useState('');
  const [data, setData] = useState<WorkshopBrowsePage | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  const [more, setMore] = useState(false);
  const [previewing, setPreviewing] = useState<string | null>(null);
  const inspect = useAppStore((s) => s.inspectWorkshopUrl);
  const openAuth = useAppStore((s) => s.openSteamAuthModal);
  const wallpapers = useAppStore((s) => s.wallpapers);
  const guest = useAppStore((s) => s.guestAvailability);
  const installedIds = useMemo(() => new Set(wallpapers.map((w) => w.workshopId)), [wallpapers]);
  const loadGuestAvailability = useAppStore((s) => s.loadGuestAvailability);
  const previewRequest = useRef(0);
  const top = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const timer = setTimeout(() => {
      if (parseImportLink(input)?.kind === 'steam_workshop') return;
      setQuery((q) => q.search === input.trim() ? q : { ...q, search: input.trim(), page: 1 });
    }, 450);
    return () => clearTimeout(timer);
  }, [input]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true); setError(null);
    void invoke<WorkshopBrowsePage>('steam_browse_workshop', { query }).then((page) => {
      if (!cancelled) { setData(page); setLoading(false); }
    }, (err) => {
      if (!cancelled) { setError(String(err)); setLoading(false); }
    });
    return () => { cancelled = true; };
  }, [query, retry]);

  useEffect(() => {
    if (data?.items.length) void loadGuestAvailability(data.items.map((item) => item.workshopId));
  }, [data, loadGuestAvailability]);

  useEffect(() => () => { previewRequest.current++; }, []);
  const preview = async (id: string) => {
    const request = ++previewRequest.current;
    setPreviewing(id); setPreviewError(null);
    try { await inspect(id); }
    catch (err) { if (request === previewRequest.current) setPreviewError(err instanceof Error ? err.message : String(err)); }
    finally { if (request === previewRequest.current) setPreviewing(null); }
  };
  const update = (partial: Partial<WorkshopBrowseQuery>) => setQuery((q) => ({ ...q, ...partial, page: 1 }));
  const chooseTag = (name: string, value: string) => {
    const group = data?.tagGroups.find((g) => g.name === name);
    update({ tags: [...query.tags.filter((tag) => !group?.tags.some((t) => t.tag === tag)), ...(value ? [value] : [])] });
  };
  const changePage = (page: number) => {
    setQuery((q) => ({ ...q, page }));
    scrollToTop(top.current);
  };
  // Steam pages hold 30 results, so use a column count that divides 30 (no half-empty last row).
  const gridRef = useRef<HTMLDivElement>(null);
  const [cols, setCols] = useState(3);
  useEffect(() => {
    const node = gridRef.current;
    if (!node || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(([entry]) => setCols(columnsFor(entry.contentRect.width)));
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  const selectFilter = (name: string, label: string) => {
    const group = data?.tagGroups.find((g) => g.name === name);
    return <div className="workshop-filter" key={name}><span>{label}</span>
      <Dropdown label={`Workshop ${label.toLowerCase()}`}
        value={query.tags.find((t) => group?.tags.some((tag) => tag.tag === t)) ?? ''}
        options={[['', 'All'], ...(group?.tags ?? []).map((tag) => [tag.tag, tag.label] as const)]}
        onChange={(tag) => chooseTag(name, tag)} />
    </div>;
  };

  return <div ref={top} className="workshop-discover">
    <div className="workshop-search-row">
      <form className="workshop-search" onSubmit={(e) => {
        e.preventDefault();
        const target = parseImportLink(input);
        if (target?.kind === 'steam_workshop') void preview(target.workshopId);
        else update({ search: input.trim() });
      }}>
        <Search size={16} aria-hidden="true" />
        <input aria-label="Search Steam Workshop" type="search" maxLength={200} placeholder="Search wallpapers, or paste a Workshop link or ID…" value={input} onChange={(e) => setInput(e.target.value)} />
        <button className="button" type="submit">Search</button>
      </form>
    </div>
    <div className="workshop-filters">
      <div className="workshop-filter"><span>Sort</span>
        <Dropdown label="Workshop sort" value={query.sort}
          options={[['trending', 'Trending'], ['most_subscribed', 'Most subscribed'], ['newest', 'Newest'], ['top_rated', 'Top rated']]}
          onChange={(sort: WorkshopSort) => update({ sort })} />
      </div>
      {query.sort === 'trending' && <div className="workshop-filter"><span>Period</span>
        <Dropdown label="Trending period" value={query.days}
          options={[[7, 'This week'], [30, 'This month'], [90, '3 months'], [365, 'This year'], [-1, 'All time']]}
          onChange={(days) => update({ days })} />
      </div>}
      <button className="button" aria-expanded={more} onClick={() => setMore(!more)}><SlidersHorizontal size={14} />Filters{query.tags.length > 0 ? ` (${query.tags.length})` : ''}</button>
      {(query.tags.length > 0 || query.search) && <button className="button button-quiet" onClick={() => { setInput(''); update({ search: '', tags: [] }); }}>Clear</button>}
    </div>
    {more && <div className="card workshop-more-filters">
      {selectFilter('Type', 'Type')}{selectFilter('Genre', 'Genre')}{selectFilter('Resolution', 'Resolution')}{selectFilter('Age Rating', 'Age rating')}
      <div className="workshop-tags">{data?.tagGroups.find((g) => g.name === 'Miscellaneous')?.tags.map((tag) => <label key={tag.tag}>
        <input type="checkbox" checked={query.tags.includes(tag.tag)} onChange={(e) => update({ tags: e.target.checked ? [...query.tags, tag.tag] : query.tags.filter((t) => t !== tag.tag) })} />{tag.label}
      </label>)}</div>
    </div>}
    <div className="workshop-results-info" role="status">
      {loading ? 'Loading Steam Workshop…' : data ? `${data.totalItems.toLocaleString()} wallpapers · Page ${data.page.toLocaleString()}` : ''}
    </div>
    {error && <div className="workshop-error" role="alert"><span>{error}</span><button className="button" onClick={() => setRetry((n) => n + 1)}>Retry</button></div>}
    {previewError && <div className="workshop-error" role="alert"><span>{previewError}</span>
      {/^(sign in to steam|steam login required)/i.test(previewError) && <button className="button" onClick={openAuth}>Sign in</button>}</div>}
    <div ref={gridRef} className="wallpaper-grid workshop-browse-grid" aria-busy={loading}
      style={{ gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))` }}>
      {!loading && !error && data?.items.map((item) => <button key={item.workshopId} type="button" className="workshop-result card" aria-label={`Preview ${item.title}`} onClick={() => void preview(item.workshopId)} disabled={!!previewing}>
        <Thumb src={item.previewUrl} alt={item.title} height={150} lazy />
        <div className="workshop-result-body"><div className="workshop-result-type"><span>{workshopTypeFromTags(item.tags)}</span><span>{guest[item.workshopId] === 'available' && <span className="workshop-mirror-badge" title="Available on GGNetwork, a third-party mirror">GGNetwork</span>}{installedIds.has(item.workshopId) ? 'Installed' : previewing === item.workshopId ? 'Opening…' : ''}</span></div>
          <div className="workshop-result-title" title={item.title}>{item.title}</div>
          <div className="workshop-result-author">{item.author || 'Workshop creator'}</div>
          <div className="workshop-result-stats"><span>{count(item.subscribers)} subscribers</span>{item.rating > 0 ? <span aria-label={`${item.rating} out of 5 stars`}><Star size={11} fill="currentColor" />{item.rating}/5</span> : <span className="workshop-result-unrated">Not rated</span>}</div>
        </div>
      </button>)}
      {loading && Array.from({ length: cols * 2 }, (_, i) => <div className="workshop-skeleton card" key={i}><div /><span /></div>)}
    </div>
    {!loading && !error && data?.items.length === 0 && <div className="workshop-empty">No wallpapers match these filters. Try a different search or clear the filters.</div>}
    {data && data.totalPages > 1 && <div className="workshop-pagination">
      <button className="button" aria-label="Previous Workshop page" disabled={loading || data.page <= 1} onClick={() => changePage(data.page - 1)}><ChevronLeft size={15} />Previous</button>
      <span className="workshop-page-label">Page {data.page}</span>
      <button className="button" aria-label="Next Workshop page" disabled={loading || data.page >= data.totalPages} onClick={() => changePage(data.page + 1)}>Next<ChevronRight size={15} /></button>
    </div>}
  </div>;
};

/** Largest column count dividing a 30-result page that keeps tiles at least 200 px wide. */
export function columnsFor(width: number): number {
  return [6, 5, 3, 2].find((n) => width / n - 16 >= 200) ?? 1;
}

/** Scrolls the nearest scrolling container so the top of `node` is visible. */
function scrollToTop(node: HTMLElement | null) {
  let el = node?.parentElement ?? null;
  while (el && !(el.scrollHeight > el.clientHeight && /(auto|scroll)/.test(getComputedStyle(el).overflowY))) el = el.parentElement;
  if (el) el.scrollTop = 0;
  else node?.scrollIntoView?.({ block: 'start' });
}
