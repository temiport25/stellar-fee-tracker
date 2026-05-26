import type { NextConfig } from "next";

// Render supplies BACKEND_HOST (bare hostname, no protocol).
// Local dev uses BACKEND_URL (full URL). We normalise both to a full URL here.
function resolveBackendUrl(): string {
  if (process.env.BACKEND_HOST) {
    return `https://${process.env.BACKEND_HOST}`;
  }
  return process.env.BACKEND_URL ?? "http://localhost:8080";
}

const nextConfig: NextConfig = {
  async rewrites() {
    return [
      {
        source: "/api/:path*",
        destination: `${resolveBackendUrl()}/:path*`,
      },
    ];
  },
};

export default nextConfig;
