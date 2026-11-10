// src/components/PresetBar.tsx
// "[Preset: Default v]" selector above the Live Inspector (§2 layout) backed by
// `wallpaper_save_preset` / `wallpaper_apply_preset`.
import React, { useState } from 'react';
import { Save } from 'lucide-react';
import { useAppStore } from '../store/useAppStore';

export const PresetBar: React.FC<{ wallpaperId: string }> = ({ wallpaperId }) => {
  const presets = useAppStore((s) => s.presetsByWallpaper[wallpaperId]) ?? [];
  const savePreset = useAppStore((s) => s.savePreset);
  const applyPreset = useAppStore((s) => s.applyPreset);
  const [name, setName] = useState('');
  const [selected, setSelected] = useState('');
  const [msg, setMsg] = useState<string | null>(null);

  const onSave = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;
    try {
      const preset = await savePreset(wallpaperId, name.trim());
      setSelected(preset.presetId);
      setName('');
      setMsg(`Saved "${preset.name}"`);
    } catch (err) {
      setMsg(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="panel" style={{ padding: 12, marginBottom: 12 }}>
      <select
        className="button"
        aria-label="Preset"
        style={{ width: '100%', fontSize: 12 }}
        value={selected}
        onChange={(e) => {
          const presetId = e.target.value;
          setSelected(presetId);
          if (presetId) void applyPreset(wallpaperId, presetId);
        }}
      >
        <option value="">Preset: Default</option>
        {presets.map((p) => (
          <option key={p.presetId} value={p.presetId}>
            Preset: {p.name}
          </option>
        ))}
      </select>
      <form onSubmit={onSave} style={{ display: 'flex', gap: 6, marginTop: 8 }}>
        <input
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Save current as preset…"
          aria-label="Preset name"
          className="button"
          style={{ flex: 1, textAlign: 'left', cursor: 'text', fontSize: 12, padding: '6px 10px' }}
        />
        <button type="submit" className="button" aria-label="Save preset" style={{ padding: '6px 10px' }}>
          <Save size={12} />
        </button>
      </form>
      {msg && <div style={{ fontSize: 11, color: 'var(--text-muted)', marginTop: 6 }}>{msg}</div>}
    </div>
  );
};
