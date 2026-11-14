# Website analytics

Production hostname: `dispatch.yytyyf.com`. GA4 property: `553546602`; web stream:
`16339723053`; public measurement ID: `G-XC1FV10BG9`.

The tag runs only on the production HTTPS site. It sends one `page_view` per page
load and `file_download` when a visitor clicks a version-matched signed Mac zip.
The GitHub release-list link sends `release_page_open` instead. Both download
buttons carry a `placement` parameter (`hero` or `getting_started`). Enhanced
measurement is disabled in GA4 to avoid duplicate and unrelated automatic events.

`file_download` measures intent, not transfer completion or successful installation.
GitHub release asset `download_count` is a separate aggregate, including direct
downloads and repeat requests. Neither metric is a count of installed users.

All four consent-mode values stay denied. No banner is added, no consent is
invented, and existing refusal, DNT and GPC prevent the tag from loading. No
persistent visitor ID is created. Queries/fragments are stripped; referrers are
reduced to their origin. Preview/local hosts do not send events. The public
`/privacy.html` page explains that basic pings still reach Google and that unique
and returning visitor counts are limited. This covers the website only.

Dark Console's Dispatch product points to this hostname, the same GA4 property,
and `IvyYang1999/dispatch` for GitHub download data. Its reports may lag and must
not treat a newly connected stream as an established zero-traffic baseline.

Verification: `npm test`, `npx tsc --noEmit`, `npm run build`; then inspect the
production page and confirm real events in the new stream. A DOM fixture proves
the event/filter contract, not Google ingestion. Never put service-account keys
or Measurement Protocol secrets into the site.
