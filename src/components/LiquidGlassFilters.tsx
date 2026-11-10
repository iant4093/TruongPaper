// src/components/LiquidGlassFilters.tsx
import React from 'react';

export const LiquidGlassFilters: React.FC = () => (
  <svg
    aria-hidden="true"
    style={{ position: 'absolute', width: 0, height: 0, pointerEvents: 'none' }}
  >
    <defs>
      <filter
        id="liquid-refraction"
        x="-10%"
        y="-10%"
        width="120%"
        height="120%"
        colorInterpolationFilters="sRGB"
      >
        {/* Subtle organic glass surface curvature noise */}
        <feTurbulence
          type="fractalNoise"
          baseFrequency="0.012 0.018"
          numOctaves={2}
          seed={42}
          result="noise"
        />
        {/* Smooth the noise field so refraction bends cleanly like polished optical glass */}
        <feGaussianBlur in="noise" stdDeviation="3.5" result="smoothNoise" />
        {/* Refract the backdrop graphic using the smoothed normal field */}
        <feDisplacementMap
          in="SourceGraphic"
          in2="smoothNoise"
          scale={11}
          xChannelSelector="R"
          yChannelSelector="G"
          result="refracted"
        />
        {/* Specular highlight layer for liquid glass sheen */}
        <feSpecularLighting
          in="smoothNoise"
          surfaceScale={2.2}
          specularConstant={0.55}
          specularExponent={28}
          lightingColor="#ffffff"
          result="specular"
        >
          <fePointLight x={-200} y={-300} z={260} />
        </feSpecularLighting>
        <feComposite
          in="specular"
          in2="SourceAlpha"
          operator="in"
          result="clippedSpecular"
        />
        <feBlend
          mode="screen"
          in="clippedSpecular"
          in2="refracted"
          result="finalGlass"
        />
      </filter>
    </defs>
  </svg>
);
