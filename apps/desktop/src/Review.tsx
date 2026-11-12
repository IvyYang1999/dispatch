// The review window: one message a session wants to send, held until the user says send.
// It shows who writes to whom and enough to recognize the recipient (its name in its own app,
// its project, when it was last active, the last thing you asked it), how it runs, what will
// happen when the message goes, and the message itself, so a wrong pick shows.
import { useCallback, useEffect, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { LogicalSize } from '@tauri-apps/api/dpi'
import { api, type ReviewInfo } from './api'
import { loadLang, t, useLang, type Key } from './i18n'
import { Avatar, Clamp, Icon, loadIcons, localStamp, plain, presence } from './App'

function modeText(mode: string | null | undefined, running: boolean): string {
  if (!running) return t('review.notRunning')
  if (!mode) return t('review.unknown')
  const key = `mode.${mode}` as Key
  const text = t(key)
  return text === key ? mode : text
}

function clientText(client: string | null | undefined, desktop: boolean, harness: string, running: boolean): string {
  if (!running) return t('review.notRunning')
  const c = client ?? (harness === 'Claude' ? (desktop ? 'desktop' : 'terminal') : null)
  if (!c) return t('review.unknown')
  const key = `client.${c}` as Key
  const text = t(key)
  return text === key ? c : text
}

/** `3 分钟前`: how long ago, from Unix milliseconds. */
export function ago(ms: number): string {
  const m = Math.floor((Date.now() - ms) / 60000)
  if (m < 1) return t('ago.now')
  if (m < 60) return t('ago.min', { n: m })
  const h = Math.floor(m / 60)
  if (h < 24) return t('ago.hour', { n: h })
  return t('ago.day', { n: Math.floor(h / 24) })
}

/** The last folder of a path: what the Claude app files a session under. */
function folder(cwd: string | null | undefined): string | null {
  if (!cwd) return null
  const parts = cwd.split('/').filter(Boolean)
  return parts[parts.length - 1] ?? null
}

/** The stored harness code as the label HarnessMark knows. */
function harnessLabel(code: string | null | undefined): string | null {
  if (!code) return null
  return ({ claude: 'Claude', codex: 'Codex', grok: 'Grok Bot', human: 'Person' } as Record<string, string>)[code] ?? code
}

export function Review() {
  useLang()
  const [ids, setIds] = useState<string[]>([])
  const [info, setInfo] = useState<ReviewInfo | null>(null)
  const [busy, setBusy] = useState(false)
  const [done, setDone] = useState<string | null>(null)
  // what the user just decided, shown on its own small card until closed or timed out
  const [result, setResult] = useState<{ sent: boolean; title: string; detail: string; to: string; more: boolean } | null>(null)
  const showing = useRef(false)
  const [left, setLeft] = useState(5)
  const [hold, setHold] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const current = useRef<string | null>(null)
  const closing = useRef<number | null>(null)
  const [always, setAlways] = useState(false)
  const [note, setNote] = useState<string | null>(null)
  const bar = useRef<HTMLDivElement>(null)
  const body = useRef<HTMLDivElement>(null)
  const foot = useRef<HTMLDivElement>(null)

  const load = useCallback(async () => {
    try {
      const list = await api.reviewList()
      setIds(list)
      const head = list[0] ?? null
      if (showing.current) return
      if (!head) {
        // nothing left: the result stays a moment, then the window goes away
        if (closing.current == null) {
          closing.current = window.setTimeout(() => {
            closing.current = null
            setInfo(null)
            setDone(null)
            current.current = null
            api.reviewClose().catch(() => {})
          }, current.current ? 1400 : 0)
        }
        return
      }
      if (closing.current != null) {
        window.clearTimeout(closing.current)
        closing.current = null
      }
      if (head !== current.current) {
        current.current = head
        setInfo(await api.reviewInfo(head))
        setErr(null)
        setDone(null)
      }
    } catch (e) {
      setErr(String(e))
    }
  }, [])

  useEffect(() => {
    loadLang()
    loadIcons()
    load()
    const un = listen('review:changed', () => load())
    return () => {
      un.then((f) => f())
    }
  }, [load])

  // the window is as tall as what it shows, within the screen
  useEffect(() => {
    if (!info) return
    const fit = () => {
      const content = (bar.current?.offsetHeight ?? 0) + (body.current?.scrollHeight ?? 0) + (foot.current?.offsetHeight ?? 0)
      const most = Math.max(420, (window.screen?.availHeight ?? 900) - 120)
      const h = Math.min(most, Math.max(showing.current ? 180 : 360, Math.ceil(content)))
      getCurrentWindow()
        .setSize(new LogicalSize(window.innerWidth || 480, h))
        .catch(() => {})
    }
    const raf = requestAnimationFrame(fit)
    return () => cancelAnimationFrame(raf)
  }, [info, done, result])

  const openIt = async () => {
    if (!info) return
    try {
      const r = await api.openSession(info.to_host ? `${info.to_host}/${info.to}` : info.to)
      setNote(r.kind === 'command' ? `${r.note ?? ''}: ${r.command}` : r.note ?? null)
    } catch (e) {
      setNote(String(e))
    }
  }

  const decide = async (decision: 'send' | 'trust' | 'decline') => {
    if (!info) return
    setBusy(true)
    try {
      const r = await api.reviewDecide(info.letter_id, decision)
      const sent = decision !== 'decline'
      current.current = info.letter_id
      showing.current = true
      setLeft(5)
      setHold(false)
      setResult({
        sent,
        title: !sent ? t('review.notSent') : decision === 'trust' ? t('review.sentTrusted') : t('review.sentTitle'),
        detail: sent ? r.detail ?? '' : t('review.notSentDetail'),
        to: info.session?.name ?? info.to_name,
        more: ids.length > 1,
      })
    } catch (e) {
      setErr(String(e))
      load()
    } finally {
      setBusy(false)
    }
  }

  // the result card: the next held message after a moment, else close after a few seconds
  const closeResult = useCallback(() => {
    showing.current = false
    setResult(null)
    setInfo(null)
    setDone(null)
    current.current = null
    api.reviewClose().catch(() => {})
    load()
  }, [load])
  useEffect(() => {
    if (!result) return
    if (result.more) {
      const h = window.setTimeout(() => {
        showing.current = false
        setResult(null)
        load()
      }, 1600)
      return () => window.clearTimeout(h)
    }
    if (hold) return
    if (left <= 0) {
      closeResult()
      return
    }
    const h = window.setTimeout(() => setLeft((n) => n - 1), 1000)
    return () => window.clearTimeout(h)
  }, [result, left, hold, closeResult, load])

  if (result) {
    return (
      <div className="review review--result" onMouseEnter={() => setHold(true)} onMouseLeave={() => setHold(false)}>
        <div className="review__bar" data-tauri-drag-region ref={bar} />
        <div className="review__body review__result" ref={body}>
          <svg className="tick" data-sent={result.sent || undefined} width="44" height="44" viewBox="0 0 44 44" aria-hidden="true">
            <circle className="tick__ring" cx="22" cy="22" r="20" />
            {result.sent ? <path className="tick__mark" d="M13 22.5l6 6 12-13" /> : <path className="tick__mark" d="M15 22h14" />}
          </svg>
          <div className="review__result-title">{result.title}</div>
          <div className="review__result-to">{t('review.resultTo', { to: plain(result.to) })}</div>
          {result.detail && <div className="review__result-detail">{result.detail}</div>}
        </div>
        <div className="review__foot review__foot--result" ref={foot}>
          <span className="review__result-left">{result.more ? t('review.nextComing') : hold ? t('review.resultHeld') : t('review.closingIn', { n: left })}</span>
          {!result.more && <button className="btn btn--primary" autoFocus onClick={closeResult}>{t('action.close')}</button>}
        </div>
      </div>
    )
  }

  const s = info?.session ?? null
  const running = s?.running ?? false
  const where = info?.to_host_name ?? t('side.local')
  const level = s?.reach.level ?? 'later'
  const shown = s?.name ?? info?.to_name ?? ''
  const appName = s?.app_name && s.app_name !== shown ? s.app_name : null
  const project = s?.project ?? folder(s?.cwd)
  const runs = s ? (running ? [clientText(s.reach.client, s.desktop, s.harness, running), modeText(s.reach.mode, running)].filter((x) => x !== t('review.unknown')).join(' · ') || t('review.unknown') : t('review.notRunning')) : t('review.unknown')
  const q = info?.last_query ?? null

  return (
    <div className="review">
      <div className="review__bar" data-tauri-drag-region ref={bar}>
        <span className="review__title" data-tauri-drag-region>{t('review.title')}</span>
        {ids.length > 1 && <span className="review__more">{t('review.waiting', { n: ids.length - 1 })}</span>}
      </div>
      {!info && !err && <div className="empty">{t('review.none')}</div>}
      {err && <div className="notice">{err}</div>}
      {info && (
        <div className="review__body" ref={body}>
          <div className="review__who">
            <div className="review__party">
              <Avatar name={info.from_name} look={info.from_avatar} size={30} unknown={info.from_unknown} harness={harnessLabel(info.from_harness)} />
              <div className="review__names">
                <span className="review__name">{plain(info.from_name)}</span>
                <span className="review__sub">{t('review.wantsTo')}</span>
              </div>
            </div>
            <div className="review__arrow">→</div>
            <div className="review__party">
              <Avatar name={shown} look={s?.avatar} appName={s?.app_name} size={30} harness={s?.harness ?? harnessLabel(info.to_harness)} />
              <div className="review__names">
                <span className="review__name">{plain(shown)}</span>
                <span className="review__sub review__sub--open">
                  <span className="review__where" title={where ?? undefined}>{where}</span>
                  <button className="review__open" title={t('review.open')} onClick={openIt}>
                    {t('review.open')}
                    <Icon name="open" size={11} />
                  </button>
                </span>
              </div>
            </div>
          </div>
          {info.first_time && <div className="review__first">{t('review.firstTime')}</div>}

          <dl className="review__facts">
            {appName && (
              <>
                <dt>{t('review.inApp', { app: s?.harness ?? '' })}</dt>
                <dd>{appName}</dd>
              </>
            )}
            {project && (
              <>
                <dt>{t('review.project')}</dt>
                <dd className="review__one" title={s?.cwd ?? ''}>
                  {project}
                  {s?.cwd_short && <span className="review__dim"> · {s.cwd_short.endsWith(`/${project}`) ? s.cwd_short.slice(0, -project.length - 1) : s.cwd_short}</span>}
                </dd>
              </>
            )}
            <dt>{t('review.active')}</dt>
            <dd>{s ? (s.last_active ? `${ago(s.last_active)} · ${presence(s)}` : presence(s)) : t('review.unknown')}</dd>
            {q && (
              <>
                <dt>{t('review.lastQuery')}</dt>
                <dd className="review__query">
                  <span className="review__clamp">{q.text}</span>
                  <span className="review__dim">
                    {localStamp(q.at)}
                    {q.via_dispatch ? ` · ${t('review.viaDispatch', { n: q.from_agent ?? '' })}` : ''}
                  </span>
                </dd>
              </>
            )}
            <dt>{t('review.runs')}</dt>
            <dd>{runs}</dd>
            <dt>{t('review.after')}</dt>
            <dd className="review__reach" data-reach={level}>{s?.reach.text ?? t('review.unknown')}</dd>
          </dl>

          <div className="section-title">{t('review.message')}</div>
          <div className="review__msg">
            <Clamp text={info.body} />
          </div>
          {!done && <div className="review__deadline">{t('review.deadline', { t: info.deadline.slice(5) })}</div>}
          {note && <div className="notice" onClick={() => setNote(null)}>{note}</div>}
        </div>
      )}
      {info && (
        <div className="review__foot" ref={foot}>
          {done ? (
            <div className="review__done">{done}</div>
          ) : (
            <>
              {info.from_unknown ? (
                <span className="review__always review__always--note" title={t('review.askEveryTimeTip')}>{t('review.askEveryTime')}</span>
              ) : (
                <label className="review__always">
                  <input type="checkbox" checked={always} disabled={busy || !info.waiting} onChange={(e) => setAlways(e.target.checked)} />
                  <span>{t('review.trustBox')}</span>
                </label>
              )}
              <button className="btn" disabled={busy || !info.waiting} onClick={() => decide('decline')}>{t('review.decline')}</button>
              <button className="btn btn--primary" disabled={busy || !info.waiting} onClick={() => decide(always && !info.from_unknown ? 'trust' : 'send')}>{t('review.send')}</button>
            </>
          )}
        </div>
      )}
    </div>
  )
}
