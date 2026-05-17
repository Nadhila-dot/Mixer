import { useLocation } from "react-router-dom";
import { useEffect, useState } from "react";
import type { SsrEnvelope } from "../types";

declare global {
  interface Window {
    __SSR_DATA__?: SsrEnvelope;
  }
}

/** Raw envelope from the server — may be undefined on subsequent client-side navigations. */
export function getSsrEnvelope<T>(): SsrEnvelope<T> | null {
  return (window.__SSR_DATA__ as SsrEnvelope<T> | undefined) ?? null;
}

/**
 * Hook that returns page-specific SSR data when the server rendered the current
 * route, and transparently falls back to a client-side fetch for subsequent
 * navigations (when __SSR_DATA__ belongs to a different page).
 *
 * @param page  — route name that matches what Rust injects (e.g. "home")
 * @param fetch — async function that returns the same data shape from the API
 */
export function usePageData<T>(
  page: string,
  fetchFn: () => Promise<T>
): { data: T | null; source: "ssr" | "api" | null; renderTs: number | null } {
  const location = useLocation();

  const ssr = getSsrEnvelope<T>();
  const ssrMatches = ssr?.page === page;

  const [data, setData] = useState<T | null>(ssrMatches ? ssr!.data : null);
  const [source, setSource] = useState<"ssr" | "api" | null>(
    ssrMatches ? "ssr" : null
  );
  const [renderTs, setRenderTs] = useState<number | null>(
    ssrMatches ? ssr!.ts : null
  );

  useEffect(() => {
    if (!ssrMatches) {
      fetchFn()
        .then((d) => {
          setData(d);
          setSource("api");
          setRenderTs(Date.now());
        })
        .catch(console.error);
    }
    // Re-fetch if the same page is navigated to again with a different key
  }, [location.key]); // eslint-disable-line react-hooks/exhaustive-deps

  return { data, source, renderTs };
}
