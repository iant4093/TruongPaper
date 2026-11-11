type ImportTarget =
  | { kind: 'steam_workshop'; workshopId: string }
  | { kind: 'photo_album'; albumSlug: string };

export function parseImportLink(raw: string): ImportTarget | null {
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
