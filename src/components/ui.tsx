// src/components/ui.tsx
// Small shared Liquid Glass building blocks used by the studio views.
import React, { useState } from 'react';
import { toAssetUrl } from '../lib/format';

export const ViewHeader: React.FC<{ title: string; subtitle?: string; actions?: React.ReactNode }> = ({
  title,
  subtitle,
  actions,
}) => (
  <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 12, marginBottom: 16 }}>
    <div>
      <h2 style={{ margin: 0, fontSize: 20, letterSpacing: '-0.01em' }}>{title}</h2>
      {subtitle && <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 4 }}>{subtitle}</div>}
    </div>
    {actions && <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', justifyContent: 'flex-end' }}>{actions}</div>}
  </div>
);

export const SectionTitle: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div
    style={{
      fontSize: 12,
      color: 'var(--text-secondary)',
      textTransform: 'uppercase',
      letterSpacing: '0.04em',
      margin: '20px 0 10px',
    }}
  >
    {children}
  </div>
);

export const Badge: React.FC<{ children: React.ReactNode; tone?: 'cyan' | 'muted' | 'warn' }> = ({
  children,
  tone = 'cyan',
}) => (
  <span
    style={{
      display: 'inline-block',
      fontSize: 10,
      fontWeight: 600,
      letterSpacing: '0.05em',
      textTransform: 'uppercase',
      padding: '2px 8px',
      borderRadius: 999,
      background: 'rgba(255,255,255,0.12)',
      color:
        tone === 'cyan' ? 'var(--accent-cyan)' : tone === 'warn' ? '#fbbf24' : 'var(--text-secondary)',
    }}
  >
    {children}
  </span>
);

/** Thumbnail with an aurora gradient fallback when the image is missing or fails to load. */
export const Thumb: React.FC<{ src: string | null | undefined; alt: string; height?: number }> = ({
  src,
  alt,
  height = 120,
}) => {
  const [failed, setFailed] = useState(false);
  const url = toAssetUrl(src);
  return (
    <div
      style={{
        height,
        borderRadius: 12,
        overflow: 'hidden',
        background:
          'radial-gradient(circle at 30% 30%, rgba(56,128,255,0.45), rgba(132,68,255,0.25) 55%, rgba(9,11,18,0.9))',
      }}
    >
      {url && !failed && (
        <img
          src={url}
          alt={alt}
          draggable={false}
          onError={() => setFailed(true)}
          style={{ width: '100%', height: '100%', objectFit: 'cover', display: 'block' }}
        />
      )}
    </div>
  );
};

export const ToggleRow: React.FC<{
  label: string;
  description?: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}> = ({ label, description, checked, onChange }) => (
  <label style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12, fontSize: 13, marginBottom: 12 }}>
    <span>
      {label}
      {description && <div style={{ fontSize: 11, color: 'var(--text-muted)', marginTop: 2 }}>{description}</div>}
    </span>
    <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
  </label>
);

export function SegmentedControl<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: ReadonlyArray<readonly [T, string]>;
  onChange: (next: T) => void;
}) {
  return (
    <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
      {options.map(([key, label]) => (
        <button
          key={key}
          type="button"
          className={`glass-button ${value === key ? 'glass-button-primary' : ''}`}
          style={{ fontSize: 12, padding: '6px 12px' }}
          aria-pressed={value === key}
          onClick={() => onChange(key)}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

export const cardGrid: React.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: 'repeat(auto-fill, minmax(200px, 1fr))',
  gap: 14,
};

/** Tracks pointer position for the `.glass-card::before` specular spotlight (§1.3). */
export function spotlightHandlers() {
  return {
    onMouseMove: (e: React.MouseEvent<HTMLElement>) => {
      const rect = e.currentTarget.getBoundingClientRect();
      e.currentTarget.style.setProperty('--mouse-x', `${e.clientX - rect.left}px`);
      e.currentTarget.style.setProperty('--mouse-y', `${e.clientY - rect.top}px`);
    },
  };
}
