// src/components/UniversalOmnibar.tsx
import React, { useEffect, useState } from 'react';
import { Link2, Plus } from 'lucide-react';
import { useAppStore } from '../store/useAppStore';

export type ParsedOmnibarTarget =
  | { kind: 'steam_workshop'; workshopId: string }
  | { kind: 'photo_album'; albumSlug: string }
  | null;

/**
 * Classifies any pasted URL, Workshop numeric ID, or Ian Truong Photography album slug.
 */
export function parseOmnibarInput(raw: string): ParsedOmnibarTarget {
  const trimmed = raw.trim();
  if (!trimmed) return null;

  const steamUrlMatch =
    trimmed.match(/steamcommunity\.com\/(?:sharedfiles|workshop)\/filedetails\/?\?.*?\bid=(\d{6,12})/i) ??
    trimmed.match(/steam:\/\/url\/CommunityFilePage\/(\d{6,12})/i);
  if (steamUrlMatch) {
    return { kind: 'steam_workshop', workshopId: steamUrlMatch[1] };
  }

  if (/^\d{7,12}$/.test(trimmed)) {
    return { kind: 'steam_workshop', workshopId: trimmed };
  }

  const albumUrlMatch = trimmed.match(
    /(?:https?:\/\/)?(?:www\.)?iantruongphotography\.com\/album\/([a-z0-9-]+)\/?/i
  );
  if (albumUrlMatch) {
    return { kind: 'photo_album', albumSlug: albumUrlMatch[1].toLowerCase() };
  }

  if (/^[a-z0-9]+(?:-[a-z0-9]+)+$/i.test(trimmed)) {
    return { kind: 'photo_album', albumSlug: trimmed.toLowerCase() };
  }

  return null;
}

export const UniversalOmnibar: React.FC = () => {
  const [input, setInput] = useState('');
  const [statusMsg, setStatusMsg] = useState<string | null>(null);
  const inspectWorkshopUrl = useAppStore((s) => s.inspectWorkshopUrl);
  const addAlbum = useAppStore((s) => s.addAlbum);
  const openSteamAuthModal = useAppStore((s) => s.openSteamAuthModal);

  const handleResolvedInput = async (rawText: string) => {
    const parsed = parseOmnibarInput(rawText);
    if (!parsed) {
      setStatusMsg('Paste a valid Steam Workshop URL/ID or iantruongphotography.com album link/slug');
      return;
    }

    try {
      if (parsed.kind === 'steam_workshop') {
        setStatusMsg(`Inspecting Steam Workshop #${parsed.workshopId}...`);
        await inspectWorkshopUrl(parsed.workshopId);
        setStatusMsg(null);
        setInput('');
      } else {
        setStatusMsg(`Adding album "${parsed.albumSlug}"...`);
        const saved = await addAlbum(parsed.albumSlug);
        setStatusMsg(`Added album "${saved.displayName}" to Saved Albums`);
        setInput('');
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      if (parsed.kind === 'steam_workshop' && /^(sign in to steam|steam login required)/i.test(msg)) {
        openSteamAuthModal();
      }
      setStatusMsg(msg);
    }
  };

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
      if (pasted && parseOmnibarInput(pasted)) {
        e.preventDefault();
        setInput(pasted);
        void handleResolvedInput(pasted);
      }
    };

    window.addEventListener('paste', onWindowPaste);
    return () => window.removeEventListener('paste', onWindowPaste);
  }, []);

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
        <input type="text" aria-label="Import a Workshop wallpaper or photo album" value={input} onChange={(e) => setInput(e.target.value)} placeholder="Paste Steam Workshop URL or photo album link…" />
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
