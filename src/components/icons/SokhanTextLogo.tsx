import React from "react";

/**
 * Sokhan wordmark: an open mouth mid-speech, with sound ripples,
 * paired with the app name in the UI typeface.
 */
const SokhanTextLogo = ({
  width,
  height,
  className,
}: {
  width?: number;
  height?: number;
  className?: string;
}) => {
  return (
    <svg
      width={width}
      height={height ?? (width ? width * 0.3 : undefined)}
      className={className}
      viewBox="0 0 420 126"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      role="img"
      aria-label="Sokhan"
    >
      <g transform="translate(14,21)">
        {/* soft halo */}
        <circle cx="42" cy="42" r="42" className="fill-accent opacity-10" />
        {/* upper lip with cupid's bow */}
        <path
          d="M14 40 C20 24 32 18 42 26 C52 18 64 24 70 40 C64 44 52 46 42 45 C32 46 20 44 14 40 Z"
          className="fill-accent"
        />
        {/* lower lip */}
        <path
          d="M17 45 C25 62 59 62 67 45 C59 50 25 50 17 45 Z"
          className="fill-accent opacity-80"
        />
        {/* open mouth line */}
        <path
          d="M16 41.5 C30 47 54 47 68 41.5 C54 49 30 49 16 41.5 Z"
          className="fill-ink opacity-60"
        />
        {/* sound ripples */}
        <path
          d="M78 30 a16 16 0 0 1 0 24"
          stroke="currentColor"
          strokeWidth="4"
          strokeLinecap="round"
          className="text-accent opacity-70"
        />
        <path
          d="M87 22 a26 26 0 0 1 0 40"
          stroke="currentColor"
          strokeWidth="4"
          strokeLinecap="round"
          className="text-accent opacity-35"
        />
      </g>
      <text
        x="136"
        y="80"
        className="fill-current text-text"
        style={{ fontSize: "46px", fontWeight: 700, letterSpacing: "0.5px" }}
      >
        Sokhan
      </text>
    </svg>
  );
};

export default SokhanTextLogo;
