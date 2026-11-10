// src/components/AmbientBacklightCanvas.tsx
import React, { useEffect, useRef } from 'react';

interface AmbientBacklightProps {
  activeThumbnailUrl: string | null;
}

export const AmbientBacklightCanvas: React.FC<AmbientBacklightProps> = ({
  activeThumbnailUrl,
}) => {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const img = new Image();
    img.crossOrigin = 'anonymous';
    if (activeThumbnailUrl) {
      img.src = activeThumbnailUrl;
    }

    let animId = 0;
    let phase = 0;

    const render = () => {
      phase += 0.004;
      const w = canvas.width;
      const h = canvas.height;
      ctx.clearRect(0, 0, w, h);

      // Base dark obsidian tint
      ctx.fillStyle = '#090b12';
      ctx.fillRect(0, 0, w, h);

      if (img.complete && img.naturalWidth > 0) {
        ctx.save();
        ctx.globalAlpha = 0.48;
        ctx.filter = 'blur(56px) saturate(185%)';
        const offsetX = Math.sin(phase) * 28;
        const offsetY = Math.cos(phase * 0.8) * 22;
        ctx.drawImage(img, -40 + offsetX, -40 + offsetY, w + 80, h + 80);
        ctx.restore();
      } else {
        // Fallback aurora gradient when no thumbnail is loaded yet
        const grad = ctx.createRadialGradient(
          w * (0.5 + 0.2 * Math.sin(phase)),
          h * (0.4 + 0.15 * Math.cos(phase)),
          20,
          w * 0.5,
          h * 0.5,
          w * 0.8
        );
        grad.addColorStop(0, 'rgba(56, 128, 255, 0.32)');
        grad.addColorStop(0.5, 'rgba(132, 68, 255, 0.18)');
        grad.addColorStop(1, 'rgba(9, 11, 18, 0.95)');
        ctx.fillStyle = grad;
        ctx.fillRect(0, 0, w, h);
      }

      animId = window.setTimeout(() => requestAnimationFrame(render), 33);
    };

    render();
    return () => clearTimeout(animId);
  }, [activeThumbnailUrl]);

  return (
    <canvas
      ref={canvasRef}
      width={320}
      height={200}
      className="ambient-backlight-canvas"
    />
  );
};
