// src/lib/escapeStack.ts
// Layers (modals, confirm dialogs) that close on Escape. Only the newest layer reacts, so one
// key press never closes two of them, whatever order their document listeners were added in.
const layers: symbol[] = [];

/** Registers a layer and returns its token. */
export const pushEscapeLayer = (): symbol => { const token = Symbol('escape-layer'); layers.push(token); return token; };
export const popEscapeLayer = (token: symbol) => { const i = layers.indexOf(token); if (i >= 0) layers.splice(i, 1); };
export const isTopEscapeLayer = (token: symbol) => layers[layers.length - 1] === token;
