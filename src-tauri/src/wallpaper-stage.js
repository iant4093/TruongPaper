(() => {
  const cover = document.createElement('style');
  cover.textContent = ':root { opacity: 0 !important; }';
  const conceal = () => document.documentElement?.append(cover);
  conceal();
  if (!document.documentElement) {
    const observer = new MutationObserver(() => {
      if (!document.documentElement) return;
      conceal();
      observer.disconnect();
    });
    observer.observe(document, { childList: true });
  }
  let scheduled = false;
  window.__truongpaper_signal_ready = () => {
    if (scheduled) return;
    scheduled = true;
    let sent = false;
    const send = () => {
      if (sent) return;
      sent = true;
      window.location.href = 'wpe://localhost/__truongpaper_ready__';
    };
    requestAnimationFrame(() => requestAnimationFrame(send));
    // A covered desktop may have throttled animation callbacks.
    setTimeout(send, 200);
  };
  window.__truongpaper_reveal = () => {
    const root = document.documentElement;
    cover.remove();
    if (!matchMedia('(prefers-reduced-motion: reduce)').matches) {
      root.animate([{ opacity: 0 }, { opacity: getComputedStyle(root).opacity }], {
        duration: 180,
        easing: 'ease-out',
      });
    }
    window.dispatchEvent(new Event('truongpaper:revealed'));
  };
})();
