// Public measurement ID for the Dispatch web stream (not a credential).
export const GA4_ID = 'G-XC1FV10BG9';
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
    return { name: 'file_download', data: { file_name: `Dispatch-${match[1]}-macos-arm64-signed.zip`, file_extension: 'zip', target: 'macos_arm64' } };
  } catch { return null; }
}

export function installAnalytics(w = window, d = document, id = GA4_ID) {
  if (w.__dispatchAnalytics) return w.__dispatchAnalytics;
  const eligible = () => {
    if (w.location.protocol !== 'https:' || w.location.hostname !== HOST || w.top !== w.self
      || w.navigator.doNotTrack === '1' || w.navigator.globalPrivacyControl === true
      || w['ga-disable-' + id] === true) return false;
    try {
      const saved = w.localStorage.getItem(CONSENT);
      return !saved || JSON.parse(saved)?.value !== 'denied';
    } catch { return false; } // Unreadable privacy preference: do not collect.
  };
  if (!/^G-[A-Z0-9]+$/.test(id) || !eligible()) return null;
  let count = 0, loaded = false;
  w.dataLayer ||= [];
  function gtag() {
    if (loaded || w.dataLayer.length < 100) w.dataLayer.push(arguments);
  }
  const page = w.location.pathname === '/' ? '/' : '/other';
  const location = 'https://' + HOST + page;
  let referrer = '';
  try { const u = new URL(d.referrer); if (/^https?:$/.test(u.protocol)) referrer = u.origin + '/'; } catch {}
  const track = (name, props = {}) => {
    if (!eligible() || count >= 100) return;
    count++;
    gtag('event', name, { ...props, site: 'dispatch', page, language: d.documentElement.lang.startsWith('zh') ? 'zh' : 'en',
      page_location: location, page_referrer: referrer, page_title: 'Dispatch', send_to: id, transport_type: 'beacon' });
  };
  // No visitor choice is fabricated. Cookies and advertising remain denied throughout.
  gtag('consent', 'default', { analytics_storage: 'denied', ad_storage: 'denied', ad_user_data: 'denied', ad_personalization: 'denied' });
  gtag('set', 'ads_data_redaction', true);
  gtag('set', 'url_passthrough', false);
  gtag('set', { page_location: location, page_referrer: referrer, page_title: 'Dispatch' });
  gtag('js', new Date());
  gtag('config', id, { send_page_view: false, allow_google_signals: false, allow_ad_personalization_signals: false,
    page_location: location, page_referrer: referrer, page_title: 'Dispatch' });
  const script = d.createElement('script');
  script.async = true;
  script.src = 'https://www.googletagmanager.com/gtag/js?id=' + id;
  script.onload = () => { loaded = true; };
  script.onerror = () => { w['ga-disable-' + id] = true; };
  d.head.append(script);
  track('page_view');
  // Capture doesn't cancel, delay or rewrite navigation, including modifier/middle clicks.
  const click = e => {
    if (e.type === 'auxclick' && e.button !== 1) return;
    const el = e.target?.closest?.('a[data-download-placement]');
    if (!el) return;
    const event = downloadEvent(el.href);
    const placement = el.dataset.downloadPlacement;
    if (event && ['hero', 'getting_started', 'other_platforms'].includes(placement)) track(event.name, { ...event.data, placement });
  };
  d.addEventListener('click', click, { capture: true });
  d.addEventListener('auxclick', click, { capture: true });
  w.addEventListener('storage', e => { if (e.key === CONSENT && !eligible()) w['ga-disable-' + id] = true; });
  const api = { enabled: true };
  w.__dispatchAnalytics = api;
  return api;
}
