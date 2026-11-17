// @vitest-environment jsdom
// tests/gpuPicker.test.tsx — Settings › Graphics card picker.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
  convertFileSrc: (p: string) => p,
}));

import { GpuPicker } from '../src/components/views/SettingsView';

afterEach(() => { cleanup(); invokeMock.mockReset(); });

const twoGpus = [
  { name: 'AMD Radeon(TM) Graphics', kind: 'integrated', inUse: false, recommended: false },
  { name: 'NVIDIA GeForce RTX 4080', kind: 'dedicated', inUse: true, recommended: true },
];

describe('GpuPicker', () => {
  it('defaults to the GPU the displays use and shows the one in use', async () => {
    invokeMock.mockResolvedValue(twoGpus);
    await act(async () => { render(<GpuPicker value="auto" onChange={vi.fn()} />); });
    expect(screen.getByRole('combobox', { name: 'Graphics card' }).textContent).toContain('NVIDIA GeForce RTX 4080 (dedicated, recommended)');
    expect(screen.getByRole('status').textContent).toBe('In use: NVIDIA GeForce RTX 4080');
  });

  it('lists each GPU once', async () => {
    invokeMock.mockResolvedValue(twoGpus);
    const onChange = vi.fn();
    await act(async () => { render(<GpuPicker value="auto" onChange={onChange} />); });
    await act(async () => { fireEvent.click(screen.getByRole('combobox', { name: 'Graphics card' })); });
    const names = screen.getAllByRole('option').map((o) => o.textContent);
    expect(names).toEqual([
      'AMD Radeon(TM) Graphics (integrated)',
      'NVIDIA GeForce RTX 4080 (dedicated, recommended)',
    ]);
    await act(async () => { fireEvent.click(screen.getByRole('option', { name: 'AMD Radeon(TM) Graphics (integrated)' })); });
    expect(onChange).toHaveBeenCalledWith('adapter:AMD Radeon(TM) Graphics');
  });

  it('shows older kind and name choices as the GPU they land on and keeps a missing one visible', async () => {
    invokeMock.mockResolvedValue([...twoGpus, { name: 'Intel Arc A770', kind: 'dedicated', inUse: false, recommended: false }]);
    await act(async () => { render(<GpuPicker value="integrated" onChange={vi.fn()} />); });
    expect(screen.getByRole('combobox', { name: 'Graphics card' }).textContent).toContain('AMD Radeon(TM) Graphics (integrated)');
    cleanup();
    await act(async () => { render(<GpuPicker value="adapter:NVIDIA GeForce RTX 4080" onChange={vi.fn()} />); });
    expect(screen.getByRole('combobox', { name: 'Graphics card' }).textContent).toContain('NVIDIA GeForce RTX 4080 (dedicated, recommended)');
    cleanup();
    const onChange = vi.fn();
    await act(async () => { render(<GpuPicker value="adapter:Old GPU" onChange={onChange} />); });
    await act(async () => { fireEvent.click(screen.getByRole('combobox', { name: 'Graphics card' })); });
    expect(screen.getAllByRole('option').map((o) => o.textContent)).toEqual([
      'AMD Radeon(TM) Graphics (integrated)',
      'NVIDIA GeForce RTX 4080 (dedicated, recommended)',
      'Intel Arc A770 (dedicated)',
      'Old GPU (not found)',
    ]);
    await act(async () => { fireEvent.click(screen.getByRole('option', { name: 'NVIDIA GeForce RTX 4080 (dedicated, recommended)' })); });
    expect(onChange).toHaveBeenCalledWith('auto');
  });

  it('says a one-GPU Mac always uses its GPU', async () => {
    invokeMock.mockResolvedValue([{ name: 'Apple M3 Max', kind: 'integrated', inUse: true, recommended: true }]);
    await act(async () => { render(<GpuPicker value="auto" onChange={vi.fn()} />); });
    expect(screen.getByText(/This computer has one GPU/)).toBeTruthy();
    expect(screen.getByRole('combobox', { name: 'Graphics card' }).textContent).toContain('Apple M3 Max (integrated)');
  });
});
