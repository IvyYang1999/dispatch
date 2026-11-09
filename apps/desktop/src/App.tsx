import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore, type KeyboardEvent } from 'react'
import { listen } from '@tauri-apps/api/event'
import { api, type Copy, type Dispatch, type Kind, type OpenResult, type PeerOverview, type Session, type SessionPageData, type SetupStatus, type UpdateCheck } from './api'
import { loadLang, setLang, t, useLang, type Lang } from './i18n'

type Screen = 'feed' | 'agents' | 'compose' | 'settings' | 'session'

const kindText = (k: Kind) => t(`kind.${k}` as const)

function initial(name: string): string {
  const s = name.replace(/^[^\p{L}\p{N}]+/u, '').trim()
  return (s[0] ?? '?').toUpperCase()
}

// A muted color of its own per name, so a list reads as different people
function hue(name: string): number {
  let h = 0
  for (const c of name) h = (h * 31 + c.codePointAt(0)!) % 360
  return h
}

function Avatar({ name, size = 24 }: { name: string; size?: number }) {
  return (
    <span className="avatar" style={{ width: size, height: size, fontSize: size * 0.46, background: `hsl(${hue(name)} 38% 56%)` }}>
      {initial(name)}
    </span>
  )
}

function Badge({ kind, text }: { kind: Kind; text?: string }) {
  return (
    <span className="badge" data-kind={kind}>
      <span className="badge__dot" />
      {text ?? kindText(kind)}
    </span>
  )
}

function summary(rs: Copy[]): string {
  const replied = rs.filter((r) => r.kind === 'replied').length
  if (rs.length === 1) return rs[0].stage
  if (replied === rs.length) return t('summary.all')
  return t('summary.some', { n: replied, m: rs.length })
}

/** `machine/id` for a session elsewhere, so the backend looks there; the id alone here. */
/** How the app names a session to the core: a machine by its id, which stays exact when two
 *  pairings share a name. */
function addressOf(s: Session): string {
  return s.host ? `${s.host.id}/${s.id}` : s.id
}

/** A name with its machine behind it when the session is elsewhere. */
function withHost(name: string, host?: string | null): string {
  return host ? `${name} @ ${host}` : name
}

// The clients' own icons, taken from the installed apps by the backend; a text tag where none is.
let icons: Record<string, string> = {}
const iconListeners = new Set<() => void>()
async function loadIcons(): Promise<void> {
  try {
    icons = await api.harnessIcons()
    iconListeners.forEach((l) => l())
  } catch {
    /* text tags stay */
  }
}
function useIcons(): Record<string, string> {
  return useSyncExternalStore(
    (cb) => {
      iconListeners.add(cb)
      return () => iconListeners.delete(cb)
    },
    () => icons,
    () => icons,
  )
}
function harnessKey(label: string): string {
  if (label === 'Claude') return 'claude'
  if (label === 'Codex') return 'codex'
  if (label === 'Grok Bot') return 'grok'
  return 'human'
}

function HarnessMark({ harness, size = 14 }: { harness?: string | null; size?: number }) {
  const all = useIcons()
  if (!harness) return null
  const key = harnessKey(harness)
  if (key === 'human') {
    return (
      <svg className="hmark hmark--person" width={size} height={size} viewBox="0 0 16 16" aria-label={harness}>
        <title>{harness}</title>
        <circle cx="8" cy="5" r="3" fill="currentColor" />
        <path d="M2 14c0-3.3 2.7-5 6-5s6 1.7 6 5z" fill="currentColor" />
      </svg>
    )
  }
  const src = all[key]
  if (!src) return <span className="tag">{harness}</span>
  return <img className="hmark" width={size} height={size} src={src} alt={harness} title={harness} />
}

function presence(s: Session): string {
  if (!s.running) return t('presence.offline')
  const st = s.status ?? 'idle'
  if (st === 'idle' || st === 'busy' || st === 'running' || st === 'waiting') return t(`presence.${st}` as const)
  return st
}

function useOpen() {
  const [notice, setNotice] = useState<string | null>(null)
  const say = useCallback((text: string) => setNotice(text || null), [])
  const open = useCallback(async (id: string) => {
    try {
      const r: OpenResult = await api.openSession(id)
      if (r.kind === 'command') setNotice(`${r.note ?? ''}: ${r.command}`)
      else if ((r.kind === 'app' || r.kind === 'note') && r.note) setNotice(r.note)
      else setNotice(null)
    } catch (e) {
      setNotice(String(e))
    }
  }, [])
  return { open, notice, clear: () => setNotice(null), say }
}

// ---- Feed ----

// Shown until the command line and the hook are installed: the one thing a first run needs.
function FirstRun() {
  useLang()
  const [st, setSt] = useState<SetupStatus | null>(null)
  const [msg, setMsg] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    api.setupStatus().then(setSt).catch(() => setSt(null))
  }, [])
  if (!st || !st.bundled_cli || (st.binary && st.hook_installed)) return null
  const install = async () => {
    setBusy(true)
    try {
      await api.installCli()
      setMsg(t('firstrun.done'))
      setSt(await api.setupStatus())
    } catch (e) {
      setMsg(String(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="callout">
      <div className="callout__text">
        <span className="callout__title">{t('firstrun.title')}</span>
        <span className="callout__sub">{msg ?? t('firstrun.sub')}</span>
      </div>
      <button className="btn btn--primary btn--small" disabled={busy} onClick={install}>{t('settings.install')}</button>
    </div>
  )
}

function Feed({ onCompose, pick }: { onCompose: (to?: string) => void; pick: string | null }) {
  const lang = useLang()
  const [items, setItems] = useState<Dispatch[]>([])
  const [selected, setSelected] = useState<string | null>(null)
  const [filter, setFilter] = useState<'all' | 'queued' | 'failed'>('all')
  const [err, setErr] = useState<string | null>(null)
  const { open, notice, clear } = useOpen()
  // picked in the menu bar: show it whatever the filter
  useEffect(() => {
    if (pick) {
      setSelected(pick)
      setFilter('all')
    }
  }, [pick])

  const load = useCallback(async () => {
    try {
      const r = await api.feed()
      setItems(r.dispatches)
      setErr(null)
    } catch (e) {
      setErr(String(e))
    }
  }, [lang])
  useEffect(() => {
    load()
    const h = setInterval(load, 5000)
    const un = listen('dispatch:changed', () => load())
    return () => {
      clearInterval(h)
      un.then((f) => f())
    }
  }, [load])

  const shown = useMemo(() => {
    const pass = (d: Dispatch) => filter === 'all' || d.recipients.some((r) => r.kind === filter)
    const list = items.filter(pass)
    // queued and failed first: that is what a person comes to see
    const weight = (d: Dispatch) => (d.recipients.some((r) => r.kind === 'failed') ? 0 : d.recipients.some((r) => r.kind === 'queued') ? 1 : 2)
    return [...list].sort((a, b) => weight(a) - weight(b) || b.created.localeCompare(a.created))
  }, [items, filter])
  const counts = useMemo(
    () => ({ queued: items.filter((d) => d.recipients.some((r) => r.kind === 'queued')).length, failed: items.filter((d) => d.recipients.some((r) => r.kind === 'failed')).length }),
    [items],
  )
  const current = shown.find((d) => d.dispatch_id === selected) ?? shown[0] ?? null
  const act = async (f: () => Promise<{ text: string }>) => {
    try {
      const r = await f()
      setErr(null)
      if (r.text) {
        /* the feed shows the outcome */
      }
    } catch (e) {
      setErr(String(e))
    }
    load()
  }

  return (
    <>
      <header className="bar" data-tauri-drag-region>
        <span className="bar__title" data-tauri-drag-region>{t('nav.feed')}</span>
        <span className="tabs">
          <button className="tab" data-active={filter === 'all' || undefined} onClick={() => setFilter('all')}>{t('tabs.all')}</button>
          <button className="tab" data-active={filter === 'queued' || undefined} onClick={() => setFilter('queued')}>{t('tabs.queued')} <i>{counts.queued}</i></button>
          <button className="tab" data-active={filter === 'failed' || undefined} onClick={() => setFilter('failed')}>{t('tabs.failed')} <i>{counts.failed}</i></button>
        </span>
        <span className="bar__spacer" />
        <button className="btn btn--primary" onClick={() => onCompose()}>{t('feed.send')}</button>
      </header>
      <div className="split">
        <div className="list">
          <FirstRun />
          {err && <div className="empty">{err}</div>}
          {!err && shown.length === 0 && <div className="empty">{t('feed.empty')}</div>}
          {shown.map((d) => (
            <div key={d.dispatch_id} className="row" data-selected={current?.dispatch_id === d.dispatch_id || undefined} onClick={() => setSelected(d.dispatch_id)}>
              <Avatar name={d.from_name} />
              <div className="row__main">
                <div className="row__line">
                  <span className="row__from">{withHost(d.from_name, d.from_host_name)}</span>
                  <HarnessMark harness={d.from_harness} />
                  <span className="row__text">{d.text}</span>
                  <span className="row__time">{d.created.slice(11)}</span>
                </div>
                <div className="row__rcpts">
                  {d.recipients.map((r) => (
                    <span key={r.letter_id} className="chip" data-kind={r.kind} title={r.to_harness ?? ''}>
                      <span className="chip__dot" />
                      {r.to_host_name && <span className="chip__host">{r.to_host_name}/</span>}
                      {r.to_name}
                    </span>
                  ))}
                  {d.recipients.length <= 2 && <span className="row__sum">{summary(d.recipients)}</span>}
                </div>
              </div>
            </div>
          ))}
        </div>
        <aside className="detail">
          {current ? (
            <>
              <div className="detail__head">
                <Avatar name={current.from_name} size={28} />
                <div className="detail__who">
                  <span className="detail__name">
                    {withHost(current.from_name, current.from_host_name)}
                    <HarnessMark harness={current.from_harness} />
                  </span>
                  <span className="detail__meta">
                    {current.created} · {current.recipients.length === 1 ? t('feed.sentOne') : t('feed.sentMany', { n: current.recipients.length })}
                  </span>
                </div>
              </div>
              <p className="detail__body">{current.text}</p>
              <div className="rule" />
              <div className="section-title">{t('feed.recipients')}</div>
              {current.recipients.map((r) => (
                <div key={r.letter_id} className="rcpt">
                  <div className="rcpt__head">
                    <Avatar name={r.to_name} size={20} />
                    <span className="rcpt__name">{r.to_host_name && <span className="rcpt__host">{r.to_host_name}/</span>}{r.to_name}</span>
                    <HarnessMark harness={r.to_harness} />
                    <Badge kind={r.kind} />
                    <span className="rcpt__time">{(r.reply?.created ?? r.created).slice(11)}</span>
                    {(r.kind === 'queued' || r.kind === 'failed') && (
                      <span className="rcpt__acts">
                        <button className="btn btn--ghost btn--small" onClick={() => act(() => api.retry(r.letter_id))}>{t('action.retry')}</button>
                        <button className="btn btn--ghost btn--small" onClick={() => act(() => api.cancel(r.letter_id))}>{t('action.cancel')}</button>
                      </span>
                    )}
                    {r.kind === 'held' && (
                      <span className="rcpt__acts">
                        <button className="btn btn--primary btn--small" onClick={() => act(() => api.retry(r.letter_id))}>{t('action.approve')}</button>
                        <button className="btn btn--ghost btn--small" onClick={() => act(() => api.cancel(r.letter_id))}>{t('action.reject')}</button>
                      </span>
                    )}
                    <button className="btn btn--ghost btn--icon" title={t('action.open')} onClick={() => open(r.to_host ? `${r.to_host}/${r.to}` : r.to)}>↗</button>
                  </div>
                  {r.reply ? (
                    <>
                      <p className="rcpt__reply">{r.reply.body}</p>
                      <div className="rcpt__foot">
                        <button className="btn btn--ghost btn--small" onClick={() => onCompose(r.to_host ? `${r.to_host}/${r.to}` : r.to)}>{t('action.reply')}</button>
                      </div>
                    </>
                  ) : (
                    <p className="rcpt__note">{r.stage}</p>
                  )}
                </div>
              ))}
              {notice && (
                <div className="notice" onClick={clear}>
                  {notice}
                </div>
              )}
            </>
          ) : (
            <div className="empty">{t('feed.pick')}</div>
          )}
        </aside>
      </div>
    </>
  )
}

// ---- Agents ----

function Agents({ onCompose, onOpenPage }: { onCompose: (to: string) => void; onOpenPage: (id: string) => void }) {
  const lang = useLang()
  const [sessions, setSessions] = useState<Session[]>([])
  const [tool, setTool] = useState<string>('all')
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<Session[] | null>(null)
  const [editing, setEditing] = useState<{ id: string; name: string; about: string } | null>(null)
  const { open, notice, clear, say } = useOpen()
  const load = useCallback(async () => {
    try {
      const r = await api.sessions(true)
      setSessions(r.sessions)
    } catch {
      /* keep the last list */
    }
  }, [lang])
  useEffect(() => {
    load()
    const h = setInterval(load, 10000)
    return () => clearInterval(h)
  }, [load])
  // the search box looks through every session with records here, whatever its age
  const search = useCallback(async (q: string) => {
    if (!q.trim()) {
      setResults(null)
      return
    }
    try {
      const r = await api.searchSessions(q)
      setResults(r.sessions)
    } catch {
      setResults([])
    }
  }, [])
  useEffect(() => {
    const h = setTimeout(() => search(query), 200)
    return () => clearTimeout(h)
  }, [query, search, lang])
  const base = results ?? sessions
  const tools = useMemo(() => Array.from(new Set(base.map((s) => s.harness))), [base])
  const shown = base.filter((s) => tool === 'all' || s.harness === tool)
  // this machine first, then each paired machine under its own name
  const groups = useMemo(() => {
    const m = new Map<string, { key: string; title: string; items: Session[] }>()
    for (const s of shown) {
      const key = s.host?.id ?? 'local'
      if (!m.has(key)) m.set(key, { key, title: s.host?.name ?? t('side.local'), items: [] })
      m.get(key)!.items.push(s)
    }
    return Array.from(m.values())
  }, [shown, lang])
  const save = async (s: Session) => {
    if (!editing) return
    const { name, about } = editing
    setEditing(null)
    try {
      if (name.trim()) {
        const r = await api.contactSet(name.trim(), addressOf(s), about.trim() || null)
        say(r.text)
      } else if (s.contact) {
        await api.contactRemove(s.contact)
      }
    } catch (e) {
      say(String(e))
    }
    load()
    if (results) search(query)
  }
  const onKey = (s: Session) => (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') save(s)
    if (e.key === 'Escape') setEditing(null)
  }
  return (
    <>
      <header className="bar" data-tauri-drag-region>
        <span className="bar__title" data-tauri-drag-region>{t('nav.agents')}</span>
        <span className="tabs">
          <button className="tab" data-active={tool === 'all' || undefined} onClick={() => setTool('all')}>{t('tabs.all')} <i>{base.length}</i></button>
          {tools.map((h) => (
            <button key={h} className="tab" data-active={tool === h || undefined} onClick={() => setTool(h)}>{h} <i>{base.filter((s) => s.harness === h).length}</i></button>
          ))}
        </span>
        <span className="bar__spacer" />
        <input className="input input--search" placeholder={t('agents.search')} value={query} onChange={(e) => setQuery(e.target.value)} />
      </header>
      <div className="lines">
        {groups.map((g) => (
          <div key={g.key}>
            {groups.length > 1 && <div className="lines__group">{g.title} <i>{g.items.length}</i></div>}
            {g.items.map((s) => (
              <div
                key={s.id}
                className="line line--link"
                data-editing={editing?.id === s.id || undefined}
                onClick={(e) => {
                  if ((e.target as HTMLElement).closest('button, input')) return
                  onOpenPage(addressOf(s))
                }}
              >
                <Avatar name={s.name} />
                {editing?.id === s.id ? (
                  <span className="line__edit">
                    <input className="input" autoFocus placeholder={t('contact.placeholder')} title={t('contact.hint')} value={editing.name} onChange={(e) => setEditing({ ...editing, name: e.target.value })} onKeyDown={onKey(s)} />
                    <input className="input input--about" placeholder={t('contact.about')} value={editing.about} onChange={(e) => setEditing({ ...editing, about: e.target.value })} onKeyDown={onKey(s)} />
                    <button className="btn btn--primary btn--small" onClick={() => save(s)}>{t('action.save')}</button>
                    <button className="btn btn--ghost btn--small" onClick={() => setEditing(null)}>{t('action.cancel')}</button>
                  </span>
                ) : (
                  <>
                    <span className="line__who" title={s.id}>
                      <span className="line__name">
                        {s.contact && !s.provisional && <span className="line__star">★</span>}
                        {s.name}
                        {s.contact && s.provisional && <span className="line__prov" title={t('contact.provisional')}>≈ {s.contact}</span>}
                      </span>
                      {s.about && <span className="line__about">{s.about}</span>}
                    </span>
                    <HarnessMark harness={s.harness} size={16} />
                    <span className="line__where" title={s.cwd ?? ''}>{s.cwd_short ?? ''}</span>
                    <span className="line__presence" data-presence={s.running ? (s.status ?? 'idle') : 'offline'}>
                      <span className="line__dot" />
                      {presence(s)}
                    </span>
                    <span className="line__how" data-reach={s.reach.level} title={s.reach.text}>{s.reach.text}</span>
                    <span className="line__actions">
                      <button className="btn btn--ghost btn--icon" title={s.contact && !s.provisional ? t('action.unname') : t('action.name')} onClick={() => setEditing({ id: s.id, name: s.contact ?? '', about: s.about ?? '' })}>{s.contact && !s.provisional ? '★' : '☆'}</button>
                      <button className="btn btn--ghost btn--icon" title={t('action.message')} onClick={() => onCompose(addressOf(s))}>✎</button>
                      <button className="btn btn--ghost btn--icon" title={t('action.open')} onClick={() => open(addressOf(s))}>↗</button>
                    </span>
                  </>
                )}
              </div>
            ))}
          </div>
        ))}
        {shown.length === 0 && <div className="empty">{results ? t('agents.noResults') : t('agents.empty')}</div>}
        {notice && <div className="notice" onClick={clear}>{notice}</div>}
      </div>
    </>
  )
}

// ---- Session page ----

/** `MM-DD HH:mm` in this machine's time; records carry UTC. */
function localStamp(iso: string | null | undefined): string {
  if (!iso) return ''
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  const p = (n: number) => String(n).padStart(2, '0')
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`
}

/** Long text folds to a few lines with a toggle. */
function Clamp({ text }: { text: string }) {
  useLang()
  const [open, setOpen] = useState(false)
  const long = text.length > 420 || text.split('\n').length > 9
  return (
    <>
      <p className={`msg__text${long && !open ? ' msg__text--clamp' : ''}`}>{text}</p>
      {long && <button className="msg__more" onClick={() => setOpen(!open)}>{open ? t('msg.less') : t('msg.more')}</button>}
    </>
  )
}

function SessionPage({ id, onCompose, onFeedPick, onBack }: { id: string; onCompose: (to: string) => void; onFeedPick: (dispatchId: string) => void; onBack: () => void }) {
  const lang = useLang()
  const [page, setPage] = useState<SessionPageData | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [showDispatches, setShowDispatches] = useState(false)
  const { open, notice, clear } = useOpen()
  const chatRef = useRef<HTMLDivElement>(null)
  const pinned = useRef(true)
  const load = useCallback(() => api.sessionPage(id).then((p) => { setPage(p); setErr(null) }).catch((e) => setErr(String(e))), [id, lang])
  useEffect(() => {
    pinned.current = true
    load()
    const h = setInterval(load, 8000)
    return () => clearInterval(h)
  }, [load])
  // keep the newest exchange in view unless the reader scrolled up
  useEffect(() => {
    const el = chatRef.current
    if (el && pinned.current) el.scrollTop = el.scrollHeight
  }, [page])
  const onScroll = () => {
    const el = chatRef.current
    if (el) pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40
  }
  const s = page?.session
  return (
    <>
      <header className="bar" data-tauri-drag-region>
        <button className="btn btn--ghost btn--small" onClick={onBack}>‹ {t('session.back')}</button>
        <span className="bar__title" data-tauri-drag-region>{s?.name ?? ''}</span>
        <span className="bar__spacer" />
        {s && <button className="btn btn--primary" onClick={() => onCompose(addressOf(s))}>{t('action.message')}</button>}
      </header>
      {err && <div className="empty">{err}</div>}
      {s && page && (
        <div className="spage">
          <div className="spage__head">
            <div className="who">
              <Avatar name={s.name} size={40} />
              <div className="who__text">
                <div className="who__line">
                  <span className="who__name">{s.contact && !s.provisional && <span className="line__star">★</span>}{s.name}</span>
                  <HarnessMark harness={s.harness} size={16} />
                  {s.host && <span className="tag">{s.host.name}</span>}
                </div>
                {s.about && <div className="who__about">{s.about}</div>}
                <div className="who__meta">
                  {s.cwd_short && <span title={s.cwd ?? ''}>{s.cwd_short}</span>}
                  <span>{presence(s)}</span>
                  <span className="line__how" data-reach={s.reach.level} title={s.reach.text}>{s.reach.text}</span>
                </div>
              </div>
              <button className="btn btn--ghost btn--icon" title={t('action.open')} onClick={() => open(addressOf(s))}>↗</button>
            </div>
            {page.dispatches.length > 0 && (
              <div className="spage__dispatches">
                <button className="spage__toggle" onClick={() => setShowDispatches(!showDispatches)}>
                  {showDispatches ? '▾' : '▸'} {t('session.dispatches')} <i>{page.dispatches.length}</i>
                </button>
                {showDispatches && (
                  <div className="spage__dlist">
                    {page.dispatches.map((d) => (
                      <div key={d.dispatch_id} className="row" onClick={() => onFeedPick(d.dispatch_id)}>
                        <Avatar name={d.from_name} size={20} />
                        <div className="row__main">
                          <div className="row__line">
                            <span className="tag tag--dir" data-dir={d.direction}>{d.direction === 'in' ? t('session.in') : t('session.out')}</span>
                            <span className="row__from">{withHost(d.from_name, d.from_host_name)}</span>
                            <HarnessMark harness={d.from_harness} />
                            <span className="row__text">{d.text}</span>
                            <span className="row__time">{d.created.slice(5)}</span>
                          </div>
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            )}
          </div>
          <div className="chat" ref={chatRef} onScroll={onScroll}>
            {!page.transcript_available && <div className="empty">{t('session.noTranscript')}</div>}
            {page.transcript_available && page.rounds.length === 0 && <div className="empty">{t('session.emptyTranscript')}</div>}
            {page.rounds.map((r, i) => {
              const u = r.user
              const at = localStamp(u?.at ?? r.replies[0]?.at)
              return (
                <div key={i} className="exchange">
                  {at && <div className="chat__time">{at}</div>}
                  {u && (
                    <div className="msg msg--prompt" data-dispatch={u.via_dispatch || undefined}>
                      {u.via_dispatch && <div className="msg__from">📨 {t('session.viaDispatch', { from: u.from_agent ?? '?' })}</div>}
                      <div className="msg__bubble">
                        <Clamp text={u.text} />
                      </div>
                    </div>
                  )}
                  <div className="msg msg--reply">
                    <div className="msg__who">
                      <HarnessMark harness={s.harness} size={14} />
                      <span>{s.name}</span>
                    </div>
                    {r.replies.length === 0 ? (
                      <p className="msg__text msg__text--faint">{i === page.rounds.length - 1 ? t('round.pending') : t('round.noText')}</p>
                    ) : (
                      <Clamp text={r.replies.map((x) => x.text).join('\n\n')} />
                    )}
                  </div>
                </div>
              )
            })}
          </div>
          {notice && <div className="notice" onClick={clear}>{notice}</div>}
        </div>
      )}
    </>
  )
}

// ---- Compose ----

const URGENCIES = ['normal', 'urgent'] as const
const DEADLINES = ['default', '30m', '2h', '1d'] as const

function reasonText(c: Session): string {
  if (c.reason === 'host') return t('reason.host', { host: c.host?.name ?? '' })
  if (c.reason === 'name') return t('reason.name')
  if (c.reason === 'cwd') return t('reason.cwd', { cwd: c.cwd_short ?? '' })
  if (c.reason === 'id') return t('reason.id')
  return t('reason.title')
}

function Compose({ presetTo, onSent }: { presetTo?: string; onSent: () => void }) {
  useLang()
  const [query, setQuery] = useState('')
  const [cands, setCands] = useState<Session[]>([])
  const [to, setTo] = useState<Session[]>([])
  const [text, setText] = useState('')
  const [urgency, setUrgency] = useState<(typeof URGENCIES)[number]>('normal')
  const [deadline, setDeadline] = useState<(typeof DEADLINES)[number]>('default')
  const [result, setResult] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (!presetTo) return
    api.find(presetTo).then((r) => {
      const hit = r.candidates.find((c) => c.id === presetTo || addressOf(c) === presetTo) ?? r.candidates[0]
      if (hit) setTo([hit])
    })
  }, [presetTo])
  useEffect(() => {
    if (query.trim().length < 1) {
      setCands([])
      return
    }
    const h = setTimeout(() => api.find(query).then((r) => setCands(r.candidates.filter((c) => !to.some((x) => x.id === c.id)))), 150)
    return () => clearTimeout(h)
  }, [query, to])

  const submit = async () => {
    if (to.length === 0 || !text.trim()) return
    setBusy(true)
    try {
      const r = await api.send(to.map(addressOf), text, urgency, deadline === 'default' ? undefined : deadline)
      setResult(r.recipients.map((x) => `${x.to.address ?? x.to.name}: ${x.detail}`).join('\n'))
      setText('')
      setTimeout(onSent, 1200)
    } catch (e) {
      setResult(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <>
      <header className="bar" data-tauri-drag-region>
        <span className="bar__title" data-tauri-drag-region>{t('nav.compose')}</span>
      </header>
      <div className="form">
        <label className="label">{t('compose.to')}</label>
        <div className="to">
          {to.map((x) => (
            <span key={x.id} className="to__chip">
              {x.address}
              <button onClick={() => setTo(to.filter((y) => y.id !== x.id))}>×</button>
            </span>
          ))}
          <input className="to__input" placeholder={to.length ? t('compose.addMore') : t('compose.toPlaceholder')} value={query} onChange={(e) => setQuery(e.target.value)} />
        </div>
        {cands.length > 0 && (
          <div className="picker">
            {cands.map((c) => (
              <div key={c.id} className="cand" onClick={() => { setTo([...to, c]); setQuery(''); setCands([]) }}>
                <Avatar name={c.name} size={22} />
                <div className="cand__who">
                  <div className="cand__line">
                    <span className="cand__name">{c.address}</span>
                    <HarnessMark harness={c.harness} />
                    <span className="cand__presence">{presence(c)}</span>
                  </div>
                  <span className="cand__reason">{reasonText(c)} · {c.reach.text}</span>
                </div>
              </div>
            ))}
            <div className="picker__hint">{t('picker.hint')}</div>
          </div>
        )}
        <label className="label">{t('compose.body')}</label>
        <textarea className="body" rows={6} placeholder={t('compose.bodyPlaceholder')} value={text} onChange={(e) => setText(e.target.value)} />
        <div className="form__row">
          <span className="label">{t('compose.urgency')}</span>
          <span className="seg">
            {URGENCIES.map((u) => (
              <button key={u} data-active={urgency === u || undefined} onClick={() => setUrgency(u)}>{t(`urgency.${u}` as const)}</button>
            ))}
          </span>
          <span className="label">{t('compose.deadline')}</span>
          <span className="seg" title={t('compose.deadlineHint')}>
            {DEADLINES.map((d) => (
              <button key={d} data-active={deadline === d || undefined} onClick={() => setDeadline(d)}>{t(`deadline.${d}` as const)}</button>
            ))}
          </span>
        </div>
        <div className="form__actions">
          <button className="btn btn--primary" disabled={busy || to.length === 0 || !text.trim()} onClick={submit}>{t('compose.send')}</button>
        </div>
        {result && <pre className="result">{result}</pre>}
      </div>
    </>
  )
}

// ---- Settings ----

const LANGS: { key: Lang; label: string }[] = [
  { key: 'zh', label: '中文' },
  { key: 'en', label: 'English' },
]

function Peers() {
  const lang = useLang()
  const [ov, setOv] = useState<PeerOverview | null>(null)
  const [msg, setMsg] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [renaming, setRenaming] = useState<string | null>(null)
  const [pairing, setPairing] = useState<string | null>(null)
  const [code, setCode] = useState('')
  const [manual, setManual] = useState('')
  const [manualCode, setManualCode] = useState('')
  const [shown, setShown] = useState<{ code: string; until: number } | null>(null)
  const [linkShown, setLinkShown] = useState<{ link: string; until: number } | null>(null)
  const [copied, setCopied] = useState(false)
  const [relay, setRelay] = useState<string | null>(null)
  const [now, setNow] = useState(Date.now())
  const load = useCallback(() => api.peersOverview().then(setOv).catch((e) => setMsg(String(e))), [])
  useEffect(() => {
    load()
    const h = setInterval(load, 10000)
    return () => clearInterval(h)
  }, [load, lang])
  useEffect(() => {
    const h = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(h)
  }, [])
  const run = async (f: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await f()
    } catch (e) {
      setMsg(String(e))
    } finally {
      setBusy(false)
      load()
    }
  }
  const showCode = () =>
    run(async () => {
      const r = await api.peerCode()
      setShown({ code: r.code, until: Date.now() + r.seconds_left * 1000 })
      setMsg(null)
    })
  const makeLink = () =>
    run(async () => {
      const r = await api.peerLink()
      setLinkShown({ link: r.link, until: Date.now() + r.seconds_left * 1000 })
      setCopied(false)
      setMsg(null)
    })
  const copyLink = async () => {
    if (!linkShown) return
    try {
      await navigator.clipboard.writeText(linkShown.link)
      setCopied(true)
    } catch (e) {
      setMsg(String(e))
    }
  }
  const pair = (target: string, c: string | null) =>
    run(async () => {
      const p = await api.peerPair(target, c)
      setMsg(p.verified ? t('peers.pairedWith', { name: p.name }) : t('peers.pairedUnconfirmed', { name: p.name, fp: p.fingerprint }))
      setPairing(null)
      setCode('')
      setManual('')
      setManualCode('')
    })
  const left = shown ? Math.max(0, Math.round((shown.until - now) / 1000)) : 0
  const linkLeft = linkShown ? Math.max(0, Math.round((linkShown.until - now) / 1000)) : 0
  const isLink = manual.trim().startsWith('dispatch-pair:')
  const addrOf = (s: { addrs: string[]; port: number }) => `${s.addrs.find((a) => !a.includes(':')) ?? s.addrs[0] ?? ''}:${s.port}`
  return (
    <>
      <div className="section-head">
        <div className="section-head__title">{t('settings.peers')}</div>
        <div className="section-head__sub">{t('settings.peersSub')}</div>
      </div>
      <div className="card">
        <div className="host">
          <div className="host__who">
            {renaming !== null ? (
              <span className="inline">
                <input className="input" value={renaming} onChange={(e) => setRenaming(e.target.value)} autoFocus />
                <button className="btn btn--small" disabled={busy || !renaming.trim()} onClick={() => run(async () => { await api.peerSetName(renaming); setRenaming(null) })}>{t('peers.save')}</button>
              </span>
            ) : (
              <span className="host__name">{t('peers.me', { name: ov?.me.name ?? '' })}</span>
            )}
            <span className="host__found">{ov?.me.listening ? t('peers.listening', { p: ov.me.listening }) : t('peers.notListening')}</span>
          </div>
          {renaming === null && <button className="btn btn--ghost btn--small" disabled={busy} onClick={() => setRenaming(ov?.me.name ?? '')}>{t('peers.rename')}</button>}
          <button className="btn btn--small" disabled={busy || !ov?.me.listening} onClick={showCode}>{t('peers.showCode')}</button>
          <button className="btn btn--small" disabled={busy || !ov?.internet || !ov?.me.internet_listening} onClick={makeLink}>{t('peers.makeLink')}</button>
        </div>
        {linkShown && linkLeft > 0 && (
          <div className="host host--code host--link">
            <code className="cmd cmd--wrap">{linkShown.link}</code>
            <div className="host__row">
              <span className="host__found host__found--wrap">{t('peers.linkShown', { s: linkLeft })}</span>
              <button className="btn btn--primary btn--small" onClick={copyLink}>{copied ? t('peers.copied') : t('peers.copy')}</button>
            </div>
          </div>
        )}
        {shown && left > 0 && (
          <div className="host host--code">
            <span className="code">{shown.code}</span>
            <span className="host__found host__found--wrap">{t('peers.codeShown', { s: left })}</span>
          </div>
        )}
        {ov?.peers.some((p) => p.blocked) && (
          <div className="host host--code">
            <span className="host__found host__found--wrap">{t('peers.blocked')}</span>
            <button className="btn btn--small" onClick={() => run(api.openLocalNetworkSettings)}>{t('peers.openPrivacy')}</button>
          </div>
        )}
        {ov?.peers.map((p) => (
          <div key={p.id} className="host host--peer">
            <div className="host__who">
              <span className="host__name">
                {p.name} <span className="tag" data-on={p.online || undefined}>{p.online ? t('peers.online') : t('peers.offline')}</span>
                {p.online && p.link && <span className="tag">{t(`peers.link.${p.link}` as const)}</span>}
              </span>
              <span className="host__found host__found--wrap">
                {p.address ?? (p.seen ? addrOf(p.seen) : t('peers.pairedAt', { t: p.paired_at }))}
                {!p.online && p.problem ? ` · ${p.problem}` : ''}
              </span>
              <span className="host__found host__found--wrap">
                <span className="fp">{t('peers.fingerprint', { fp: p.fingerprint })}</span>
                {' · '}
                {p.verified ? t('peers.confirmed') : t('peers.unconfirmed')}
              </span>
              {p.superseded && <span className="host__found host__found--wrap">{t('peers.superseded')}</span>}
              <span className="host__row">
                <span className="host__found">{t('peers.rule')}</span>
                <span className="seg">
                  {(['all', 'contacts', 'ask'] as const).map((r) => (
                    <button key={r} data-active={p.rule === r || undefined} onClick={() => run(() => api.peerRule(p.id, r))}>{t(`peers.rule.${r}` as const)}</button>
                  ))}
                </span>
                {!p.verified && <button className="btn btn--primary btn--small" disabled={busy} onClick={() => run(async () => { const r = await api.peerVerify(p.id); setMsg(t('peers.pairedWith', { name: r.name })) })}>{t('peers.confirm')}</button>}
              </span>
            </div>
            <button className="btn btn--ghost btn--small" disabled={busy} onClick={() => run(() => api.peerRemove(p.id))}>{t('peers.remove')}</button>
          </div>
        ))}
        {ov?.seen.map((s) => (
          <div key={s.id} className="host">
            <div className="host__who">
              <span className="host__name">{s.name || s.id.slice(0, 8)}</span>
              <span className="host__found">{addrOf(s)} · {t('peers.notPaired')}</span>
            </div>
            {pairing === s.id ? (
              <span className="inline">
                <input className="input input--code" placeholder={t('peers.codePlaceholder')} value={code} onChange={(e) => setCode(e.target.value)} autoFocus />
                <button className="btn btn--primary btn--small" disabled={busy || code.trim().length !== 6} onClick={() => pair(addrOf(s), code)}>{t('peers.pair')}</button>
                <button className="btn btn--ghost btn--small" onClick={() => setPairing(null)}>{t('action.cancel')}</button>
              </span>
            ) : (
              <button className="btn btn--primary btn--small" disabled={busy} onClick={() => { setPairing(s.id); setCode('') }}>{t('peers.pair')}</button>
            )}
          </div>
        ))}
        <div className="host">
          <div className="host__who">
            <span className="host__name">{t('peers.manual')}</span>
            <span className="host__found host__found--wrap">{t('peers.manualSub')}</span>
          </div>
          <span className="inline">
            <input className="input" placeholder={t('peers.manualTarget')} value={manual} onChange={(e) => setManual(e.target.value)} />
            {!isLink && <input className="input input--code" placeholder={t('peers.codePlaceholder')} value={manualCode} onChange={(e) => setManualCode(e.target.value)} />}
            <button className="btn btn--small" disabled={busy || !manual.trim() || (!isLink && manualCode.trim().length !== 6)} onClick={() => pair(manual.trim(), isLink ? null : manualCode)}>{t('peers.pair')}</button>
          </span>
          <button className="btn btn--ghost btn--small" disabled={busy} onClick={() => run(api.peerScan)}>{t('peers.scan')}</button>
        </div>
        <div className="host">
          <div className="host__who">
            <span className="host__name">{t('peers.internet')}</span>
            <span className="host__found host__found--wrap">{t('peers.internetSub')}</span>
          </div>
          <span className="seg">
            <button data-active={ov?.internet || undefined} onClick={() => run(() => api.setPeerInternet(true))}>{t('settings.notifyOn')}</button>
            <button data-active={!ov?.internet || undefined} onClick={() => run(() => api.setPeerInternet(false))}>{t('settings.notifyOff')}</button>
          </span>
        </div>
        <div className="host">
          <div className="host__who">
            <span className="host__name">{t('peers.relay')}</span>
            <span className="host__found host__found--wrap">{t('peers.relaySub')}</span>
          </div>
          <span className="inline">
            <input className="input input--relay" placeholder={t('peers.relayDefault')} value={relay ?? ov?.relay ?? ''} onChange={(e) => setRelay(e.target.value)} />
            <button className="btn btn--small" disabled={busy || relay === null} onClick={() => run(async () => { await api.setPeerRelay(relay?.trim() ? relay.trim() : null); setRelay(null) })}>{t('peers.relaySave')}</button>
          </span>
        </div>
        {msg && <div className="notice" onClick={() => setMsg(null)}>{msg}</div>}
      </div>
    </>
  )
}

function Settings() {
  const lang = useLang()
  const [st, setSt] = useState<SetupStatus | null>(null)
  const [msg, setMsg] = useState<string | null>(null)
  const [upd, setUpd] = useState<UpdateCheck | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const load = useCallback(() => api.setupStatus().then((s) => {
    setSt(s)
    if (s.cli_sync_error) setMsg(s.cli_sync_error)
  }).catch((e) => setMsg(String(e))), [])
  useEffect(() => {
    load()
  }, [load])
  const run = async (what: string, f: () => Promise<string | void>) => {
    setBusy(what)
    try {
      const r = await f()
      if (typeof r === 'string') setMsg(r)
      load()
    } catch (e) {
      setMsg(String(e))
    } finally {
      setBusy(null)
    }
  }
  const check = () => run('check', async () => { setUpd(await api.updateCheck()) })
  const cliText = st?.binary ? t('settings.cliInstalled', { p: st.binary }) : st?.bundled_cli ? t('settings.cliBundled') : t('settings.cliNone')
  const codexText = st?.codex ? (st.codex_daemon ? t('settings.codexDaemon', { p: st.codex, n: st.codex_loaded }) : t('settings.codexNoDaemon', { p: st.codex })) : t('settings.codexMissing')
  const grokState = !st ? '' : !st.grok_app ? t('settings.grokMissing') : !st.grok_signed_in ? t('settings.grokSignIn') : !st.gbot ? t('settings.grokNoGbot') : st.grok_error ? t('settings.grokOffline', { e: st.grok_error }) : t('settings.grokReady', { n: st.grok_bots })
  return (
    <>
      <header className="bar" data-tauri-drag-region>
        <span className="bar__title" data-tauri-drag-region>{t('nav.settings')}</span>
      </header>
      <div className="settings">
        <div className="section-head">
          <div className="section-head__title">{t('settings.agents')}</div>
          <div className="section-head__sub">{t('settings.agentsSub')}</div>
        </div>
        <div className="card">
          <div className="host">
            <div className="host__who">
              <span className="host__name">{t('settings.cli')}</span>
              <span className="host__found">{cliText}</span>
            </div>
            <button className="btn btn--primary btn--small" disabled={busy !== null || !st?.bundled_cli} onClick={() => run('install', api.installCli)}>{st?.binary ? t('settings.reinstall') : t('settings.install')}</button>
          </div>
          <div className="host">
            <div className="host__who">
              <span className="host__name">Claude Code</span>
              <span className="host__found">{st?.hook_installed ? t('settings.hookOn') : t('settings.hookOff')}</span>
            </div>
            {st?.hook_installed ? <Badge kind="replied" text={st.claude_plugin ? t('settings.pluginAndHook') : t('settings.hookOn')} /> : <button className="btn btn--small" disabled={busy !== null} onClick={() => run('hook', api.installHook)}>{t('settings.hookOnly')}</button>}
          </div>
          <div className="host">
            <div className="host__who">
              <span className="host__name">{t('settings.inbound')}</span>
              <span className="host__found host__found--wrap">
                {st?.inbound && st.inbound !== 'accept' ? `${t('settings.inboundOther', { v: st.inbound })} ` : ''}
                {t('settings.inboundSub')}
              </span>
            </div>
            <span className="seg">
              <button data-active={st?.inbound === 'accept' || undefined} onClick={() => run('inbound', async () => { await api.setInbound(true) })}>{t('settings.notifyOn')}</button>
              <button data-active={st?.inbound !== 'accept' || undefined} onClick={() => run('inbound', async () => { await api.setInbound(false) })}>{t('settings.notifyOff')}</button>
            </span>
          </div>
          <div className="host">
            <div className="host__who">
              <span className="host__name">Codex</span>
              <span className="host__found">{codexText}</span>
            </div>
            <code className="cmd">codex plugin marketplace add IvyYang1999/dispatch && codex plugin add dispatch</code>
          </div>
          <div className="host">
            <div className="host__who">
              <span className="host__name">{t('settings.claudePlugin')}</span>
              <span className="host__found">{t('settings.claudePluginSub')}</span>
            </div>
            <code className="cmd">/plugin install dispatch --marketplace IvyYang1999/dispatch</code>
          </div>
          <div className="host">
            <div className="host__who">
              <span className="host__name">{t('settings.grok')}</span>
              <span className="host__found">{grokState} {t('settings.grokSub')}</span>
            </div>
            <code className="cmd">{t('settings.grokCmd')}</code>
          </div>
        </div>
        <Peers />
        <div className="section-head">
          <div className="section-head__title">{t('settings.language')}</div>
        </div>
        <div className="card">
          <div className="host">
            <div className="host__who">
              <span className="host__name">{LANGS.find((l) => l.key === lang)?.label}</span>
              <span className="host__found">{t('settings.languageSub')}</span>
            </div>
            <span className="seg">
              {LANGS.map((l) => (
                <button key={l.key} data-active={lang === l.key || undefined} onClick={() => setLang(l.key)}>{l.label}</button>
              ))}
            </span>
          </div>
        </div>
        <div className="section-head">
          <div className="section-head__title">{t('settings.theme')}</div>
        </div>
        <div className="card">
          <div className="host">
            <div className="host__who">
              <span className="host__name">{t(`theme.${st?.theme ?? 'system'}` as const)}</span>
              <span className="host__found">{t('settings.themeSub')}</span>
            </div>
            <span className="seg">
              {(['system', 'light', 'dark'] as const).map((th) => (
                <button key={th} data-active={(st?.theme ?? 'system') === th || undefined} onClick={() => run('theme', async () => { await api.setTheme(th) })}>{t(`theme.${th}` as const)}</button>
              ))}
            </span>
          </div>
        </div>
        <div className="section-head">
          <div className="section-head__title">{t('settings.notify')}</div>
        </div>
        <div className="card">
          <div className="host">
            <div className="host__who">
              <span className="host__name">{st?.notify === false ? t('settings.notifyOff') : t('settings.notifyOn')}</span>
              <span className="host__found host__found--wrap">{t('settings.notifySub')}</span>
            </div>
            <span className="seg">
              <button data-active={st?.notify !== false || undefined} onClick={() => run('notify', async () => { await api.setNotify(true) })}>{t('settings.notifyOn')}</button>
              <button data-active={st?.notify === false || undefined} onClick={() => run('notify', async () => { await api.setNotify(false) })}>{t('settings.notifyOff')}</button>
            </span>
          </div>
        </div>
        <div className="section-head">
          <div className="section-head__title">{t('settings.messages')}</div>
        </div>
        <div className="card">
          <div className="host">
            <div className="host__who">
              <span className="host__name">{t('settings.storeWhere')}</span>
              <span className="host__found">{st?.store_root}</span>
            </div>
          </div>
        </div>
        <div className="section-head">
          <div className="section-head__title">{t('settings.updates')}</div>
        </div>
        <div className="card">
          <div className="host">
            <div className="host__who">
              <span className="host__name">Dispatch {st?.version ?? ''}</span>
              <span className="host__found">
                {upd === null ? t('settings.updateIdle') : upd.available ? `${t('settings.updateAvailable', { v: upd.version ?? '' })}${upd.notes ? `: ${upd.notes}` : ''}` : t('settings.upToDate')}
              </span>
            </div>
            {upd?.available ? (
              <button className="btn btn--primary btn--small" disabled={busy !== null} onClick={() => run('update', api.updateInstall)}>{t('settings.updateRestart')}</button>
            ) : (
              <button className="btn btn--small" disabled={busy !== null} onClick={check}>{busy === 'check' ? t('settings.checking') : t('settings.check')}</button>
            )}
          </div>
        </div>
        {msg && <div className="notice" onClick={() => setMsg(null)}>{msg}</div>}
      </div>
    </>
  )
}

// ---- Shell ----

const NAV: { key: Screen; label: 'nav.feed' | 'nav.agents' | 'nav.compose' | 'nav.settings'; icon: string }[] = [
  { key: 'feed', label: 'nav.feed', icon: '▤' },
  { key: 'agents', label: 'nav.agents', icon: '◉' },
  { key: 'compose', label: 'nav.compose', icon: '✎' },
  { key: 'settings', label: 'nav.settings', icon: '⚙' },
]

export function App() {
  const lang = useLang()
  const [screen, setScreen] = useState<Screen>('feed')
  const [composeTo, setComposeTo] = useState<string | undefined>(undefined)
  const [live, setLive] = useState<Session[]>([])
  const [queued, setQueued] = useState(0)
  const [pick, setPick] = useState<string | null>(null)
  const [sessionId, setSessionId] = useState<string | null>(null)
  const openPage = (id: string) => {
    setSessionId(id)
    setScreen('session')
  }
  const feedPick = (dispatchId: string) => {
    setPick(dispatchId)
    setScreen('feed')
  }

  useEffect(() => {
    loadLang()
    loadIcons()
  }, [])
  // the menu bar item: a dispatch picked from its list, or Send
  useEffect(() => {
    const un1 = listen<string>('tray:select', (e) => {
      setPick(e.payload)
      setScreen('feed')
    })
    const un2 = listen('tray:compose', () => {
      setComposeTo(undefined)
      setScreen('compose')
    })
    return () => {
      un1.then((f) => f())
      un2.then((f) => f())
    }
  }, [])
  useEffect(() => {
    const tick = async () => {
      try {
        const [s, f] = await Promise.all([api.sessions(true), api.feed(100)])
        setLive(s.sessions)
        setQueued(f.dispatches.filter((d) => d.recipients.some((r) => r.kind === 'queued' || r.kind === 'failed')).length)
      } catch {
        /* the backend may not be up yet */
      }
    }
    tick()
    const h = setInterval(tick, 12000)
    return () => clearInterval(h)
  }, [lang])

  const compose = (to?: string) => {
    setComposeTo(to)
    setScreen('compose')
  }

  return (
    <div className="shell">
      <nav className="side" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region>Dispatch</div>
        <div className="group" data-tauri-drag-region>
          {NAV.map((n) => (
            <button key={n.key} className="nav" data-active={screen === n.key || undefined} onClick={() => (n.key === 'compose' ? compose() : setScreen(n.key))}>
              <span className="nav__icon">{n.icon}</span>
              <span className="nav__label">{t(n.label)}</span>
              {n.key === 'feed' && queued > 0 && <span className="count">{queued}</span>}
            </button>
          ))}
        </div>
        <div className="group">
          <div className="section">{t('side.local')}</div>
          {live.filter((s) => !s.host).slice(0, 8).map((s) => (
            <div key={s.id} className="agent agent--link" title={s.cwd ?? ''} data-active={(screen === 'session' && sessionId === addressOf(s)) || undefined} onClick={() => openPage(addressOf(s))}>
              <Avatar name={s.name} size={16} />
              <span className="agent__name">{s.name}</span>
              <span className="presence" data-on={s.running || undefined} />
            </div>
          ))}
        </div>
        {Array.from(new Map(live.filter((s) => s.host).map((s) => [s.host!.id, s.host!])).values()).map((h) => (
          <div key={h.id} className="group">
            <div className="section">{h.name}</div>
            {live.filter((s) => s.host?.id === h.id).slice(0, 5).map((s) => (
              <div key={s.id} className="agent agent--link" title={s.cwd ?? ''} data-active={(screen === 'session' && sessionId === addressOf(s)) || undefined} onClick={() => openPage(addressOf(s))}>
                <Avatar name={s.name} size={16} />
                <span className="agent__name">{s.name}</span>
                <span className="presence" data-on={s.running || undefined} />
              </div>
            ))}
          </div>
        ))}
      </nav>
      <main className="panel">
        {screen === 'feed' && <Feed onCompose={compose} pick={pick} />}
        {screen === 'agents' && <Agents onCompose={compose} onOpenPage={openPage} />}
        {screen === 'session' && sessionId && <SessionPage id={sessionId} onCompose={compose} onFeedPick={feedPick} onBack={() => setScreen('agents')} />}
        {screen === 'compose' && <Compose presetTo={composeTo} onSent={() => setScreen('feed')} />}
        {screen === 'settings' && <Settings />}
      </main>
    </div>
  )
}
