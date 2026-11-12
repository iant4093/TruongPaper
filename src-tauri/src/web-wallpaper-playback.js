(() => {
  // Inject before the project can capture the browser's frame functions.
  const requestFrame = window.requestAnimationFrame.bind(window);
  const cancelFrame = window.cancelAnimationFrame.bind(window);
  const frames = new Map();
  const playingMedia = new Set();
  const runningAnimations = new Set();
  const activeMedia = new Set();
  const trackedMedia = new WeakSet();
  const mediaLevels = new WeakMap();
  const routedMedia = new WeakSet();
  const masterNodes = new Map();
  const suspendedContexts = new Set();
  let masterVolume = 0;
  let nextId = 0;
  let paused = false;

  // Preserve the wallpaper's own mix. Multiply media levels and Web Audio's
  // final output by the user's master level instead of replacing the mix.
  const media = HTMLMediaElement.prototype;
  const volume = Object.getOwnPropertyDescriptor(media, 'volume');
  function syncMedia(element) {
    if (!mediaLevels.has(element)) mediaLevels.set(element, volume.get.call(element));
    const level = mediaLevels.get(element) * (routedMedia.has(element) ? 1 : masterVolume);
    if (volume.get.call(element) !== level) volume.set.call(element, level);
  }
  Object.defineProperty(media, 'volume', {
    ...volume,
    get() { return mediaLevels.get(this) ?? volume.get.call(this); },
    set(value) {
      const level = Number(value);
      if (!(level >= 0 && level <= 1)) return volume.set.call(this, value);
      mediaLevels.set(this, level);
      syncMedia(this);
    },
  });

  function trackMedia(element) {
    if (!trackedMedia.has(element)) {
      trackedMedia.add(element);
      for (const event of ['pause', 'ended']) {
        element.addEventListener(event, () => { if (element.paused || element.ended) activeMedia.delete(element); });
      }
    }
    activeMedia.add(element);
    syncMedia(element);
  }
  const { play, pause } = media;
  media.pause = function () {
    playingMedia.delete(this);
    return pause.call(this);
  };
  media.play = function () {
    if (paused) {
      playingMedia.add(this);
      return Promise.resolve();
    }
    trackMedia(this);
    const result = play.call(this);
    result?.catch(() => { if (this.paused) activeMedia.delete(this); });
    return result;
  };

  if (window.AudioNode) {
    const { connect, disconnect } = AudioNode.prototype;
    AudioNode.prototype.connect = function (destination, ...ports) {
      const context = this.context;
      // Offline rendering is part of the wallpaper's own sound processing.
      if (typeof context.resume !== 'function' || 'startRendering' in context) {
        return connect.apply(this, arguments);
      }
      if (this.mediaElement instanceof HTMLMediaElement && !routedMedia.has(this.mediaElement)) {
        routedMedia.add(this.mediaElement);
        syncMedia(this.mediaElement);
      }
      if (destination && destination === context.destination) {
        let master = masterNodes.get(context);
        if (!master) {
          master = context.createGain();
          master.gain.value = paused ? 0 : masterVolume;
          connect.call(master, destination);
          masterNodes.set(context, master);
          context.addEventListener('statechange', () => {
            if (context.state === 'closed') masterNodes.delete(context);
          });
        }
        connect.call(this, master, ...ports);
        return destination;
      }
      return connect.apply(this, arguments);
    };
    AudioNode.prototype.disconnect = function (destination, ...ports) {
      const master = destination === this.context.destination && masterNodes.get(this.context);
      return master ? disconnect.call(this, master, ...ports) : disconnect.apply(this, arguments);
    };
  }
  function syncMasterNodes() {
    for (const master of masterNodes.values()) master.gain.value = paused ? 0 : masterVolume;
  }
  window.__truongpaper_set_volume = (level) => {
    masterVolume = Math.min(1, Math.max(0, Number(level) || 0));
    for (const element of activeMedia) syncMedia(element);
    document.querySelectorAll('video, audio').forEach(syncMedia);
    syncMasterNodes();
  };

  function schedule(id, frame) {
    frame.handle = requestFrame((time) => {
      frame.handle = null;
      if (paused || !frames.delete(id)) return;
      frame.callback.call(window, time);
    });
  }

  window.requestAnimationFrame = (callback) => {
    if (typeof callback !== 'function') return requestFrame(callback);
    const id = ++nextId;
    const frame = { callback, handle: null };
    frames.set(id, frame);
    if (!paused) schedule(id, frame);
    return id;
  };
  window.cancelAnimationFrame = (id) => {
    const key = Number(id);
    const frame = frames.get(key);
    if (!frame) return;
    if (frame.handle !== null) cancelFrame(frame.handle);
    frames.delete(key);
  };

  function pauseMedia(element) {
    if (element.paused || element.ended) return;
    playingMedia.add(element);
    pause.call(element);
  }

  function pauseAnimations() {
    for (const animation of document.getAnimations()) {
      // The host's reveal must still finish when applying a paused wallpaper.
      if (animation.playState !== 'running' || animation.id === 'truongpaper-reveal') continue;
      try {
        animation.pause();
        runningAnimations.add(animation);
      } catch {}
    }
  }

  document.addEventListener('play', (event) => {
    if (!(event.target instanceof HTMLMediaElement)) return;
    trackMedia(event.target);
    if (paused) pauseMedia(event.target);
  }, true);
  document.addEventListener('animationstart', () => { if (paused) pauseAnimations(); }, true);
  document.addEventListener('transitionrun', () => { if (paused) pauseAnimations(); }, true);

  window.__truongpaper_set_paused = (next) => {
    if (paused === next) return;
    paused = next;
    syncMasterNodes();
    if (paused) {
      for (const context of masterNodes.keys()) {
        if (context.state === 'running') {
          suspendedContexts.add(context);
          context.suspend().catch(() => {});
        }
      }
      for (const frame of frames.values()) {
        if (frame.handle !== null) cancelFrame(frame.handle);
        frame.handle = null;
      }
      for (const element of activeMedia) pauseMedia(element);
      document.querySelectorAll('video, audio').forEach(pauseMedia);
      pauseAnimations();
    } else {
      for (const context of suspendedContexts) context.resume().catch(() => {});
      suspendedContexts.clear();
      for (const [id, frame] of frames) schedule(id, frame);
      for (const element of playingMedia) {
        if (!element.ended) element.play()?.catch(() => {});
      }
      playingMedia.clear();
      for (const animation of runningAnimations) {
        if (animation.playState === 'paused') animation.play();
      }
      runningAnimations.clear();
    }
  };
})();
