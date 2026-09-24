import React from "react";

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
      height={height ?? (width ? width * 0.32 : undefined)}
      className={className}
      viewBox="0 0 400 128"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      role="img"
      aria-label="Sokhan"
    >
      <circle cx="34" cy="64" r="30" className="fill-accent opacity-15" />
      <path
        d="M34 44a10 10 0 0 1 10 10v10a10 10 0 0 1-20 0V54a10 10 0 0 1 10-10Z"
        className="fill-accent"
      />
      <path
        d="M20 62a14 14 0 0 0 28 0M34 76v10M26 86h16"
        stroke="currentColor"
        strokeWidth="4"
        strokeLinecap="round"
        className="text-accent"
        fill="none"
      />
      <text
        x="78"
        y="78"
        className="fill-current text-text"
        style={{ fontSize: "44px", fontWeight: 700, letterSpacing: "1px" }}
      >
        Sokhan
      </text>
    </svg>
  );
};

export default SokhanTextLogo;
