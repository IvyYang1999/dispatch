# The Dispatch landing page

A static page: Vite + React, no backend.

```bash
npm ci
npm run dev      # builds the embedded app, then serves http://127.0.0.1:5173
npm run build    # writes dist/, which any static host can serve
```

## What is on it

- A hero with the download button. The button asks GitHub for the newest release and links to
  its signed Mac zip; if that fails it goes to the releases page.
- Four points with icons: across harnesses, computers, networks, systems.
- The app itself, to click through. `scripts/build-app.mjs` builds the desktop app's own frontend
  (`../apps/desktop`) into `public/app/` and puts a stand-in backend in front of it
  (`app-demo/mock.js`), which answers from made-up data: seven invented sessions on three invented
  machines. The page embeds it twice, as the main window and as the review window, and the tabs
  above it open the main window, the address book, the review window and the card after Send.
  Nothing in it is a real session, and it talks to no network.
- Three steps to start, and the links.

## Files

- `src/App.tsx`, `src/site.css`: the page. Colors and shapes are the app's own tokens.
- `src/i18n.ts`: the page's words, English and Chinese.
- `src/links.ts`: every outside link. A link set to `null` shows as "soon": fill in X, Discord
  and the support address before publishing.
- `src/icons.tsx`, `app-demo/harness-icons.js`: generated from Simple Icons (CC0-1.0) and Tabler
  Icons (MIT). The product marks belong to their owners and only name what Dispatch works with.
- `app-demo/mock.js`: the demo's data and its answers to the app.
- `vercel.json`: the build settings for Vercel, with `site` as the project's root directory. The
  build reads `../apps/desktop`, so the project must include files outside the root directory
  (Vercel's default).

`public/app/` is generated and not committed.
