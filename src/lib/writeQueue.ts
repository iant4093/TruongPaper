// src/lib/writeQueue.ts
// Debounced, last-write-wins backend writes for slider-style controls. The UI updates its
// local state on every tick; only the newest write per key is sent, after a quiet period.

type Waiter = { resolve: () => void; reject: (error: unknown) => void };
interface Entry {
  timer: ReturnType<typeof setTimeout>;
  write: () => Promise<unknown>;
  waiters: Waiter[];
}

export class WriteQueue {
  private pending = new Map<string, Entry>();
  /** Bumped on every schedule(), so a slow older write cannot report a stale failure. */
  private sequence = new Map<string, number>();

  constructor(private readonly delayMs = 150) {}

  /**
   * Replace any pending write for `key`. The promise settles when the final coalesced
   * write finishes; a failure of a write that has since been superseded resolves quietly.
   */
  schedule(key: string, write: () => Promise<unknown>): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const previous = this.pending.get(key);
      if (previous) clearTimeout(previous.timer);
      this.sequence.set(key, (this.sequence.get(key) ?? 0) + 1);
      this.pending.set(key, {
        timer: setTimeout(() => this.run(key), this.delayMs),
        write,
        waiters: [...(previous?.waiters ?? []), { resolve, reject }],
      });
    });
  }

  /** Send every pending write whose key starts with `prefix` now (all of them when omitted). */
  flush(prefix = ''): void {
    for (const key of [...this.pending.keys()]) {
      if (key.startsWith(prefix)) this.run(key);
    }
  }

  hasPending(prefix = ''): boolean {
    return [...this.pending.keys()].some((key) => key.startsWith(prefix));
  }

  private run(key: string): void {
    const entry = this.pending.get(key);
    if (!entry) return;
    clearTimeout(entry.timer);
    this.pending.delete(key);
    const seq = this.sequence.get(key);
    let result: Promise<unknown>;
    try {
      result = Promise.resolve(entry.write());
    } catch (error) {
      result = Promise.reject(error);
    }
    result.then(
      () => entry.waiters.forEach((w) => w.resolve()),
      (error) => {
        const superseded = this.sequence.get(key) !== seq;
        entry.waiters.forEach((w) => (superseded ? w.resolve() : w.reject(error)));
      },
    );
  }
}

/** Shared by the store and the property inspector so both coalesce into the same slot. */
export const wallpaperWrites = new WriteQueue(150);
export const propertyWriteKey = (wallpaperId: string, propKey: string) => `prop:${wallpaperId}:${propKey}`;
export const adjustmentWriteKey = (wallpaperId: string) => `adj:${wallpaperId}`;
