import type { CSSProperties } from "react";

interface AppBackgroundProps {
  animated?: boolean;
}

const mediaStyle: CSSProperties = {
  position: "absolute",
  inset: 0,
  width: "100%",
  height: "100%",
  objectFit: "cover",
  objectPosition: "center",
  zIndex: 0,
  filter: "blur(5px)",
  animation: "video-fade-in 800ms ease-out both",
};

export default function AppBackground({ animated = false }: AppBackgroundProps) {
  if (animated) {
    return (
      <video
        autoPlay
        loop
        muted
        playsInline
        preload="metadata"
        poster="/background/image.webp"
        style={mediaStyle}
      >
        <source src="/background/leaves-bg-uncompressed.mp4" type="video/mp4" />
      </video>
    );
  }

  return (
    <img
      src="/background/image.webp"
      alt=""
      aria-hidden
      decoding="async"
      style={mediaStyle}
    />
  );
}
