import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import { expect, it, vi } from 'vitest';

const preload = readFileSync(new URL('../src-tauri/src/web-wallpaper-playback.js', import.meta.url), 'utf8');

function page() {
  const pending = new Map<number, (time: number) => void>();
  let next = 0;
  class Media extends EventTarget {
    paused = true;
    ended = false;
    private level = 1;
    get volume() { return this.level; }
    set volume(value) { this.level = value; }
    outputLevel() { return this.level; }
    pause() { this.paused = true; }
    async play() { this.paused = false; }
  }
  class AudioNode {
    static nativeConnect = vi.fn();
    static nativeDisconnect = vi.fn();
    constructor(readonly context: object, readonly mediaElement?: Media) {}
    connect(destination: unknown) { AudioNode.nativeConnect(destination); return destination; }
    disconnect(destination: unknown) { AudioNode.nativeDisconnect(destination); }
  }
  const playing = new Media();
  const idle = new Media();
  const animation = {
    playState: 'running',
    pause() { this.playState = 'paused'; },
    play() { this.playState = 'running'; },
  };
  const win = {
    requestAnimationFrame(callback: (time: number) => void) { pending.set(++next, callback); return next; },
    cancelAnimationFrame(handle: number) { pending.delete(handle); },
    __truongpaper_set_paused(_paused: boolean) {},
    __truongpaper_set_volume(_volume: number) {},
    AudioNode,
  };
  const document = {
    addEventListener: vi.fn(),
    getAnimations: () => [animation],
    querySelectorAll: () => [playing, idle],
  };
  runInNewContext(preload, { window: win, document, HTMLMediaElement: Media, AudioNode });
  return {
    win, playing, idle, animation, AudioNode,
    frame(time: number) {
      for (const [id, callback] of [...pending]) {
        if (pending.delete(id)) callback(time);
      }
    },
  };
}

it('freezes canvas frames, animations and playing media, then resumes the same work', async () => {
  const { win, frame, playing, idle, animation } = page();
  const draw = vi.fn();
  const cancelled = vi.fn();
  await playing.play();
  win.requestAnimationFrame(draw);
  const cancelledId = win.requestAnimationFrame(cancelled);
  win.__truongpaper_set_paused(true);
  frame(100);
  expect(draw).not.toHaveBeenCalled();
  expect(animation.playState).toBe('paused');
  expect(playing.paused).toBe(true);

  win.cancelAnimationFrame(cancelledId);
  win.__truongpaper_set_paused(false);
  frame(200);
  expect(draw).toHaveBeenCalledExactlyOnceWith(200);
  expect(cancelled).not.toHaveBeenCalled();
  expect(animation.playState).toBe('running');
  expect(playing.paused).toBe(false);
  expect(idle.paused).toBe(true);
});

it('scales a wallpaper’s own media mix and Web Audio output without changing project levels', async () => {
  const { win, playing, AudioNode } = page();
  const master = { gain: { value: 1 } };
  const context = {
    destination: {},
    createGain: () => master,
    addEventListener: vi.fn(),
    suspend: vi.fn(),
    resume: vi.fn(),
  };
  const node = new AudioNode(context);
  win.__truongpaper_set_volume(0.25);
  playing.volume = 0.6;
  await playing.play();
  expect(playing.volume).toBe(0.6);
  expect(playing.outputLevel()).toBeCloseTo(0.15);
  expect(node.connect(context.destination)).toBe(context.destination);
  expect(AudioNode.nativeConnect).toHaveBeenLastCalledWith(master);
  expect(master.gain.value).toBe(0.25);

  const mediaSource = new AudioNode(context, playing);
  mediaSource.connect(context.destination);
  expect(playing.outputLevel()).toBe(0.6);

  win.__truongpaper_set_volume(0);
  expect(playing.outputLevel()).toBe(0.6); // Web Audio output is muted once, at the master node.
  expect(master.gain.value).toBe(0);
  node.disconnect(context.destination);
  expect(AudioNode.nativeDisconnect).toHaveBeenCalledWith(master);
});
