import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { RuntimeMemoryUsage } from '../types/ipc';
import { formatBytes } from '../lib/format';
import { usePageVisible } from '../lib/hooks';

/** Poll only one native counter; no process enumeration or system-wide telemetry. */
export const ResourceUsage: React.FC<{ cachedBytes: number }> = ({ cachedBytes }) => {
  const [ram, setRam] = useState<number | null>(null);
  // Studio closed to the tray is hidden: stop polling until it is shown again.
  const visible = usePageVisible();
  useEffect(() => {
    if (!visible) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const sample = async () => {
      try {
        const usage = await invoke<RuntimeMemoryUsage>('runtime_get_memory_usage');
        if (!disposed) setRam(Number.isFinite(usage.residentBytes) ? usage.residentBytes : null);
      } catch { if (!disposed) setRam(null); }
      if (!disposed) timer = setTimeout(() => void sample(), 3000);
    };
    void sample();
    return () => { disposed = true; clearTimeout(timer); };
  }, [visible]);
  return <div className="sidebar-resources">
    <div className="resource-row" title="Space used by cached photos on disk"><span>Photo storage</span><span>{formatBytes(cachedBytes)}</span></div>
    <div className="resource-row" title="Resident RAM used by the main app process. Helper processes are separate."><span>RAM</span><span aria-live="off">{ram === null ? '—' : formatBytes(ram)}</span></div>
  </div>;
};
