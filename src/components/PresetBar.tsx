import { Dropdown } from './Dropdown';
// src/components/PresetBar.tsx
// "[Preset: Default v]" selector above the Live Inspector (§2 layout) backed by
// `wallpaper_save_preset` / `wallpaper_apply_preset`.
import React, { useState } from 'react';
import { Save } from 'lucide-react';
import { useAppStore } from '../store/useAppStore';
import { toast } from '../store/feedback';

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
    <div className="preset-bar">
      <Dropdown label="Preset" value={selected}
        options={[['', 'Preset: Default'], ...presets.map((preset) => [preset.presetId, `Preset: ${preset.name}`] as const)]}
        onChange={(presetId) => {
          setSelected(presetId);
          if (presetId) applyPreset(wallpaperId, presetId).catch((err) => toast.error(err, 'Could not apply the preset'));
        }}
      />
      <form onSubmit={onSave} className="preset-save">
        <input
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="Save current as preset…"
          aria-label="Preset name"
          className="button"
        />
        <button type="submit" className="button" aria-label="Save preset" disabled={!name.trim()}>
          <Save size={12} />
        </button>
      </form>
      {msg && <div style={{ fontSize: 11, color: 'var(--text-muted)', marginTop: 6 }}>{msg}</div>}
    </div>
  );
};
