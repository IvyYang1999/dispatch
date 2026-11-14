import { useCallback, useEffect, useRef, useState } from 'react'
import { Icon, type IconName } from './icons'
import { LINKS } from './links'
import { PAGE, type Lang, type ScreenId } from './i18n'
import { installAnalytics } from './analytics.mjs'

// The app itself is embedded twice, as its main window and its review window (public/app, built
// from apps/desktop by scripts/build-app.mjs), with a stand-in backend that answers from made-up
// data (app-demo/mock.js). Both frames and this page share one object:
type Hub = {
  store?: unknown
  showReview?: () => void
  hideReview?: () => void
  resizeReview?: (height: number) => void
  setLang?: (lang: Lang) => void
  /** Puts a fresh message on hold, when the first one was already decided. */
  holdOne?: () => void
}
const hub: Hub = ((window as unknown as { __dispatchDemo?: Hub }).__dispatchDemo ??= {})

const APP_W = 1040
const APP_H = 660
const SCREENS: ScreenId[] = ['main', 'book', 'review', 'sent']

function useDark(): boolean {
  const [dark, setDark] = useState(() => window.matchMedia('(prefers-color-scheme: dark)').matches)
  useEffect(() => {
    const mq = window.matchMedia('(prefers-color-scheme: dark)')
    const on = () => setDark(mq.matches)
    mq.addEventListener('change', on)
    return () => mq.removeEventListener('change', on)
  }, [])
  return dark
}

function firstLang(): Lang {
  try {
    const saved = localStorage.getItem('lang')
    if (saved === 'en' || saved === 'zh') return saved
  } catch {
    /* private mode: go by the browser */
  }
  return navigator.language.toLowerCase().startsWith('zh') ? 'zh' : 'en'
}

/** The newest release: its version and the signed Mac download, from GitHub. Until it answers
 *  (or if it never does), the button goes to the releases page. */
function useLatest(): { version: string | null; mac: string } {
  const [latest, setLatest] = useState<{ version: string | null; mac: string }>({ version: null, mac: LINKS.releases })
  useEffect(() => {
    let live = true
    fetch(LINKS.latestApi, { headers: { Accept: 'application/vnd.github+json' } })
      .then((r) => (r.ok ? r.json() : null))
      .then((v) => {
        if (!live || !v) return
        const asset = (v.assets as { name: string; browser_download_url: string }[] | undefined)?.find((a) => a.name.endsWith('-macos-arm64-signed.zip'))
        setLatest({ version: typeof v.tag_name === 'string' ? v.tag_name.replace(/^v/, '') : null, mac: asset?.browser_download_url ?? LINKS.releases })
      })
      .catch(() => {})
    return () => {
      live = false
    }
  }, [])
  return latest
}

function Out({ href, soon, children }: { href: string | null; soon: string; children: React.ReactNode }) {
  if (!href) {
    return (
      <span className="out out--soon" title={soon}>
        {children}
        <i>{soon}</i>
      </span>
    )
  }
  return (
    <a className="out" href={href} target={href.startsWith('mailto:') ? undefined : '_blank'} rel="noreferrer">
      {children}
    </a>
  )
}

/** The app in a window frame, scaled to the column; below a readable size it scrolls sideways. */
function AppDemo({ lang, version, screen, onScreen }: { lang: Lang; version: string | null; screen: ScreenId; onScreen: (s: ScreenId) => void }) {
  const fit = useRef<HTMLDivElement>(null)
  const main = useRef<HTMLIFrameElement>(null)
  const review = useRef<HTMLIFrameElement>(null)
  const [scale, setScale] = useState(1)
  const [reviewOpen, setReviewOpen] = useState(false)
  const [reviewH, setReviewH] = useState(380)
  const wantSend = useRef(false)

  useEffect(() => {
    const el = fit.current
    if (!el) return
    const measure = () => setScale(Math.min(1, Math.max(0.62, el.clientWidth / APP_W)))
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  // what the two frames ask of the page
  useEffect(() => {
    hub.showReview = () => {
      setReviewH(380)
      setReviewOpen(true)
    }
    hub.hideReview = () => {
      setReviewOpen(false)
      onScreen('main')
    }
    // the review window asks for its own height, as it asks the system in the app
    hub.resizeReview = (h) => setReviewH(Math.max(160, Math.min(APP_H - 48, Math.round(h))))
    return () => {
      hub.showReview = hub.hideReview = hub.resizeReview = undefined
    }
  }, [onScreen])

  const click = (frame: HTMLIFrameElement | null, selector: string, index = 0): boolean => {
    const el = frame?.contentDocument?.querySelectorAll<HTMLElement>(selector)[index]
    el?.click()
    return !!el
  }

  // the tabs above the window drive it; clicking inside it works as in the app
  useEffect(() => {
    wantSend.current = false
    if (screen === 'main') {
      setReviewOpen(false)
      click(main.current, '.side .nav', 0)
    } else if (screen === 'book') {
      setReviewOpen(false)
      click(main.current, '.side .nav', 1)
      // the Address book tab of the directory
      setTimeout(() => click(main.current, '.tabs .tab', 1), 120)
    } else {
      click(main.current, '.side .nav', 0)
      hub.holdOne?.()
      wantSend.current = screen === 'sent'
      setReviewH(380)
      setReviewOpen(true)
    }
  }, [screen])

  // "After you send": press Send in the review window once the button is there
  useEffect(() => {
    if (!reviewOpen) return
    const h = setInterval(() => {
      if (wantSend.current && click(review.current, '.review__foot .btn--primary')) wantSend.current = false
    }, 200)
    return () => clearInterval(h)
  }, [reviewOpen])

  const src = `./app/index.html?lang=${lang}&v=${encodeURIComponent(version ?? 'demo')}`
  return (
    <div className="appfit" ref={fit}>
      <div className="appscale" style={{ width: APP_W * scale, height: APP_H * scale }}>
        <div className="appwin" style={{ width: APP_W, height: APP_H, transform: `scale(${scale})` }}>
          <span className="lights" aria-hidden="true">
            <i />
            <i />
            <i />
          </span>
          <iframe ref={main} key={`main-${lang}`} title="Dispatch" src={src} loading="lazy" />
          {reviewOpen && (
            <div className="revwin" style={{ height: reviewH }}>
              <span className="lights" aria-hidden="true">
                <i />
                <i />
                <i />
              </span>
              <iframe ref={review} key={`review-${lang}`} title="Dispatch review" src={`${src}#review`} />
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

const ACROSS: { id: 'harness' | 'machines' | 'networks' | 'systems'; icons: (IconName | 'grok')[] }[] = [
  { id: 'harness', icons: ['claude', 'openai', 'grok'] },
  { id: 'machines', icons: ['laptop', 'desktop'] },
  { id: 'networks', icons: ['wifi', 'world', 'router'] },
  { id: 'systems', icons: ['apple', 'windows', 'linux'] },
]
const ICON_TITLE: Partial<Record<IconName, string>> = { claude: 'Claude Code', openai: 'Codex', apple: 'macOS', windows: 'Windows', linux: 'Linux' }

export function App() {
  const [lang, setLang] = useState<Lang>(firstLang)
  const [screen, setScreen] = useState<ScreenId>('main')
  useDark()
  const latest = useLatest()
  const t = PAGE[lang]

  useEffect(() => {
    document.documentElement.lang = lang === 'zh' ? 'zh-CN' : 'en'
    document.title = t.title
    installAnalytics()
    try {
      localStorage.setItem('lang', lang)
    } catch {
      /* private mode */
    }
  }, [lang, t.title])

  // the embedded app's own language switch changes the page's too
  const switchLang = useCallback((l: Lang) => {
    hub.store = undefined
    setScreen('main')
    setLang(l)
  }, [])
  useEffect(() => {
    hub.setLang = switchLang
    return () => {
      hub.setLang = undefined
    }
  }, [switchLang])

  const docs = lang === 'zh' ? LINKS.docsZh : LINKS.docs
  return (
    <div className="page">
      <header className="top">
        <a className="brand" href="#top">
          <img src="./icon.png" alt="" width={24} height={24} />
          Dispatch
        </a>
        <nav className="top__nav">
          <a href={docs} target="_blank" rel="noreferrer">
            {t.docs}
          </a>
          <a href={LINKS.github} target="_blank" rel="noreferrer">
            GitHub
          </a>
          <span className="seg" role="group" aria-label="Language">
            <button data-active={lang === 'en' || undefined} onClick={() => switchLang('en')}>
              EN
            </button>
            <button data-active={lang === 'zh' || undefined} onClick={() => switchLang('zh')}>
              中文
            </button>
          </span>
        </nav>
      </header>

      <main id="top">
        <section className="hero">
          <h1>{t.h1}</h1>
          <p className="hero__sub">{t.sub}</p>
          <div className="hero__cta">
            <a className="btn btn--primary" href={latest.mac} data-download-placement="hero">
              {t.download}
            </a>
            <a className="btn" href={LINKS.github} target="_blank" rel="noreferrer">
              {t.source}
            </a>
          </div>
          <p className="hero__meta">
            {latest.version ? `${t.version} ${latest.version} · ` : ''}
            {t.meta}{' '}
            <a href={LINKS.releases} target="_blank" rel="noreferrer" data-download-placement="other_platforms">
              {t.other}
            </a>
          </p>
        </section>

        <section className="across" aria-label={t.acrossLabel}>
          {ACROSS.map((a) => (
            <div key={a.id} className="across__item">
              <div className="across__icons">
                {a.icons.map((name) =>
                  name === 'grok' ? (
                    <span key={name} className="across__word">
                      Grok Bot
                    </span>
                  ) : (
                    <span key={name} className="across__icon" title={ICON_TITLE[name]}>
                      <Icon name={name} size={20} title={ICON_TITLE[name]} />
                    </span>
                  ),
                )}
              </div>
              <h2>{t.across[a.id].title}</h2>
              <p>{t.across[a.id].body}</p>
            </div>
          ))}
        </section>

        <section className="try" aria-label={t.tryLabel}>
          <div className="try__head">
            <div className="tabs" role="tablist" aria-label={t.tryLabel}>
              {SCREENS.map((id) => (
                <button key={id} role="tab" aria-selected={screen === id} className="tab" data-active={screen === id || undefined} onClick={() => setScreen(id)}>
                  {t.screens[id].title}
                </button>
              ))}
            </div>
            <p className="try__sub">{t.screens[screen].sub}</p>
          </div>
          <AppDemo lang={lang} version={latest.version} screen={screen} onScreen={setScreen} />
          <p className="try__foot">{t.tryHint}</p>
        </section>

        <section className="start">
          <h2>{t.startTitle}</h2>
          <ol className="steps">
            {t.steps.map((s, i) => (
              <li key={i}>
                <span className="steps__n">{i + 1}</span>
                <div>
                  <div className="steps__title">{s.title}</div>
                  <div className="steps__body">{s.body}</div>
                </div>
              </li>
            ))}
          </ol>
          <div className="hero__cta">
            <a className="btn btn--primary" href={latest.mac} data-download-placement="getting_started">
              {t.download}
            </a>
            <a className="btn" href={docs} target="_blank" rel="noreferrer">
              {t.readDocs}
            </a>
          </div>
        </section>
      </main>

      <footer className="foot">
        <div className="foot__links">
          <Out href={docs} soon={t.soon}>
            {t.docs}
          </Out>
          <Out href={LINKS.github} soon={t.soon}>
            GitHub
          </Out>
          <Out href={LINKS.x} soon={t.soon}>
            X
          </Out>
          <Out href={LINKS.discord} soon={t.soon}>
            Discord
          </Out>
          <Out href={LINKS.email ? `mailto:${LINKS.email}` : null} soon={t.soon}>
            {LINKS.email ?? t.email}
          </Out>
          <a className="out" href="/privacy.html">{lang === 'zh' ? '网站统计与隐私' : 'Website analytics & privacy'}</a>
        </div>
        <div className="foot__note">{t.license}</div>
      </footer>
    </div>
  )
}
