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
  { name: 'AMD Radeon(TM) Graphics', kind: 'integrated', inUse: true },
  { name: 'NVIDIA GeForce RTX 4080', kind: 'dedicated', inUse: false },
];

describe('GpuPicker', () => {
  it('defaults to the integrated GPU and shows the one in use', async () => {
    invokeMock.mockResolvedValue(twoGpus);
    await act(async () => { render(<GpuPicker value="integrated" onChange={vi.fn()} />); });
    expect(screen.getByRole('combobox', { name: 'Graphics card' }).textContent).toContain('Integrated GPU (recommended)');
    expect(screen.getByRole('status').textContent).toBe('In use: AMD Radeon(TM) Graphics');
  });

  it('lets the user pick the dedicated GPU or one by name', async () => {
    invokeMock.mockResolvedValue(twoGpus);
    const onChange = vi.fn();
    await act(async () => { render(<GpuPicker value="integrated" onChange={onChange} />); });
    await act(async () => { fireEvent.click(screen.getByRole('combobox', { name: 'Graphics card' })); });
    const names = screen.getAllByRole('option').map((o) => o.textContent);
    expect(names).toEqual([
      'Integrated GPU (recommended)',
      'Dedicated GPU',
      'AMD Radeon(TM) Graphics (integrated)',
      'NVIDIA GeForce RTX 4080 (dedicated)',
    ]);
    await act(async () => { fireEvent.click(screen.getByRole('option', { name: 'NVIDIA GeForce RTX 4080 (dedicated)' })); });
    expect(onChange).toHaveBeenCalledWith('adapter:NVIDIA GeForce RTX 4080');
  });

  it('says a one-GPU Mac always uses its GPU', async () => {
    invokeMock.mockResolvedValue([{ name: 'Apple M3 Max', kind: 'integrated', inUse: true }]);
    await act(async () => { render(<GpuPicker value="integrated" onChange={vi.fn()} />); });
    expect(screen.getByText(/has one GPU, so it is always used/)).toBeTruthy();
  });
});
