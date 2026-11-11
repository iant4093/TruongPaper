// @vitest-environment jsdom
// tests/propertyInspectorComponent.test.tsx — dynamic project.json controls (§3)
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';

const invokeMock = vi.fn(async (_cmd: string, _args?: unknown): Promise<unknown> => null);
const dialogOpenMock = vi.fn(async (_opts: unknown): Promise<string | null> => '/Users/ian/Pictures/logo.png');
vi.mock('@tauri-apps/api/core', () => ({ invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: (opts: unknown) => dialogOpenMock(opts) }));

import {
  PropertyInspector,
  evaluatePropertyCondition,
  hexToWeColor,
  weColorToHex,
} from '../src/components/PropertyInspector';
import type { ProjectJsonSchema, UniversalAdjustments } from '../src/types/ipc';

const adjustments: UniversalAdjustments = {
  volume: 0.5, muted: false, audioSensitivity: 1, playbackSpeed: 1, fitMode: 'fill', zoom: 1,
  offsetX: 0, offsetY: 0, brightness: 0, contrast: 1, saturation: 1, hueShift: 0,
};

const project: ProjectJsonSchema = {
  title: 'Neon Rain',
  type: 'scene',
  file: 'scene.json',
  general: {
    localization: { 'en-us': { ui_rain: 'Rain Speed', ui_clear: 'Clear', ui_storm: 'Storm' } },
    properties: {
      zgroup: { type: 'group', text: '<b>Weather</b>', order: 0 },
      enable_rain: { type: 'bool', text: 'Enable Rain', order: 1, value: true },
      rain_speed: { type: 'slider', text: 'ui_rain', order: 2, min: 0, max: 2, fraction: true, value: 1, condition: 'enable_rain.value == true' },
      mode: { type: 'combo', text: 'Mode', order: 3, value: 0, options: [{ label: 'ui_clear', value: 0 }, { label: 'ui_storm', value: 2 }] },
      color: { type: 'color', text: 'ui_browse_properties_scheme_color', order: 4, value: '1 0 0' },
      logo: { type: 'file', text: 'Logo', order: 5 },
      caption: { type: 'text', text: 'Caption', order: 6, value: 'hi', condition: 'mode.value == 2' },
    },
  },
};

beforeEach(() => invokeMock.mockClear());
afterEach(cleanup);

function renderInspector(values: Record<string, string | number | boolean>) {
  const onPropertyChange = vi.fn();
  const onAdjustmentChange = vi.fn();
  render(
    <PropertyInspector
      wallpaperId="workshop:1"
      project={project}
      values={values}
      adjustments={adjustments}
      presets={[]}
      onPropertyChange={onPropertyChange}
      onAdjustmentChange={onAdjustmentChange}
    />
  );
  return { onPropertyChange, onAdjustmentChange };
}

describe('<PropertyInspector />', () => {
  it('renders localized, ordered controls and hides properties whose condition is false', () => {
    renderInspector({ enable_rain: true, mode: 0 });
    expect(screen.getByText('Weather')).toBeTruthy(); // HTML stripped from group label
    expect(screen.getByText('Rain Speed')).toBeTruthy(); // localized slider label
    expect(screen.getByRole('option', { name: 'Storm' })).toBeTruthy(); // localized combo option
    expect(screen.queryByText('Caption')).toBeNull(); // mode != 2
    expect(screen.getByRole('button', { name: 'Scheme Color' })).toBeTruthy();
    expect(screen.getByText('#FF0000')).toBeTruthy();
  });

  it('hides dependent controls when the controlling bool is off', () => {
    renderInspector({ enable_rain: false, mode: 2 });
    expect(screen.queryByText('Rain Speed')).toBeNull();
    expect(screen.getByText('Caption')).toBeTruthy();
  });

  it('applies a color from the in-app picker as one WE-encoded update', () => {
    const { onPropertyChange } = renderInspector({ enable_rain: true, mode: 0 });
    fireEvent.click(screen.getByRole('button', { name: 'Scheme Color' }));
    expect(screen.getByRole('slider', { name: 'Hue' })).toBeTruthy();
    fireEvent.change(screen.getByRole('textbox', { name: 'Hex' }), { target: { value: '#2673d9' } });
    expect(onPropertyChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Apply' }));
    expect(onPropertyChange).toHaveBeenCalledWith('color', '0.14902 0.45098 0.85098');
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith('monitor_update_property', {
      wallpaperId: 'workshop:1',
      propKey: 'color',
      propValue: '0.14902 0.45098 0.85098',
    });
  });

  it('sends the full adjustments object on volume change', () => {
    const { onAdjustmentChange } = renderInspector({ enable_rain: true });
    const volume = document.querySelectorAll('input[type="range"]')[0];
    fireEvent.change(volume, { target: { value: '0.8' } });
    expect(onAdjustmentChange).toHaveBeenCalledWith({ ...adjustments, volume: 0.8 });
    expect(invokeMock).toHaveBeenCalledWith('monitor_update_adjustments', {
      wallpaperId: 'workshop:1',
      adjustments: { ...adjustments, volume: 0.8 },
    });
  });

  it('opens the native file dialog for file properties', async () => {
    const { onPropertyChange } = renderInspector({ enable_rain: true });
    fireEvent.click(screen.getByText('Choose File...'));
    await vi.waitFor(() => expect(onPropertyChange).toHaveBeenCalledWith('logo', '/Users/ian/Pictures/logo.png'));
    expect(dialogOpenMock).toHaveBeenCalledWith({ directory: false, multiple: false });
  });
});

describe('evaluatePropertyCondition edge cases', () => {
  const v = { a: 1, b: 'x', c: true, z: 0 };
  it.each([
    ['(a.value == 1)', true],
    ['a.value >= 1 && a.value <= 1', true],
    ['a.value < 1 || c.value', true],
    ["b.value == 'x'", true],
    ['b.value != "x"', false],
    ['z.value', false],
    ['!z.value', true],
    ['missing.value', false],
    ['true', true],
    ['0', false],
    ['some weird ++ expression', true], // fails open
  ])('%s → %s', (cond, expected) => {
    expect(evaluatePropertyCondition(cond, v)).toBe(expected);
  });
});

describe('color conversion edge cases', () => {
  it('clamps and rejects malformed inputs', () => {
    expect(weColorToHex('2 0 0')).toBe('#020000'); // >1 → treated as 0..255 bytes
    expect(weColorToHex('a b c')).toBe('#ffffff');
    expect(weColorToHex('0.5 0.5')).toBe('#ffffff');
    expect(hexToWeColor('#fff')).toBe('1.00000 1.00000 1.00000');
    expect(hexToWeColor('000000')).toBe('0.00000 0.00000 0.00000');
  });
});
