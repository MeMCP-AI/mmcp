// Pure client-side SPA. Every data fetch flows through the browser
// against the mmcp-server, so prerendering individual pages has no
// upside and the `fallback: 'index.html'` SPA shell covers every
// route on first navigation.
export const ssr = false;
export const prerender = false;
