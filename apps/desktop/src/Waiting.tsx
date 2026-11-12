// The reminder for messages waiting for their session to be opened: a Claude Code session that
// is not running, or a Codex thread no window has open. The app opens such sessions itself (see
// dispatch_core::wake); this lists only what needs the user, one row per session with why and
// Open. Later puts it away until a new one needs them.
import { useCallback, useEffect, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { LogicalSize } from '@tauri-apps/api/dpi'
import { api, type WaitItem } from './api'
import { loadLang, t, useLang } from './i18n'
import { Avatar, Icon, loadIcons, plain } from './App'
import { ago, contentHeight } from './Review'

type Group = { session: WaitItem['session']; items: WaitItem[] }

function groups(list: WaitItem[]): Group[] {
  const m = new Map<string, Group>()
  for (const w of list) {
    const g = m.get(w.session.id) ?? { session: w.session, items: [] }
    g.items.push(w)
    m.set(w.session.id, g)
  }
  return [...m.values()]
}

export function Waiting() {
  useLang()
  const [list, setList] = useState<WaitItem[]>([])
  const [mode, setMode] = useState('auto')
  const [note, setNote] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const bar = useRef<HTMLDivElement>(null)
  const body = useRef<HTMLDivElement>(null)
  const foot = useRef<HTMLDivElement>(null)

  const load = useCallback(async () => {
    try {
      const r = await api.waitList()
      setList(r.waiting)
      setMode(r.mode)
    } catch (e) {
      setNote(String(e))
    }
  }, [])

  useEffect(() => {
    loadLang()
    loadIcons()
    load()
    const un = listen('wait:changed', () => load())
    const h = setInterval(load, 5000)
    return () => {
      un.then((f) => f())
      clearInterval(h)
    }
  }, [load])

  // as tall as what it shows
  useEffect(() => {
    const fit = () => {
      const content = (bar.current?.offsetHeight ?? 0) + contentHeight(body.current) + (foot.current?.offsetHeight ?? 0)
      const h = Math.min(520, Math.max(160, Math.ceil(content)))
      getCurrentWindow().setSize(new LogicalSize(window.innerWidth || 420, h)).catch(() => {})
    }
    const raf = requestAnimationFrame(fit)
    return () => cancelAnimationFrame(raf)
  }, [list, note])

  const open = async (g: Group) => {
    setBusy(g.session.id)
    try {
      const r = await api.waitOpen(g.session.id)
      setNote(r.kind === 'command' ? `${r.note ?? ''}: ${r.command}` : r.note ?? null)
      await load()
    } catch (e) {
      setNote(String(e))
    } finally {
      setBusy(null)
    }
  }

  const all = groups(list)
  return (
    <div className="wait">
      <div className="wait__bar" ref={bar} data-tauri-drag-region>
        <span className="wait__title" data-tauri-drag-region>{t('wait.title')}</span>
      </div>
      <div className="wait__body" ref={body}>
        {all.map((g) => {
          const first = g.items[0]
          const from = first.from_host_name ? `${plain(first.from_name)} @ ${first.from_host_name}` : plain(first.from_name)
          return (
            <div key={g.session.id} className="wait__row">
              <Avatar name={g.session.name} look={g.session.avatar} appName={g.session.app_name} size={30} harness={g.session.harness} />
              <div className="wait__who">
                <span className="wait__name">{plain(g.session.name)}</span>
                <span className="wait__sub">
                  {g.items.length > 1 ? t('wait.many', { n: g.items.length, from }) : t('wait.one', { from })}
                  {' · '}
                  {ago(Date.parse(first.since))}
                </span>
                <span className="wait__sub wait__why">{t(`wait.why.${first.why ?? 'ask'}` as const)}</span>
              </div>
              <button className="btn btn--primary btn--small btn--with-icon" disabled={busy !== null} onClick={() => open(g)}>
                <Icon name="open" size={13} />
                {t('review.open')}
              </button>
            </div>
          )
        })}
        {note && <div className="wait__note">{note}</div>}
      </div>
      <div className="wait__foot" ref={foot}>
        <span className="wait__hint">{mode === 'auto' ? t('wait.autoHint') : t('wait.askHint')}</span>
        <button className="btn btn--small" onClick={() => api.waitDismiss()}>{t('wait.later')}</button>
      </div>
    </div>
  )
}
