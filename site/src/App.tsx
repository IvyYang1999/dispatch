import { useEffect, useState } from 'react'
import { Demo, type Lang, type ScenarioId } from './Demo'
import { LINKS } from './links'
import { PAGE } from './i18n'

const SCENARIOS: ScenarioId[] = ['handoff', 'machines', 'closed', 'unknown']

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

export function App() {
  const [lang, setLang] = useState<Lang>(firstLang)
  const [scenario, setScenario] = useState<ScenarioId>('handoff')
  const [run, setRun] = useState(0)
  const dark = useDark()
  const latest = useLatest()
  const t = PAGE[lang]

  useEffect(() => {
    document.documentElement.lang = lang === 'zh' ? 'zh-CN' : 'en'
    document.title = t.title
    try {
      localStorage.setItem('lang', lang)
    } catch {
      /* private mode */
    }
  }, [lang, t.title])

  const pick = (id: ScenarioId) => {
    setScenario(id)
    setRun((n) => n + 1)
  }

  return (
    <div className="page">
      <header className="top">
        <a className="brand" href="#top">
          <img src="./icon.png" alt="" width={24} height={24} />
          Dispatch
        </a>
        <nav className="top__nav">
          <a href={lang === 'zh' ? LINKS.docsZh : LINKS.docs} target="_blank" rel="noreferrer">
            {t.docs}
          </a>
          <a href={LINKS.github} target="_blank" rel="noreferrer">
            GitHub
          </a>
          <span className="seg" role="group" aria-label="Language">
            <button data-active={lang === 'en' || undefined} onClick={() => setLang('en')}>
              EN
            </button>
            <button data-active={lang === 'zh' || undefined} onClick={() => setLang('zh')}>
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
            <a className="btn btn--primary" href={latest.mac}>
              {t.download}
            </a>
            <a className="btn" href={LINKS.github} target="_blank" rel="noreferrer">
              {t.source}
            </a>
          </div>
          <p className="hero__meta">
            {latest.version ? `${t.version} ${latest.version} · ` : ''}
            {t.meta}{' '}
            <a href={LINKS.releases} target="_blank" rel="noreferrer">
              {t.other}
            </a>
          </p>
        </section>

        <section className="try" aria-label={t.tryLabel}>
          <div className="try__tabs" role="tablist" aria-label={t.tryLabel}>
            {SCENARIOS.map((id) => (
              <button key={id} role="tab" aria-selected={scenario === id} className="tab" data-active={scenario === id || undefined} onClick={() => pick(id)}>
                <span className="tab__title">{t.scenarios[id].title}</span>
                <span className="tab__sub">{t.scenarios[id].sub}</span>
              </button>
            ))}
          </div>
          <div className="try__stage">
            <Demo key={`${scenario}-${run}`} scenario={scenario} lang={lang} dark={dark} />
          </div>
          <div className="try__foot">
            <span>{t.tryHint}</span>
            <button className="link" onClick={() => setRun((n) => n + 1)}>
              {t.replay}
            </button>
          </div>
        </section>

        <section className="points">
          {t.points.map((p) => (
            <div key={p.title} className="point">
              <h2>{p.title}</h2>
              <p>{p.body}</p>
            </div>
          ))}
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
            <a className="btn btn--primary" href={latest.mac}>
              {t.download}
            </a>
            <a className="btn" href={lang === 'zh' ? LINKS.docsZh : LINKS.docs} target="_blank" rel="noreferrer">
              {t.readDocs}
            </a>
          </div>
        </section>
      </main>

      <footer className="foot">
        <div className="foot__links">
          <Out href={lang === 'zh' ? LINKS.docsZh : LINKS.docs} soon={t.soon}>
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
        </div>
        <div className="foot__note">{t.license}</div>
      </footer>
    </div>
  )
}
