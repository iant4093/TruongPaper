// src-tauri/crates/truongpaper-platform/src/web-wallpaper-preload.js
(function () {
  'use strict';

  const audioCallbacks = [];
  window.__truongpaper_fps_limit = 60;

  // 1. Wallpaper Engine Audio Listener Registration (128-float array: 0..63 Left, 64..127 Right)
  window.wallpaperRegisterAudioListener = function (callback) {
    if (typeof callback === 'function' && !audioCallbacks.includes(callback)) {
      audioCallbacks.push(callback);
    }
  };

  // Called by Rust backend at 60 Hz with a 128-element Array<number>
  window.__truongpaper_dispatch_audio = function (stereo128Array) {
    for (let i = 0; i < audioCallbacks.length; i++) {
      try {
        audioCallbacks[i](stereo128Array);
      } catch (err) {
        console.error('[TruongPaper WebBridge] Audio callback error:', err);
      }
    }
  };

  // 2. Called by Rust backend on startup and whenever user changes a property in PropertyInspector
  window.__truongpaper_dispatch_properties = function (propsPayload, generalSettings) {
    if (window.wallpaperPropertyListener) {
      if (
        propsPayload &&
        typeof window.wallpaperPropertyListener.applyUserProperties === 'function'
      ) {
        try {
          window.wallpaperPropertyListener.applyUserProperties(propsPayload);
        } catch (err) {
          console.error('[TruongPaper WebBridge] applyUserProperties error:', err);
        }
      }
      if (
        generalSettings &&
        typeof window.wallpaperPropertyListener.applyGeneralProperties === 'function'
      ) {
        try {
          window.wallpaperPropertyListener.applyGeneralProperties(generalSettings);
        } catch (err) {
          console.error('[TruongPaper WebBridge] applyGeneralProperties error:', err);
        }
      }
    }
  };

  // 3. Random file helper for directory properties
  window.wallpaperRequestRandomFileForProperty = function (propertyName, callback) {
    if (window.__truongpaper_files_by_prop && window.__truongpaper_files_by_prop[propertyName]) {
      const list = window.__truongpaper_files_by_prop[propertyName];
      if (list.length > 0 && typeof callback === 'function') {
        const pick = list[Math.floor(Math.random() * list.length)];
        callback(propertyName, pick);
      }
    }
  };
})();
