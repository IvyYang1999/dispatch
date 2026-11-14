import { inject, track } from '@vercel/analytics';

const HOST = 'dispatch.yytyyf.com';
const CONSENT = 'dc-analytics-consent-v1';
const REPO = '/IvyYang1999/dispatch/releases/';

export function downloadEvent(href) {
  try {
    const u = new URL(href);
    if (u.protocol !== 'https:' || u.hostname !== 'github.com' || u.username || u.password) return null;
    if (u.pathname === REPO + 'latest') return { name: 'release_page_open', data: { target: 'github_release' } };
    const match = u.pathname.match(/^\/IvyYang1999\/dispatch\/releases\/download\/v(\d+\.\d+\.\d+)\/Dispatch-(\d+\.\d+\.\d+)-macos-arm64-signed\.zip$/);
    if (!match || match[1] !== match[2]) return null;
    return { name: 'download_click', data: { target: 'macos_arm64' } };
  } catch { return null; }
}

export function installAnalytics(w = window, d = document, sdk = { inject, track }) {
  if (w.__dispatchAnalytics) return w.__dispatchAnalytics;
  const eligible = () => {
    if (w.location.protocol !== 'https:' || w.location.hostname !== HOST || w.top !== w.self
      || w.navigator.doNotTrack === '1' || w.navigator.globalPrivacyControl === true
      || w['ga-disable-G-XC1FV10BG9'] === true) return false;
    try {
      const saved = w.localStorage.getItem(CONSENT);
      if (saved && JSON.parse(saved)?.value === 'denied') return false;
      // The SDK exposes URL redaction, but not referrer redaction. Skip collection
      // when a referring site sends more than its origin; never forward its path.
      if (d.referrer) {
        const r = new URL(d.referrer);
        if (!/^https?:$/.test(r.protocol) || r.username || r.password || r.pathname !== '/' || r.search || r.hash) return false;
      }
      return true;
    } catch { return false; }
  };
  if (!eligible()) return null;
  const api = { enabled: true };
  w.__dispatchAnalytics = api;
  let viewed = false, attempts = 0;
  try {
    sdk.inject({ mode: 'production', beforeSend(event) {
      if (!eligible() || !api.enabled) return null;
      if (event.type === 'pageview') {
        if (viewed) return null;
        viewed = true;
      } else if (event.type !== 'event') return null;
      // This is a single-page landing site: no query, fragment, or dynamic path.
      return { ...event, url: 'https://' + HOST + '/' };
    } });
  } catch { api.enabled = false; return api; }
  // Capture leaves normal, keyboard, modifier and middle-click navigation intact.
  const click = e => {
    if (!api.enabled || !eligible() || attempts >= 99 || (e.type === 'auxclick' && e.button !== 1)) return;
    const el = e.target?.closest?.('a[data-download-placement]');
    if (!el) return;
    const event = downloadEvent(el.href), placement = el.dataset.downloadPlacement;
    if (!event || !['hero', 'getting_started', 'other_platforms'].includes(placement)) return;
    attempts++;
    // Two properties fit the existing Pro plan; values come only from allowlists.
    try { sdk.track(event.name, { ...event.data, placement }); } catch { /* Downloads must remain usable. */ }
  };
  d.addEventListener('click', click, { capture: true });
  d.addEventListener('auxclick', click, { capture: true });
  return api;
}
