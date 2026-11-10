// src/components/PropertyInspector.tsx
import React, { useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import type {
  ProjectJsonSchema,
  UniversalAdjustments,
  WallpaperPreset,
} from '../types/ipc';

/**
 * Converts Wallpaper Engine normalized float color string ("0.15 0.45 0.85")
 * into standard 6-digit hex ("#2673d9").
 */
export function weColorToHex(weColor: string | undefined): string {
  if (!weColor) return '#ffffff';
  const trimmed = weColor.trim();
  if (trimmed.startsWith('#')) return trimmed;
  const parts = trimmed.split(/\s+/).map(Number);
  if (parts.length < 3 || parts.some(isNaN)) return '#ffffff';
  const toByte = (n: number) => {
    // Some older wallpapers store 0..255 integers instead of 0..1 floats
    const normalized = parts.some((p) => p > 1.0) ? n / 255.0 : n;
    return Math.max(0, Math.min(255, Math.round(normalized * 255)))
      .toString(16)
      .padStart(2, '0');
  };
  return `#${toByte(parts[0])}${toByte(parts[1])}${toByte(parts[2])}`;
}

/**
 * Converts "#2673d9" back to Wallpaper Engine's "0.14902 0.45098 0.85098" string.
 */
export function hexToWeColor(hex: string): string {
  const clean = hex.replace('#', '');
  if (clean.length !== 6) return '1.00000 1.00000 1.00000';
  const r = (parseInt(clean.slice(0, 2), 16) / 255).toFixed(5);
  const g = (parseInt(clean.slice(2, 4), 16) / 255).toFixed(5);
  const b = (parseInt(clean.slice(4, 6), 16) / 255).toFixed(5);
  return `${r} ${g} ${b}`;
}

/**
 * Safely evaluates Wallpaper Engine property visibility conditions such as:
 *   "schemecolor.value == true"
 *   "weather_mode.value == 2 && rain_intensity.value > 0.5"
 *   "!disable_particles.value"
 * without using eval() or new Function().
 */
export function evaluatePropertyCondition(
  condition: string | undefined,
  currentValues: Record<string, string | number | boolean>
): boolean {
  if (!condition || condition.trim() === '') return true;

  // Split by logical OR (||) first
  const orClauses = condition.split('||');
  return orClauses.some((orClause) => {
    // Split by logical AND (&&)
    const andClauses = orClause.split('&&');
    return andClauses.every((rawAtom) => {
      let atom = rawAtom.trim();
      // Strip outer parentheses if present
      while (atom.startsWith('(') && atom.endsWith(')')) {
        atom = atom.slice(1, -1).trim();
      }
      if (atom === 'true' || atom === '1') return true;
      if (atom === 'false' || atom === '0') return false;

      // Match comparison operators: ==, !=, >=, <=, >, <
      const match = atom.match(
        /^(!?)([a-zA-Z0-9_]+)(?:\.value)?\s*(?:(==|!=|>=|<=|>|<)\s*(.+))?$/
      );
      if (!match) return true; // Fail open so controls are never accidentally hidden

      const [, negated, propKey, op, rhsRaw] = match;
      const lhsVal = currentValues[propKey];

      if (!op) {
        const truthy = Boolean(lhsVal) && lhsVal !== 'false' && lhsVal !== 0;
        return negated === '!' ? !truthy : truthy;
      }

      const rhsClean = rhsRaw.trim().replace(/^['"]|['"]$/g, '');
      let rhsVal: string | number | boolean = rhsClean;
      if (rhsClean === 'true') rhsVal = true;
      else if (rhsClean === 'false') rhsVal = false;
      else if (!isNaN(Number(rhsClean)) && rhsClean !== '') rhsVal = Number(rhsClean);

      let result = false;
      switch (op) {
        case '==':
          result = String(lhsVal) === String(rhsVal);
          break;
        case '!=':
          result = String(lhsVal) !== String(rhsVal);
          break;
        case '>=':
          result = Number(lhsVal) >= Number(rhsVal);
          break;
        case '<=':
          result = Number(lhsVal) <= Number(rhsVal);
          break;
        case '>':
          result = Number(lhsVal) > Number(rhsVal);
          break;
        case '<':
          result = Number(lhsVal) < Number(rhsVal);
          break;
      }
      return negated === '!' ? !result : result;
    });
  });
}

/**
 * Resolves localized labels (e.g. "ui_browse_properties_scheme_color") from project.json.
 */
export function resolveLocalizedText(
  rawText: string | undefined,
  project: ProjectJsonSchema,
  locale = 'en-us'
): string {
  if (!rawText) return '';
  const locTable =
    project.general?.localization?.[locale] ??
    project.general?.localization?.['en-us'];
  if (locTable && locTable[rawText]) {
    return locTable[rawText];
  }
  return rawText.replace(/<[^>]+>/g, ''); // Strip basic HTML tags often used in WE labels
}

interface PropertyInspectorProps {
  wallpaperId: string;
  project: ProjectJsonSchema;
  values: Record<string, string | number | boolean>;
  adjustments: UniversalAdjustments;
  presets: WallpaperPreset[];
  onPropertyChange: (key: string, value: string | number | boolean) => void;
  onAdjustmentChange: (next: UniversalAdjustments) => void;
}

export const PropertyInspector: React.FC<PropertyInspectorProps> = ({
  wallpaperId,
  project,
  values,
  adjustments,
  onPropertyChange,
  onAdjustmentChange,
}) => {
  const sortedProperties = useMemo(() => {
    const propsMap = project.general?.properties ?? {};
    return Object.entries(propsMap)
      .map(([key, prop]) => ({ key, ...prop }))
      .sort((a, b) => (a.order ?? 999) - (b.order ?? 999));
  }, [project]);

  const handleChange = async (key: string, val: string | number | boolean) => {
    onPropertyChange(key, val);
    await invoke('monitor_update_property', {
      wallpaperId,
      propKey: key,
      propValue: val,
    });
  };

  return (
    <aside className="panel" style={{ padding: 20, overflowY: 'auto' }}>
      <h3 style={{ margin: '0 0 12px', fontSize: 16, fontWeight: 600 }}>
        {project.title}
      </h3>

      {/* Universal Color & Audio Adjustments */}
      <section style={{ marginBottom: 24 }}>
        <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginBottom: 8 }}>
          UNIVERSAL PLAYBACK & AUDIO
        </div>
        <label style={{ display: 'block', fontSize: 13, marginBottom: 10 }}>
          <div style={{ display: 'flex', justifyContent: 'space-between' }}>
            <span>Master Volume</span>
            <span>{Math.round(adjustments.volume * 100)}%</span>
          </div>
          <input
            type="range"
            className="slider"
            min={0}
            max={1}
            step={0.01}
            value={adjustments.volume}
            onChange={(e) => {
              const next = { ...adjustments, volume: Number(e.target.value) };
              onAdjustmentChange(next);
              void invoke('monitor_update_adjustments', { wallpaperId, adjustments: next });
            }}
          />
        </label>
        <label style={{ display: 'block', fontSize: 13, marginBottom: 10 }}>
          <div style={{ display: 'flex', justifyContent: 'space-between' }}>
            <span>Playback Speed</span>
            <span>{adjustments.playbackSpeed.toFixed(2)}x</span>
          </div>
          <input
            type="range"
            className="slider"
            min={0.1}
            max={3.0}
            step={0.05}
            value={adjustments.playbackSpeed}
            onChange={(e) => {
              const next = { ...adjustments, playbackSpeed: Number(e.target.value) };
              onAdjustmentChange(next);
              void invoke('monitor_update_adjustments', { wallpaperId, adjustments: next });
            }}
          />
        </label>
      </section>

      {/* Auto-Generated project.json Properties */}
      <section>
        <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginBottom: 12 }}>
          WALLPAPER PROPERTIES
        </div>
        {sortedProperties.map((prop) => {
          const isVisible = evaluatePropertyCondition(prop.condition, values);
          if (!isVisible) return null;

          const label = resolveLocalizedText(prop.text, project) || prop.key;
          const currentVal = values[prop.key] ?? prop.value;

          if (prop.type === 'group') {
            return (
              <div
                key={prop.key}
                style={{
                  marginTop: 16,
                  marginBottom: 8,
                  fontSize: 12,
                  fontWeight: 600,
                  textTransform: 'uppercase',
                  letterSpacing: '0.04em',
                  color: 'var(--accent-cyan)',
                }}
              >
                {label}
              </div>
            );
          }

          if (prop.type === 'slider') {
            const min = prop.min ?? 0;
            const max = prop.max ?? 100;
            const step = prop.step ?? (prop.fraction ? (max - min) / 100 : 1);
            return (
              <div key={prop.key} style={{ marginBottom: 14 }}>
                <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: 13 }}>
                  <span>{label}</span>
                  <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                    {Number(currentVal).toFixed(prop.fraction ? 2 : 0)}
                  </span>
                </div>
                <input
                  type="range"
                  className="slider"
                  min={min}
                  max={max}
                  step={step}
                  value={Number(currentVal)}
                  onChange={(e) => void handleChange(prop.key, Number(e.target.value))}
                />
              </div>
            );
          }

          if (prop.type === 'bool') {
            const checked = Boolean(currentVal) && currentVal !== 'false';
            return (
              <div
                key={prop.key}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  marginBottom: 12,
                  fontSize: 13,
                }}
              >
                <span>{label}</span>
                <input
                  type="checkbox"
                  checked={checked}
                  onChange={(e) => void handleChange(prop.key, e.target.checked)}
                />
              </div>
            );
          }

          if (prop.type === 'color') {
            const hexVal = weColorToHex(String(currentVal));
            return (
              <div
                key={prop.key}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  marginBottom: 12,
                  fontSize: 13,
                }}
              >
                <span>{label}</span>
                <input
                  type="color"
                  value={hexVal}
                  onChange={(e) => void handleChange(prop.key, hexToWeColor(e.target.value))}
                />
              </div>
            );
          }

          if (prop.type === 'combo') {
            return (
              <div key={prop.key} style={{ marginBottom: 14 }}>
                <div style={{ fontSize: 13, marginBottom: 4 }}>{label}</div>
                <select
                  className="button"
                  style={{ width: '100%' }}
                  value={String(currentVal)}
                  onChange={(e) => void handleChange(prop.key, e.target.value)}
                >
                  {(prop.options ?? []).map((opt) => (
                    <option key={String(opt.value)} value={String(opt.value)}>
                      {resolveLocalizedText(opt.label, project)}
                    </option>
                  ))}
                </select>
              </div>
            );
          }

          if (prop.type === 'file' || prop.type === 'scenetexture' || prop.type === 'directory') {
            return (
              <div key={prop.key} style={{ marginBottom: 14 }}>
                <div style={{ fontSize: 13, marginBottom: 4 }}>{label}</div>
                <button
                  type="button"
                  className="button"
                  onClick={async () => {
                    const selected = await open({
                      directory: prop.type === 'directory',
                      multiple: false,
                    });
                    if (typeof selected === 'string') {
                      await handleChange(prop.key, selected);
                    }
                  }}
                >
                  {currentVal ? String(currentVal).split(/[\\/]/).pop() : 'Choose File...'}
                </button>
              </div>
            );
          }

          return (
            <div key={prop.key} style={{ marginBottom: 14 }}>
              <div style={{ fontSize: 13, marginBottom: 4 }}>{label}</div>
              <input
                type="text"
                className="button"
                style={{ width: '100%', textAlign: 'left' }}
                value={String(currentVal ?? '')}
                onChange={(e) => void handleChange(prop.key, e.target.value)}
              />
            </div>
          );
        })}
      </section>
    </aside>
  );
};
