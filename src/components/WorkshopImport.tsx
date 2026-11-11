import React, { useCallback, useEffect, useState } from 'react';
import { Link2, Plus } from 'lucide-react';
import { parseImportLink } from '../lib/importLinks';
import { useAppStore } from '../store/useAppStore';

export const WorkshopImport: React.FC = () => {
  const [input, setInput] = useState('');
  const [statusMsg, setStatusMsg] = useState<string | null>(null);
  const inspectWorkshopUrl = useAppStore((s) => s.inspectWorkshopUrl);
  const openSteamAuthModal = useAppStore((s) => s.openSteamAuthModal);

  const handleResolvedInput = useCallback(async (rawText: string) => {
    const parsed = parseImportLink(rawText);
    if (parsed?.kind !== 'steam_workshop') {
      setStatusMsg('Enter a Steam Workshop link or item ID.');
      return;
    }

    try {
      setStatusMsg(`Loading Workshop item #${parsed.workshopId}…`);
      await inspectWorkshopUrl(parsed.workshopId);
      setStatusMsg(null);
      setInput('');
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      if (/^(sign in to steam|steam login required)/i.test(msg)) {
        openSteamAuthModal();
      }
      setStatusMsg(msg);
    }
  }, [inspectWorkshopUrl, openSteamAuthModal]);

  useEffect(() => {
    const onWindowPaste = (e: ClipboardEvent) => {
      const active = document.activeElement;
      if (
        active instanceof HTMLInputElement ||
        active instanceof HTMLTextAreaElement ||
        (active instanceof HTMLElement && active.isContentEditable)
      ) {
        return;
      }
      const pasted = e.clipboardData?.getData('text/plain')?.trim();
      if (pasted && parseImportLink(pasted)?.kind === 'steam_workshop') {
        e.preventDefault();
        setInput(pasted);
        void handleResolvedInput(pasted);
      }
    };

    window.addEventListener('paste', onWindowPaste);
    return () => window.removeEventListener('paste', onWindowPaste);
  }, [handleResolvedInput]);

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        void handleResolvedInput(input);
      }}
      className="omnibar"
    >
      <div className="omnibar-field">
        <Link2 size={15} aria-hidden="true" />
        <input type="text" aria-label="Steam Workshop link or item ID" value={input} onChange={(e) => setInput(e.target.value)} placeholder="Paste a Steam Workshop link or item ID…" />
      </div>
      <button type="submit" className="button"><Plus size={14} />Import</button>
      {statusMsg && (
        <span className="omnibar-status" role="status">
          {statusMsg}
        </span>
      )}
    </form>
  );
};
