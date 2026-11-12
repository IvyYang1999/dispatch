# The Dispatch landing page

A static page: Vite + React, no backend. The demo on it is built with the Stageshow kit
(`src/kit`, MIT, see `src/kit/LICENSE` and `src/kit/NOTICE.md`; the kit's third-party brand
artwork is left out).

```bash
npm ci
npm run dev      # http://127.0.0.1:5173
npm run build    # writes dist/, which any static host can serve
```

- `src/App.tsx`, `src/site.css`: the page. Colors and shapes are the app's own tokens.
- `src/Demo.tsx`, `src/demo.css`: the demo. The visitor chooses Send or Don't send in the review
  card; four scenarios share one scene.
- `src/i18n.ts`: the page's words, English and Chinese.
- `src/links.ts`: every outside link. A link set to `null` shows as "soon": fill in X, Discord
  and the support address before publishing.

The download button asks GitHub for the newest release and links to its signed Mac zip; if that
fails it goes to the releases page.

Not published anywhere yet. `vite.config.ts` uses relative asset paths, so `dist/` works at a
domain root or under a path (GitHub Pages).
