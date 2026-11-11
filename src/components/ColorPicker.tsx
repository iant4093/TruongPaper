import { useEffect, useId, useRef, useState } from 'react';

type Hsl = { hue: number; saturation: number; lightness: number };

export function normalizeHex(input: string): string | null {
  const hex = input.trim().replace(/^#/, '');
  if (/^[0-9a-f]{3}$/i.test(hex)) {
    return `#${[...hex].map((digit) => digit.repeat(2)).join('').toUpperCase()}`;
  }
  return /^[0-9a-f]{6}$/i.test(hex) ? `#${hex.toUpperCase()}` : null;
}

function hexToHsl(hex: string): Hsl {
  const [red, green, blue] = [1, 3, 5].map((offset) => parseInt(hex.slice(offset, offset + 2), 16) / 255);
  const max = Math.max(red, green, blue);
  const min = Math.min(red, green, blue);
  const delta = max - min;
  const lightness = (max + min) / 2;
  if (delta === 0) return { hue: 0, saturation: 0, lightness: lightness * 100 };
  const sector = max === red ? (green - blue) / delta
    : max === green ? (blue - red) / delta + 2
      : (red - green) / delta + 4;
  return {
    hue: (sector * 60 + 360) % 360,
    saturation: delta / (1 - Math.abs(2 * lightness - 1)) * 100,
    lightness: lightness * 100,
  };
}

function hslToHex({ hue, saturation, lightness }: Hsl): string {
  const light = lightness / 100;
  const chroma = (1 - Math.abs(2 * light - 1)) * saturation / 100;
  const sector = hue / 60;
  const channel = (offset: number) => {
    const position = (offset + sector) % 12;
    const value = light - chroma / 2 * Math.max(-1, Math.min(position - 3, 9 - position, 1));
    return Math.round(value * 255).toString(16).padStart(2, '0');
  };
  return `#${channel(0)}${channel(8)}${channel(4)}`.toUpperCase();
}

export function ColorPicker({ label, value, onChange }: {
  label: string;
  value: string;
  onChange: (hex: string) => void;
}) {
  const id = useId();
  const current = normalizeHex(value) ?? '#FFFFFF';
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(current);
  const [hsl, setHsl] = useState(() => hexToHsl(current));
  const panel = useRef<HTMLFormElement>(null);
  const valid = normalizeHex(draft);
  const preview = valid ?? hslToHex(hsl);
  useEffect(() => {
    if (open) panel.current?.scrollIntoView?.({ block: 'nearest' });
  }, [open]);
  const setChannel = (channel: keyof Hsl, value: number) => {
    const next = { ...hsl, [channel]: value };
    setHsl(next);
    setDraft(hslToHex(next));
  };
  const setHex = (value: string) => {
    setDraft(value);
    const hex = normalizeHex(value);
    if (hex) setHsl(hexToHsl(hex));
  };
  const close = () => {
    setOpen(false);
    document.getElementById(`${id}-trigger`)?.focus();
  };

  return (
    <div className="property-field">
      <span id={`${id}-label`}>{label}</span>
      <button
        type="button"
        className="property-color button"
        id={`${id}-trigger`}
        aria-labelledby={`${id}-label`}
        aria-expanded={open}
        aria-controls={`${id}-picker`}
        onClick={() => {
          if (open) close();
          else {
            setHex(current);
            setOpen(true);
          }
        }}
      >
        <span className="color-picker-swatch" style={{ background: open ? preview : current }} />
        <span className="property-value">{open ? preview : current}</span>
      </button>
      {open && (
        <form
          ref={panel}
          className="color-picker"
          id={`${id}-picker`}
          aria-labelledby={`${id}-label`}
          onSubmit={(event) => {
            event.preventDefault();
            if (!valid) return;
            if (valid !== current) onChange(valid);
            close();
          }}
          onKeyDown={(event) => {
            if (event.key === 'Escape') {
              event.stopPropagation();
              close();
            }
          }}
        >
          <label className="color-picker-channel">
            Hue
            <input type="range" min={0} max={359} value={Math.floor(hsl.hue)}
              style={{ background: 'linear-gradient(to right, red, yellow, lime, cyan, blue, magenta, red)' }}
              onChange={(event) => setChannel('hue', Number(event.target.value))} />
          </label>
          <label className="color-picker-channel">
            Saturation
            <input type="range" min={0} max={100} value={Math.round(hsl.saturation)}
              style={{ background: `linear-gradient(to right, hsl(${hsl.hue} 0% ${hsl.lightness}%), hsl(${hsl.hue} 100% ${hsl.lightness}%))` }}
              onChange={(event) => setChannel('saturation', Number(event.target.value))} />
          </label>
          <label className="color-picker-channel">
            Lightness
            <input type="range" min={0} max={100} value={Math.round(hsl.lightness)}
              style={{ background: `linear-gradient(to right, black, hsl(${hsl.hue} ${hsl.saturation}% 50%), white)` }}
              onChange={(event) => setChannel('lightness', Number(event.target.value))} />
          </label>
          <label className="color-picker-hex">
            Hex
            <input type="text" className="button" value={draft} maxLength={7} spellCheck={false}
              aria-invalid={!valid} onChange={(event) => setHex(event.target.value)} />
          </label>
          <div className="color-picker-actions">
            <button type="button" className="button" onClick={close}>Cancel</button>
            <button type="submit" className="button button-primary" disabled={!valid}>Apply</button>
          </div>
        </form>
      )}
    </div>
  );
}
