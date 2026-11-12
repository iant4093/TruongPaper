// The Workshop page has one import control: the "Import from computer…" menu. Links and IDs
// are pasted into the Discover search box, so there is no second link field and no global paste handler.
import React from 'react';
import { toast } from '../store/feedback';
import { LocalImport } from './LocalImport';

export const WorkshopImport: React.FC = () => (
  <LocalImport onStatus={(m) => { if (m && !/^Copying/.test(m)) toast.info(m); }} />
);
