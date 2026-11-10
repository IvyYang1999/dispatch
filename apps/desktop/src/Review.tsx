// The review window: one message a session wants to send, held until the user says send.
// It shows who writes to whom and enough to recognize the recipient (its name in its own app,
// its project, when it was last active, the last thing you asked it), how it runs, what will
// happen when the message goes, and the message itself, so a wrong pick shows.
import { useCallback, useEffect, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { api, type ReviewInfo } from './api'
import { loadLang, t, useLang, type Key } from './i18n'
import { Avatar, Clamp, HarnessMark, loadIcons, localStamp, presence } from './App'

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
  const [err, setErr] = useState<string | null>(null)
  const current = useRef<string | null>(null)
  const closing = useRef<number | null>(null)

  const load = useCallback(async () => {
    try {
      const list = await api.reviewList()
      setIds(list)
      const head = list[0] ?? null
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

  const decide = async (decision: 'send' | 'trust' | 'decline') => {
    if (!info) return
    setBusy(true)
    try {
      const r = await api.reviewDecide(info.letter_id, decision)
      setDone(decision === 'decline' ? t('review.notSent') : t('review.sent', { detail: r.detail ?? '' }))
      current.current = info.letter_id
      // the next one comes up on the next look
      window.setTimeout(load, 900)
    } catch (e) {
      setErr(String(e))
      load()
    } finally {
      setBusy(false)
    }
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
      <div className="review__bar" data-tauri-drag-region>
        <span className="review__title" data-tauri-drag-region>{t('review.title')}</span>
        {ids.length > 1 && <span className="review__more">{t('review.waiting', { n: ids.length - 1 })}</span>}
      </div>
      {!info && !err && <div className="empty">{t('review.none')}</div>}
      {err && <div className="notice">{err}</div>}
      {info && (
        <div className="review__body">
          <div className="review__who">
            <div className="review__party">
              <Avatar name={info.from_name} size={26} />
              <div className="review__names">
                <span className="review__name">{info.from_name} <HarnessMark harness={harnessLabel(info.from_harness)} /></span>
                <span className="review__sub">{t('review.wantsTo')}</span>
              </div>
            </div>
            <div className="review__arrow">→</div>
            <div className="review__party">
              <Avatar name={shown} size={26} />
              <div className="review__names">
                <span className="review__name">{shown} <HarnessMark harness={s?.harness ?? harnessLabel(info.to_harness)} /></span>
                <span className="review__sub">{where}</span>
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

        </div>
      )}
      {info && (
        <div className="review__foot">
          {done ? (
            <div className="review__done">{done}</div>
          ) : (
            <>
              <div className="review__deadline">{t('review.deadline', { t: info.deadline.slice(5) })}</div>
              <div className="review__acts">
                <button className="btn btn--ghost" disabled={busy || !info.waiting} onClick={() => decide('decline')}>{t('review.decline')}</button>
                <button className="btn btn--ghost" disabled={busy || !info.waiting} onClick={() => decide('trust')}>{t('review.trust')}</button>
                <button className="btn btn--primary" disabled={busy || !info.waiting} onClick={() => decide('send')}>{t('review.send')}</button>
              </div>
            </>
          )}
        </div>
      )}
    </div>
  )
}
