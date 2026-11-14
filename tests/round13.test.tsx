// @vitest-environment jsdom
// tests/round13.test.tsx — modal focus/Escape layering, settings rollback, cancel result.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import React, { useRef } from 'react';

const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
  convertFileSrc: (p: string) => p,
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => vi.fn()), emit: vi.fn(async () => undefined) }));

import { useModalFocus } from '../src/lib/useModalFocus';
import { FeedbackHost } from '../src/components/Feedback';
import { confirmAction } from '../src/store/feedback';
import { useAppStore } from '../src/store/useAppStore';

beforeEach(() => invokeMock.mockReset());
afterEach(() => cleanup());

const Dialog: React.FC<{ onClose: () => void; busy?: boolean }> = ({ onClose, busy }) => {
  const ref = useRef<HTMLDivElement>(null);
  useModalFocus(true, ref, onClose);
  return (
    <div ref={ref} role="dialog">
      <button disabled={busy}>first</button>
      <button>middle</button>
      <button disabled={busy}>last</button>
    </div>
  );
};

describe('useModalFocus', () => {
  it('skips disabled buttons when wrapping Tab', () => {
    const { getByText } = render(<Dialog onClose={() => {}} busy />);
    expect(document.activeElement).toBe(getByText('middle'));
    fireEvent.keyDown(document, { key: 'Tab' });
    expect(document.activeElement).toBe(getByText('middle'));
  });

  it('pulls focus back in when it was dropped to the body', () => {
    const { getByText } = render(<Dialog onClose={() => {}} />);
    (document.activeElement as HTMLElement).blur();
    fireEvent.keyDown(document, { key: 'Tab' });
    expect(document.activeElement).toBe(getByText('first'));
  });

  it('one Escape closes only the confirm dialog opened over a modal', async () => {
    const onClose = vi.fn();
    render(<><Dialog onClose={onClose} /><FeedbackHost /></>);
    let answer: Promise<boolean> | undefined;
    act(() => { answer = confirmAction('Sure?', 'Yes'); });
    fireEvent.keyDown(document, { key: 'Escape' });
    await expect(answer).resolves.toBe(false);
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe('store', () => {
  it('rolls the optimistic setting back when the write and the re-read both fail', async () => {
    useAppStore.setState({ settings: { fpsCap: 30 } as never });
    invokeMock.mockImplementation(async (cmd: string) => { if (String(cmd).startsWith('settings_')) throw new Error('down'); return null; });
    await expect(useAppStore.getState().updateSettings({ fpsCap: 99 } as never)).rejects.toThrow('down');
    expect(useAppStore.getState().settings).toMatchObject({ fpsCap: 30 });
  });

  it('cancelWorkshopDownload reports whether a download was found', async () => {
    invokeMock.mockResolvedValueOnce(false);
    await expect(useAppStore.getState().cancelWorkshopDownload('1')).resolves.toBe(false);
    invokeMock.mockResolvedValueOnce(true);
    await expect(useAppStore.getState().cancelWorkshopDownload('1')).resolves.toBe(true);
  });
});
