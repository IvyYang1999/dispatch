# Website analytics

Production: `https://dispatch.yytyyf.com`. Vercel project: `dispatch`, team:
`ivyyang1999-7110s-projects`. Read traffic in the project's **Analytics** tab.

- Page views: one per landing-page load; changing language does not add another.
- `download_click`: version-matched signed Mac zip; two properties, `target`
  (`macos_arm64`) and `placement` (`hero` / `getting_started`).
- `release_page_open`: GitHub release page, including Windows/Linux and the
  fallback when the latest Mac asset cannot be resolved. This is not a file download.

Clicks measure intent, not transfer completion or installation. GitHub release
asset `download_count` is separate and includes direct downloads and repeats.
Neither is a count of installed users. Acceptance clicks also enter the totals.

The existing Pro plan supports these custom events without Plus. Usage is
metered at the current plan rate; the per-page bound is not a billing cap.

Privacy: no consent prompt and no fabricated consent. No analytics cookies or
advertising configuration. DNT, GPC, a legacy saved refusal, unreadable privacy
preferences, non-production hosts and iframes block SDK loading. `beforeSend`
rechecks these gates and fixes the event URL to the public root (no query/hash).
The SDK does not expose referrer redaction: when the incoming referrer contains
more than its origin, the page is not counted. A strict-origin referrer policy
also keeps outgoing request headers free of page paths/queries. Click properties
are allowlisted, at most 99 per page; SDK failures never block download links.
The static privacy page has no tracker. This integration covers only the website.

GA4 remains configured: property `553546602`, stream `16339723053`, measurement
ID `G-XC1FV10BG9`, enhanced measurement off. The site does **not** load Google or
send GA4 events. All-denied consent pings cannot guarantee usable GA4 reports
without sufficient eligible traffic for modeling; they are not our visit counter.

Dark Console has the Dispatch product, URL and GitHub repository binding. Its
current collectors do not read Vercel traffic. Source registration / a Vercel
adapter are separate work; configuration alone does not prove collection.

Verification: `npm test`, `npx tsc --noEmit`, `npm run build`; inspect the real
production page and read back page views / custom events using Vercel Analytics
or `vercel metrics`. Unit fixtures do not prove server ingestion. Never embed
Vercel credentials, GA service-account keys or Measurement Protocol secrets.
