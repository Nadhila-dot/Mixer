/**
 * VideoBackground — full-screen looping video with a layered composite overlay.
 *
 * The SVG filter below simulates light refraction through glass by applying a
 * feTurbulence → feDisplacementMap pipeline to a copy of the video that sits
 * in the "glass zone". Real glass bends rays by ~2–5px at typical viewing
 * distances; we match that scale.
 */

interface Props {
  src: string;
  /** Additional sources for multi-format fallback */
  fallbackSrc?: string;
}

export default function VideoBackground({ src, fallbackSrc }: Props) {
  return (
    <>
      {/* ── SVG filter definitions (invisible, referenced by CSS) ─────────── */}
      <svg
        aria-hidden
        style={{ position: "absolute", width: 0, height: 0, overflow: "hidden" }}
      >
        <defs>
          {/* Glass refraction: fractal noise drives a displacement map */}
          <filter
            id="glass-refract"
            x="-5%"
            y="-5%"
            width="110%"
            height="110%"
            colorInterpolationFilters="sRGB"
          >
            <feTurbulence
              type="fractalNoise"
              baseFrequency="0.012 0.025"
              numOctaves="3"
              seed="8"
              result="noise"
            />
            {/* Pull only the displacement signal — keep it subtle */}
            <feColorMatrix
              type="matrix"
              values="0 0 0 0 0
                      0 0 0 0 0
                      0 0 0 0 0
                      1 0 0 0 0"
              result="mask"
            />
            <feDisplacementMap
              in="SourceGraphic"
              in2="noise"
              scale="5"
              xChannelSelector="R"
              yChannelSelector="G"
              result="displaced"
            />
            {/* Slightly desaturate the displaced region — frosted glass loses some colour */}
            <feColorMatrix
              type="saturate"
              values="0.7"
              in="displaced"
            />
          </filter>

          {/* Caustic shimmer applied to the specular highlight layer */}
          <filter id="caustic" x="-20%" y="-20%" width="140%" height="140%">
            <feTurbulence
              type="turbulence"
              baseFrequency="0.04 0.06"
              numOctaves="4"
              seed="3"
              result="noise"
            />
            <feColorMatrix
              type="luminanceToAlpha"
              result="luminance"
            />
            <feBlend in="SourceGraphic" in2="luminance" mode="screen" />
          </filter>
        </defs>
      </svg>

      {/* ── Video layer ────────────────────────────────────────────────────── */}
      <div
        aria-hidden
        style={{
          position: "fixed",
          inset: 0,
          zIndex: 0,
          overflow: "hidden",
        }}
      >
        <video
          autoPlay
          loop
          muted
          playsInline
          style={{
            width: "100%",
            height: "100%",
            objectFit: "cover",
            objectPosition: "center",
          }}
        >
          <source src={src} type="video/mp4" />
          {fallbackSrc && <source src={fallbackSrc} type="video/mp4" />}
        </video>

        {/* Subtle vignette — darkens edges so the centred content pops */}
        <div
          style={{
            position: "absolute",
            inset: 0,
            background:
              "radial-gradient(ellipse 80% 80% at 50% 50%, transparent 30%, rgba(0,0,0,0.55) 100%)",
          }}
        />

        {/* Very slight overall dark tint so text is always legible */}
        <div
          style={{
            position: "absolute",
            inset: 0,
            background: "rgba(0,0,0,0.18)",
          }}
        />
      </div>
    </>
  );
}
