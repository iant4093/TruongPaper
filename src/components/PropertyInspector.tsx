import { Dropdown } from './Dropdown';
// src/components/PropertyInspector.tsx
import React, { useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { ColorPicker, normalizeHex } from './ColorPicker';
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
  if (trimmed.startsWith('#')) return normalizeHex(trimmed)?.toLowerCase() ?? '#ffffff';
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
  const localization = project.general?.localization;
  const translated = localization?.[locale]?.[rawText] ?? localization?.['en-us']?.[rawText];
  const text = (translated ?? rawText).replace(/<[^>]+>/g, '').trim();
  if (!translated && /^ui_browse_properties_[a-z0-9_]+$/i.test(text)) {
    return text.replace(/^ui_browse_properties_/i, '').split('_').filter(Boolean)
      .map((word) => word.charAt(0).toUpperCase() + word.slice(1)).join(' ');
  }
  return text;
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
    <div className="property-inspector">
      <h3>
        {project.title}
      </h3>

      {/* Universal Color & Audio Adjustments */}
      <section>
        <h4>Playback &amp; audio</h4>
        <label className="property-field">
          <span className="property-row">
            <span>Master Volume</span>
            <span className="property-value" aria-hidden="true">{Math.round(adjustments.volume * 100)}%</span>
          </span>
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
        <label className="property-field">
          <span className="property-row">
            <span>Playback Speed</span>
            <span className="property-value" aria-hidden="true">{Math.min(2, adjustments.playbackSpeed).toFixed(2)}x</span>
          </span>
          <input
            type="range"
            className="slider"
            min={0.1}
            max={2.0}
            step={0.05}
            value={Math.min(2, adjustments.playbackSpeed)}
            onChange={(e) => {
              const next = { ...adjustments, playbackSpeed: Number(e.target.value) };
              onAdjustmentChange(next);
              void invoke('monitor_update_adjustments', { wallpaperId, adjustments: next });
            }}
          />
        </label>
      </section>

      {/* Auto-Generated project.json Properties */}
      {sortedProperties.length > 0 && <section>
        <h4>Wallpaper properties</h4>
        {sortedProperties.map((prop) => {
          const isVisible = evaluatePropertyCondition(prop.condition, values);
          if (!isVisible) return null;

          const label = resolveLocalizedText(prop.text, project) || prop.key;
          const currentVal = values[prop.key] ?? prop.value;

          if (prop.type === 'group') {
            return (
              <div key={prop.key} className="property-group">
                {label}
              </div>
            );
          }

          if (prop.type === 'slider') {
            const min = prop.min ?? 0;
            const max = prop.max ?? 100;
            const step = prop.step ?? (prop.fraction ? (max - min) / 100 : 1);
            return (
              <label key={prop.key} className="property-field">
                <span className="property-row">
                  <span>{label}</span>
                  <span className="property-value" aria-hidden="true">
                    {Number(currentVal).toFixed(prop.fraction ? 2 : 0)}
                  </span>
                </span>
                <input
                  type="range"
                  className="slider"
                  min={min}
                  max={max}
                  step={step}
                  value={Number(currentVal)}
                  onChange={(e) => void handleChange(prop.key, Number(e.target.value))}
                />
              </label>
            );
          }

          if (prop.type === 'bool') {
            const checked = Boolean(currentVal) && currentVal !== 'false';
            return (
              <label key={prop.key} className="property-field property-row">
                <span>{label}</span>
                <input
                  type="checkbox"
                  checked={checked}
                  onChange={(e) => void handleChange(prop.key, e.target.checked)}
                />
              </label>
            );
          }

          if (prop.type === 'color') {
            return (
              <ColorPicker
                key={prop.key}
                label={label}
                value={weColorToHex(String(currentVal))}
                onChange={(hex) => void handleChange(prop.key, hexToWeColor(hex))}
              />
            );
          }

          if (prop.type === 'combo') {
            return (
              <div key={prop.key} className="property-field">
                <span>{label}</span>
                <Dropdown label={label} value={String(currentVal)}
                  options={(prop.options ?? []).map((option) => [String(option.value), resolveLocalizedText(option.label, project)])}
                  onChange={(value) => void handleChange(prop.key, value)} />
              </div>
            );
          }

          if (prop.type === 'file' || prop.type === 'scenetexture' || prop.type === 'directory') {
            return (
              <div key={prop.key} className="property-field">
                <span>{label}</span>
                <button
                  type="button"
                  className="button"
                  aria-label={`Choose file for ${label}`}
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
            <label key={prop.key} className="property-field">
              <span>{label}</span>
              <input
                type="text"
                className="button"
                value={String(currentVal ?? '')}
                onChange={(e) => void handleChange(prop.key, e.target.value)}
              />
            </label>
          );
        })}
      </section>}
    </div>
  );
};
