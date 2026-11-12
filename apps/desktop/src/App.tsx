import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore, type KeyboardEvent } from 'react'
import { listen } from '@tauri-apps/api/event'
import { api, type Kind, type Look, type OpenResult, type Party, type PeerOverview, type Session, type SessionPageData, type SetupStatus, type Thread, type UpdateCheck } from './api'
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

// An emoji at the start of a name ("🪞 Transparent"): the session's own picture.
const LEADING_EMOJI = /^(\p{Extended_Pictographic}(️|‍\p{Extended_Pictographic})*)\s*/u

/** The emoji a name starts with, if any. */
function leadingEmoji(name: string): string | null {
  const m = name.match(LEADING_EMOJI)
  return m ? m[1] : null
}

/** A name without the emoji it starts with, for where the avatar already shows that emoji. */
export function plain(name: string): string {
  const rest = name.replace(LEADING_EMOJI, '')
  return rest.trim() ? rest : name
}

/** A session's picture: the emoji the user chose (or the one its name, or its name in its own
 *  app, starts with) on a soft tint, else its initial on a color of its own; the user's color
 *  wins either way. */
export function Avatar({ name, look, appName, size = 24, harness, unknown }: { name: string; look?: Look | null; appName?: string | null; size?: number; harness?: string | null; unknown?: boolean }) {
  if (harness && harnessKey(harness) !== 'human') {
    return (
      <span className="avatar-wrap" style={{ width: size, height: size }}>
        <Avatar name={name} look={look} appName={appName} size={size} unknown={unknown} />
        <span className="avatar-badge"><HarnessMark harness={harness} size={Math.max(9, Math.round(size * 0.5))} /></span>
      </span>
    )
  }
  if (unknown) {
    return (
      <span className="avatar avatar--unknown" style={{ width: size, height: size, fontSize: size * 0.5 }}>
        ?
      </span>
    )
  }
  const emoji = look?.emoji || leadingEmoji(name) || (appName ? leadingEmoji(appName) : null)
  const color = look?.color || null
  if (emoji) {
    return (
      <span className="avatar avatar--emoji" style={{ width: size, height: size, fontSize: size * 0.58, background: color ?? `hsl(${hue(plain(name))} 40% 92%)` }}>
        {emoji}
      </span>
    )
  }
  return (
    <span className="avatar" style={{ width: size, height: size, fontSize: size * 0.46, background: color ?? `hsl(${hue(name)} 38% 56%)` }}>
      {initial(name)}
    </span>
  )
}

// Icon outlines after Lucide (lucide.dev, ISC license), drawn at 24 and scaled.
const ICONS: Record<string, string> = {
  feed: '<path d="M14 9a2 2 0 0 1-2 2H6l-4 4V4a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2z"/><path d="M18 9h2a2 2 0 0 1 2 2v11l-4-4h-6a2 2 0 0 1-2-2v-1"/>',
  agents: '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>',
  compose: '<path d="M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.375 2.625a1 1 0 0 1 3 3l-9.013 9.014a2 2 0 0 1-.853.505l-2.873.84a.5.5 0 0 1-.62-.62l.84-2.873a2 2 0 0 1 .506-.852z"/>',
  settings: '<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>',
  open: '<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>',
  write: '<rect width="20" height="16" x="2" y="4" rx="2"/><path d="m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7"/>',
  edit: '<path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/><path d="m15 5 4 4"/>',
}

export function Icon({ name, size = 16 }: { name: keyof typeof ICONS; size?: number }) {
  return <svg className="icon" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" dangerouslySetInnerHTML={{ __html: ICONS[name] }} />
}

const SWATCHES = ['#e5484d', '#f76b15', '#ffc53d', '#46a758', '#12a594', '#0090ff', '#6e56cf', '#d6409f', '#8d8d86']
const QUICK_EMOJI = ['🤖', '🧠', '🛠️', '🚀', '📦', '🔍', '📝', '🎨', '🔑', '🧪', '📊', '🌐']

/** What the editor works on: a session's address, the name it shows, and what the address
 *  book already says about it. */
export interface EditTarget {
  address: string
  name: string
  contact?: string | null
  about?: string | null
  avatar?: Look | null
  /** the person themself: a name and a picture, nothing to remove */
  me?: boolean
}

/** Name a session, say what it is for, and pick its picture: one sheet, wherever it opens. */
export function ContactEditor({ target, onClose, onSaved }: { target: EditTarget; onClose: () => void; onSaved: (text: string) => void }) {
  useLang()
  const [name, setName] = useState(target.me ? (target.name === t('party.you') ? '' : target.name) : (target.contact ?? target.name))
  const [about, setAbout] = useState(target.about ?? '')
  const [emoji, setEmoji] = useState(target.avatar?.emoji ?? '')
  const [color, setColor] = useState(target.avatar?.color ?? '')
  const [err, setErr] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const save = async () => {
    if (target.me) {
      setBusy(true)
      try {
        await api.meSet(name.trim() || null, { emoji: emoji.trim() || null, color: color || null })
        onSaved('')
        onClose()
      } catch (e) {
        setErr(String(e))
      } finally {
        setBusy(false)
      }
      return
    }
    if (!name.trim()) {
      setErr(t('editor.needName'))
      return
    }
    setBusy(true)
    try {
      const r = await api.contactSet(name.trim(), target.address, about.trim() || null, { emoji: emoji.trim() || null, color: color || null })
      onSaved(r.text)
      onClose()
    } catch (e) {
      setErr(String(e))
    } finally {
      setBusy(false)
    }
  }
  const remove = async () => {
    if (!target.contact) return
    setBusy(true)
    try {
      await api.contactRemove(target.contact)
      onSaved('')
      onClose()
    } catch (e) {
      setErr(String(e))
    } finally {
      setBusy(false)
    }
  }
  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') save()
    if (e.key === 'Escape') onClose()
  }
  return (
    <div className="sheet" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet__card" role="dialog" aria-label={t('editor.title')}>
        <div className="sheet__head">
          <Avatar name={name || target.name} look={{ emoji: emoji || null, color: color || null }} size={44} />
          <div className="sheet__title">
            <span>{target.me ? t('editor.meTitle') : t('editor.title')}</span>
            <span className="sheet__sub">{target.me ? t('editor.meSub') : t('editor.sub')}</span>
          </div>
        </div>
        <label className="field">
          <span className="field__label">{t('editor.name')}</span>
          <input className="input" autoFocus placeholder={target.me ? t('party.you') : undefined} value={name} onChange={(e) => setName(e.target.value)} onKeyDown={onKey} />
        </label>
        {!target.me && <label className="field">
          <span className="field__label">{t('editor.about')}</span>
          <input className="input" placeholder={t('contact.about')} value={about} onChange={(e) => setAbout(e.target.value)} onKeyDown={onKey} />
        </label>}
        <div className="field">
          <span className="field__label">{t('editor.emoji')}</span>
          <div className="picks">
            <input className="input input--emoji" placeholder={leadingEmoji(target.name) ?? '🙂'} value={emoji} onChange={(e) => setEmoji(e.target.value)} onKeyDown={onKey} />
            {QUICK_EMOJI.map((x) => (
              <button key={x} className="pick" data-on={emoji === x || undefined} onClick={() => setEmoji(emoji === x ? '' : x)}>{x}</button>
            ))}
          </div>
        </div>
        <div className="field">
          <span className="field__label">{t('editor.color')}</span>
          <div className="picks">
            <button className="swatch swatch--auto" data-on={!color || undefined} title={t('editor.auto')} onClick={() => setColor('')}>A</button>
            {SWATCHES.map((c) => (
              <button key={c} className="swatch" data-on={color === c || undefined} style={{ background: c }} onClick={() => setColor(c)} />
            ))}
          </div>
        </div>
        {err && <div className="notice">{err}</div>}
        <div className="sheet__foot">
          {target.contact && <button className="btn btn--ghost btn--danger" disabled={busy} onClick={remove}>{t('editor.remove')}</button>}
          <span className="bar__spacer" />
          <button className="btn btn--ghost" onClick={onClose}>{t('action.cancel')}</button>
          <button className="btn btn--primary" disabled={busy} onClick={save}>{t('action.save')}</button>
        </div>
      </div>
    </div>
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
export async function loadIcons(): Promise<void> {
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

export function HarnessMark({ harness, size = 14 }: { harness?: string | null; size?: number }) {
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
  if (!src) return <span className="hmark hmark--text" style={{ width: size, height: size, fontSize: Math.round(size * 0.62) }} title={harness}>{harness.slice(0, 1)}</span>
  return <img className="hmark" width={size} height={size} src={src} alt={harness} title={harness} />
}

/** In the address book: named by the user, not just remembered for a reply. */
const isContact = (s: Session) => !!s.contact && !s.provisional

export function presence(s: Session): string {
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

/** `13:25` today, `11-08 16:30` before: a time on this machine's clock. */
function when(at: string): string {
  const n = new Date()
  const today = `${n.getFullYear()}-${String(n.getMonth() + 1).padStart(2, '0')}-${String(n.getDate()).padStart(2, '0')}`
  return at.startsWith(today) ? at.slice(11, 16) : at.slice(5, 16)
}

function firstLine(text: string): string {
  return text.split('\n').find((l) => l.trim()) ?? ''
}

/** Under a party's name: its machine, then its name in its own app when the address book
 *  renamed it, else its folder. */
function partySub(p: Party): string {
  const bits = [p.host_name ?? t('side.local')]
  if (p.app_name && p.app_name !== p.name) bits.push(p.app_name)
  else if (p.cwd_short) bits.push(p.cwd_short)
  return bits.join(' · ')
}

/** One side of a conversation, as the review window shows it: picture, name, client, where;
 *  and what can be done with it: open it in its app, write to it, rename it. */
function PartyCard({ p, onCompose, onOpen, onEdit }: { p: Party; onCompose: (to: string) => void; onOpen: (address: string) => void; onEdit: (p: Party) => void }) {
  return (
    <div className="party">
      <Avatar name={p.name} look={p.avatar} appName={p.app_name} size={28} unknown={p.unknown} harness={p.harness} />
      <div className="party__names">
        <span className="party__name">
          <span className="party__label">{plain(p.name)}</span>
        </span>
        {!p.person && <span className="party__sub" title={p.unknown ? t('party.unknownSub') : partySub(p)}>{p.unknown ? t('party.unknownSub') : partySub(p)}</span>}
        {p.person && <span className="party__sub">{t('party.youSub')}</span>}
      </div>
      {(p.address || p.person) && (
        <span className="party__acts">
          {p.address && (
            <>
              <button className="btn btn--ghost btn--small btn--with-icon" title={t('thread.open')} onClick={() => onOpen(p.address!)}>
                <Icon name="open" size={13} />
                {t('action.openShort')}
              </button>
              <button className="btn btn--ghost btn--icon" title={t('thread.write')} onClick={() => onCompose(p.address!)}>
                <Icon name="write" size={14} />
              </button>
            </>
          )}
          <button className="btn btn--ghost btn--icon" title={p.person ? t('editor.meTitle') : t('thread.edit')} onClick={() => onEdit(p)}>
            <Icon name="edit" size={14} />
          </button>
        </span>
      )}
    </div>
  )
}

type Filter = 'all' | 'queued' | 'failed'
const passes = (th: Thread, f: Filter) => f === 'all' || (f === 'failed' ? th.attention === 'failed' : th.attention === 'queued' || th.attention === 'review' || th.attention === 'held')

// The feed: one row per pair who wrote to each other, both as equals; the conversation between
// them on the right, oldest first, each message with where it stands.
/** Messages held for the user's review, at the top of the feed: the way back to the review
 *  window after it was closed, which keeps them held. */
function ReviewStrip() {
  useLang()
  const [n, setN] = useState(0)
  useEffect(() => {
    const load = () => api.reviewList().then((l) => setN(l.length)).catch(() => {})
    load()
    const h = setInterval(load, 5000)
    const uns = ['review:changed', 'dispatch:changed'].map((e) => listen(e, () => load()))
    return () => {
      clearInterval(h)
      uns.forEach((un) => un.then((f) => f()))
    }
  }, [])
  if (n === 0) return null
  return (
    <div className="callout callout--strip">
      <div className="callout__text">
        <span className="callout__title">{n === 1 ? t('feed.reviewOne') : t('feed.reviewMany', { n })}</span>
        <span className="callout__sub">{t('feed.reviewSub')}</span>
      </div>
      <button className="btn btn--primary btn--small" onClick={() => api.reviewOpen().catch(() => {})}>{t('action.review')}</button>
    </div>
  )
}

function Feed({ onCompose, pick }: { onCompose: (to?: string) => void; pick: string | null }) {
  const lang = useLang()
  const [items, setItems] = useState<Thread[]>([])
  const [selected, setSelected] = useState<string | null>(null)
  const [picked, setPicked] = useState<string | null>(null)
  const [filter, setFilter] = useState<Filter>('all')
  const [err, setErr] = useState<string | null>(null)
  const { open, notice, clear, say } = useOpen()
  const [editing, setEditing] = useState<EditTarget | null>(null)
  const scroller = useRef<HTMLDivElement>(null)
  const pinned = useRef(true)
  const edit = (p: Party) => setEditing(p.person ? { address: '', name: p.name, me: true, avatar: p.avatar } : { address: p.address!, name: p.app_name ?? p.name, contact: p.contact, about: p.about, avatar: p.avatar })
  // picked in the menu bar (a dispatch): show the conversation it is in, whatever the filter
  useEffect(() => {
    if (pick) {
      setPicked(pick)
      setFilter('all')
    }
  }, [pick])
  useEffect(() => {
    if (!picked) return
    const th = items.find((x) => x.messages.some((m) => m.dispatch_id === picked))
    if (th) {
      setSelected(th.key)
      setPicked(null)
    }
  }, [picked, items])

  const load = useCallback(async () => {
    try {
      const r = await api.threads()
      setItems(r.threads)
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

  const shown = useMemo(() => items.filter((th) => passes(th, filter)), [items, filter])
  const counts = useMemo(() => ({ queued: items.filter((th) => passes(th, 'queued')).length, failed: items.filter((th) => passes(th, 'failed')).length }), [items])
  const current = shown.find((th) => th.key === selected) ?? shown[0] ?? null

  // a conversation opens at its newest message, and stays there as new ones come in
  useEffect(() => {
    pinned.current = true
  }, [current?.key])
  useEffect(() => {
    const el = scroller.current
    if (el && pinned.current) el.scrollTop = el.scrollHeight
  }, [current?.key, current?.messages.length])

  const act = async (f: () => Promise<unknown>) => {
    try {
      await f()
      setErr(null)
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
      <ReviewStrip />
      <div className="split">
        <div className="list tlist">
          <FirstRun />
          {err && <div className="empty">{err}</div>}
          {!err && shown.length === 0 && <div className="empty">{t('feed.empty')}</div>}
          {shown.map((th) => {
            const speaker = th.last.side === 'a' ? th.a : th.b
            return (
              <div key={th.key} className="row trow" data-selected={current?.key === th.key || undefined} onClick={() => setSelected(th.key)}>
                <span className="pair">
                  <Avatar name={th.a.name} look={th.a.avatar} appName={th.a.app_name} size={22} unknown={th.a.unknown} harness={th.a.harness} />
                  <Avatar name={th.b.name} look={th.b.avatar} appName={th.b.app_name} size={22} unknown={th.b.unknown} harness={th.b.harness} />
                </span>
                <div className="row__main">
                  <div className="row__line">
                    <span className="trow__names">
                      <span className="trow__name" title={th.a.host_name ?? ''}>{plain(th.a.name)}</span>
                      <span className="trow__sep">⇄</span>
                      <span className="trow__name" title={th.b.host_name ?? ''}>{plain(th.b.name)}</span>
                    </span>
                  </div>
                  <div className="row__line">
                    <span className="row__text trow__last">
                      <span className="trow__speaker">{plain(speaker.name)}：</span>
                      {firstLine(th.last.body)}
                    </span>
                    {th.attention && <Badge kind={th.attention} />}
                    <span className="row__time">{when(th.last_at)}</span>
                  </div>
                </div>
              </div>
            )
          })}
        </div>
        <aside className="convo">
          {current ? (
            <>
              <div className="convo__head">
                <PartyCard p={current.a} onCompose={onCompose} onOpen={open} onEdit={edit} />
                <PartyCard p={current.b} onCompose={onCompose} onOpen={open} onEdit={edit} />
              </div>
              <div
                className="convo__msgs"
                ref={scroller}
                onScroll={(e) => {
                  const el = e.currentTarget
                  pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40
                }}
              >
                {current.count > current.messages.length && <div className="convo__more">{t('thread.more', { n: current.count - current.messages.length })}</div>}
                {current.messages.map((m) => {
                  const who = m.side === 'a' ? current.a : current.b
                  return (
                    <div key={m.letter_id} className="tmsg">
                      <Avatar name={who.name} look={who.avatar} appName={who.app_name} size={28} unknown={who.unknown} harness={who.harness} />
                      <div className="tmsg__main">
                      <div className="tmsg__head">
                        <span className="tmsg__who">{plain(who.name)}</span>
                        <span className="tmsg__at">{when(m.at)}</span>
                      </div>
                      <div className="tmsg__body">
                        <Clamp text={m.body} />
                      </div>
                      <div className="tmsg__foot">
                        <span className="tmsg__stage" data-kind={m.kind}>{m.stage}</span>
                        {m.also_to && m.also_to.length > 0 && <span className="tmsg__also">{t('thread.alsoTo', { n: m.also_to.join('、') })}</span>}
                        {(m.kind === 'queued' || m.kind === 'failed') && (
                          <>
                            <button className="btn btn--ghost btn--small" onClick={() => act(() => api.retry(m.letter_id))}>{t('action.retry')}</button>
                            <button className="btn btn--ghost btn--small" onClick={() => act(() => api.cancel(m.letter_id))}>{t('action.cancel')}</button>
                          </>
                        )}
                        {m.kind === 'review' && (
                          <>
                            <button className="btn btn--primary btn--small" onClick={() => act(() => api.reviewOpen())}>{t('action.review')}</button>
                            <button className="btn btn--ghost btn--small" onClick={() => act(() => api.cancel(m.letter_id))}>{t('action.cancel')}</button>
                          </>
                        )}
                        {m.kind === 'held' && (
                          <>
                            <button className="btn btn--primary btn--small" onClick={() => act(() => api.retry(m.letter_id))}>{t('action.approve')}</button>
                            <button className="btn btn--ghost btn--small" onClick={() => act(() => api.cancel(m.letter_id))}>{t('action.reject')}</button>
                          </>
                        )}
                      </div>
                      </div>
                    </div>
                  )
                })}
              </div>
              {notice && (
                <div className="notice convo__notice" onClick={clear}>
                  {notice}
                </div>
              )}
            </>
          ) : (
            <div className="empty">{t('thread.pick')}</div>
          )}
        </aside>
      </div>
      {editing && <ContactEditor target={editing} onClose={() => setEditing(null)} onSaved={(text) => { say(text); load() }} />}
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
  const [editing, setEditing] = useState<EditTarget | null>(null)
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
  const shown = base.filter((s) => tool === 'all' || (tool === 'contacts' ? isContact(s) : s.harness === tool))
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
  return (
    <>
      <header className="bar" data-tauri-drag-region>
        <span className="bar__title" data-tauri-drag-region>{t('nav.agents')}</span>
        <span className="tabs">
          <button className="tab" data-active={tool === 'all' || undefined} onClick={() => setTool('all')}>{t('tabs.all')} <i>{base.length}</i></button>
          <button className="tab" data-active={tool === 'contacts' || undefined} title={t('agents.contactsTip')} onClick={() => setTool('contacts')}>{t('agents.contacts')} <i>{base.filter(isContact).length}</i></button>
          {tools.map((h) => (
            <button key={h} className="tab" data-active={tool === h || undefined} onClick={() => setTool(h)}>{h} <i>{base.filter((s) => s.harness === h).length}</i></button>
          ))}
        </span>
        <span className="bar__spacer" />
        <input className="input input--search" placeholder={t('agents.search')} title={t('agents.searchTip')} value={query} onChange={(e) => setQuery(e.target.value)} />
      </header>
      <div className="lines">
        {groups.map((g) => (
          <div key={g.key}>
            {groups.length > 1 && <div className="lines__group">{g.title} <i>{g.items.length}</i></div>}
            {g.items.map((s) => (
              <div
                key={s.id}
                className="line line--link"
                onClick={(e) => {
                  if ((e.target as HTMLElement).closest('button, input')) return
                  onOpenPage(addressOf(s))
                }}
              >
                <Avatar name={s.name} look={s.avatar} appName={s.app_name} />
                {(
                  <>
                    <span className="line__who" title={s.id}>
                      <span className="line__name">
                        {plain(s.name)}
                        {s.contact && s.provisional && <span className="line__prov" title={t('contact.provisional')}>≈ {s.contact}</span>}
                      </span>
                      {s.about && <span className="line__about">{s.about}</span>}
                    </span>
                    <HarnessMark harness={s.harness} size={16} />
                    <span className="line__where" title={s.cwd ?? ''}>{s.cwd_short ?? ''}</span>
                    <span className="line__presence" data-presence={s.running ? (s.status ?? 'idle') : 'offline'} title={s.running ? undefined : t('presence.offlineTip')}>
                      <span className="line__dot" />
                      {presence(s)}
                    </span>
                    <span className="line__how" data-reach={s.reach.level} title={s.reach.text}>{s.reach.text}</span>
                    <span className="line__actions">
                      <button className="btn btn--ghost btn--small btn--with-icon" title={t('action.open')} onClick={() => open(addressOf(s))}>
                        <Icon name="open" size={13} />
                        {t('action.openShort')}
                      </button>
                      <button className="btn btn--ghost btn--icon" title={t('action.message')} onClick={() => onCompose(addressOf(s))}><Icon name="write" size={14} /></button>
                      <button className="btn btn--ghost btn--icon" title={t('action.edit')} onClick={() => setEditing({ address: addressOf(s), name: s.app_name ?? s.name, contact: s.provisional ? null : s.contact, about: s.about, avatar: s.avatar })}><Icon name="edit" size={14} /></button>
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
      {editing && (
        <ContactEditor
          target={editing}
          onClose={() => setEditing(null)}
          onSaved={(text) => {
            say(text)
            load()
            if (results) search(query)
          }}
        />
      )}
    </>
  )
}

// ---- Session page ----

/** `MM-DD HH:mm` in this machine's time; records carry UTC. */
export function localStamp(iso: string | null | undefined): string {
  if (!iso) return ''
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  const p = (n: number) => String(n).padStart(2, '0')
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`
}

/** Long text folds to a few lines with a toggle. */
export function Clamp({ text }: { text: string }) {
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
  const [editing, setEditing] = useState<EditTarget | null>(null)
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
              <Avatar name={s.name} look={s.avatar} appName={s.app_name} size={40} harness={s.harness} />
              <div className="who__text">
                <div className="who__line">
                  <span className="who__name">{plain(s.name)}</span>
                  {s.host && <span className="tag">{s.host.name}</span>}
                </div>
                {s.about && <div className="who__about">{s.about}</div>}
                <div className="who__meta">
                  {s.cwd_short && <span title={s.cwd ?? ''}>{s.cwd_short}</span>}
                  <span>{presence(s)}</span>
                  <span className="line__how" data-reach={s.reach.level} title={s.reach.text}>{s.reach.text}</span>
                </div>
              </div>
              <span className="who__acts">
                <button className="btn btn--small btn--with-icon" onClick={() => open(addressOf(s))}>
                  <Icon name="open" size={13} />
                  {t('action.open')}
                </button>
                <button className="btn btn--ghost btn--icon" title={t('action.edit')} onClick={() => setEditing({ address: addressOf(s), name: s.app_name ?? s.name, contact: s.provisional ? null : s.contact, about: s.about, avatar: s.avatar })}>
                  <Icon name="edit" size={14} />
                </button>
              </span>
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
      {editing && <ContactEditor target={editing} onClose={() => setEditing(null)} onSaved={() => load()} />}
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
      <header className="bar bar--page" data-tauri-drag-region>
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
          <div className="section-head__title">{t('settings.review')}</div>
        </div>
        <div className="card">
          <div className="host">
            <div className="host__who">
              <span className="host__name">{st?.review === 'off' ? t('settings.reviewOff') : st?.review === 'all' ? t('settings.reviewAll') : t('settings.reviewNew')}</span>
              <span className="host__found host__found--wrap">{t('settings.reviewSub')}</span>
            </div>
            <span className="seg">
              <button data-active={(st?.review ?? 'new') === 'new' || undefined} onClick={() => run('review', async () => { await api.setReview('new') })}>{t('settings.reviewNew')}</button>
              <button data-active={st?.review === 'all' || undefined} onClick={() => run('review', async () => { await api.setReview('all') })}>{t('settings.reviewAll')}</button>
              <button data-active={st?.review === 'off' || undefined} onClick={() => run('review', async () => { await api.setReview('off') })}>{t('settings.reviewOff')}</button>
            </span>
          </div>
          {(st?.review_trusted ?? 0) > 0 && (
            <div className="host">
              <div className="host__who">
                <span className="host__found host__found--wrap">{t('settings.reviewTrusted', { n: st?.review_trusted ?? 0 })}</span>
              </div>
              <button className="btn btn--small" onClick={() => run('review', async () => { await api.reviewForget() })}>{t('settings.reviewForget')}</button>
            </div>
          )}
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

const NAV: { key: Screen; label: 'nav.feed' | 'nav.agents' | 'nav.compose' | 'nav.settings'; icon: 'feed' | 'agents' | 'compose' | 'settings' }[] = [
  { key: 'feed', label: 'nav.feed', icon: 'feed' },
  { key: 'agents', label: 'nav.agents', icon: 'agents' },
  { key: 'compose', label: 'nav.compose', icon: 'compose' },
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
        // only what waits on the user: a message held for review, or one held for approval
        setQueued(f.dispatches.filter((d) => d.recipients.some((r) => r.kind === 'review' || r.kind === 'held')).length)
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

  // the address book first, most recently active on top; then recent sessions not in it, per machine
  const contacts = live.filter(isContact).sort((a, b) => b.last_active - a.last_active)
  const others = live.filter((s) => !isContact(s))
  const sideRow = (s: Session) => (
    <div key={`${s.host?.id ?? ''}|${s.id}`} className="agent agent--link" title={[s.host?.name, s.cwd].filter(Boolean).join(' · ')} data-active={(screen === 'session' && sessionId === addressOf(s)) || undefined} onClick={() => openPage(addressOf(s))}>
      <Avatar name={s.name} look={s.avatar} appName={s.app_name} size={20} harness={s.harness} />
      <span className="agent__name">{plain(s.name)}</span>
      <span className="presence" data-on={s.running || undefined} title={s.running ? t('presence.onTip') : t('presence.offlineTip')} />
    </div>
  )

  return (
    <div className="shell">
      <nav className="side" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region>Dispatch</div>
        <div className="group" data-tauri-drag-region>
          {NAV.map((n) => (
            <button key={n.key} className="nav" data-active={screen === n.key || undefined} onClick={() => (n.key === 'compose' ? compose() : setScreen(n.key))}>
              <span className="nav__icon"><Icon name={n.icon} size={17} /></span>
              <span className="nav__label">{t(n.label)}</span>
              {n.key === 'feed' && queued > 0 && <span className="count" title={t('nav.waiting', { n: queued })}>{queued}</span>}
            </button>
          ))}
        </div>
        <div className="side__sessions">
          {contacts.length > 0 && (
            <div className="group">
              <div className="section" title={t('agents.contactsTip')}>{t('agents.contacts')}</div>
              {contacts.map(sideRow)}
            </div>
          )}
          {others.some((s) => !s.host) && (
            <div className="group">
              <div className="section">{t('side.local')}</div>
              {others.filter((s) => !s.host).slice(0, 8).map(sideRow)}
            </div>
          )}
          {Array.from(new Map(others.filter((s) => s.host).map((s) => [s.host!.id, s.host!])).values()).map((h) => (
            <div key={h.id} className="group">
              <div className="section">{h.name}</div>
              {others.filter((s) => s.host?.id === h.id).slice(0, 5).map(sideRow)}
            </div>
          ))}
        </div>
        <div className="side__foot">
          <button className="nav" data-active={screen === 'settings' || undefined} onClick={() => setScreen('settings')}>
            <span className="nav__icon"><Icon name="settings" size={17} /></span>
            <span className="nav__label">{t('nav.settings')}</span>
          </button>
        </div>
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
