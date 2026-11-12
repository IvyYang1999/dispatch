import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore, type KeyboardEvent } from 'react'
import { listen } from '@tauri-apps/api/event'
import { api, type Group, type Kind, type Look, type MergeSuggestion, type OpenResult, type Party, type PeerOverview, type Session, type SessionPageData, type SetupStatus, type Thread, type UpdateCheck } from './api'
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
// A Grok bot's face, after the Grok Bot app: one of its shapes in one of its colours, with two
// eyes. The app's own drawings are not ours to copy; these are our own outlines of the same idea.
const BOT_COLORS: Record<string, string> = { black: '#16161a', blue: '#2f6bff', cyan: '#14b8a6', gray: '#8b8f98', green: '#22a565', orange: '#ff5b1f', violet: '#7c4dff', yellow: '#f2b01e' }
const BOT_SHAPES: Record<string, string> = {
  blob: '<path d="M25 4c11 0 19 6 19 17 0 13-8 23-21 23C11 44 4 36 4 25 4 13 13 4 25 4z"/>',
  square: '<rect x="6" y="6" width="36" height="36" rx="11"/>',
  hex: '<path d="M24 4.5 40.5 14v20L24 43.5 7.5 34V14z" stroke-width="5" stroke-linejoin="round"/>',
  house: '<path d="M24 6 41 19.5V39a2 2 0 0 1-2 2H9a2 2 0 0 1-2-2V19.5z" stroke-width="5" stroke-linejoin="round"/>',
  star: '<path d="m24 6 5.2 11.3 12.3 1.5-9.1 8.4 2.4 12.2L24 33.300l-10.8 6.1 2.400-12.2-9.100-8.4 12.300-1.500z" stroke-width="6" stroke-linejoin="round"/>',
  flower: '<circle cx="24" cy="12.500" r="9"/><circle cx="35" cy="20.500" r="9"/><circle cx="30.800" cy="33.500" r="9"/><circle cx="17.200" cy="33.500" r="9"/><circle cx="13" cy="20.500" r="9"/><circle cx="24" cy="24" r="9"/>',
  teardrop: '<path d="M24 4c8 10 15 17 15 25a15 15 0 0 1-30 0C9 21 16 14 24 4z" stroke-width="3" stroke-linejoin="round"/>',
}
function BotFace({ shape, color, size }: { shape: string; color?: string | null; size: number }) {
  const fill = BOT_COLORS[color ?? ''] ?? BOT_COLORS.gray
  const body = BOT_SHAPES[shape] ?? BOT_SHAPES.blob
  const eyes = '<g fill="#fff" stroke="none" transform="rotate(-14 24 25)"><rect x="18.500" y="19" width="4" height="10" rx="2"/><rect x="26.500" y="19" width="4" height="10" rx="2"/></g>'
  return <svg className="avatar avatar--bot" width={size} height={size} viewBox="0 0 48 48" aria-hidden="true" style={{ fill, stroke: fill }} dangerouslySetInnerHTML={{ __html: body + eyes }} />
}

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
  if (look?.bot_shape && !look.emoji) return <BotFace shape={look.bot_shape} color={look.bot_color} size={size} />
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

/** One line to copy: a command, or a sentence to send. */
function Cmd({ text }: { text: string }) {
  const [copied, setCopied] = useState(false)
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      /* the text stays selectable */
    }
  }
  return (
    <div className="copy">
      <code className="copy__text">{text}</code>
      <button className="btn btn--small" onClick={copy}>{copied ? t('setup.copied') : t('setup.copy')}</button>
    </div>
  )
}

/** Something most people never need to read: closed until asked for. */
function More({ label, children }: { label: string; children: React.ReactNode }) {
  const [open, setOpen] = useState(false)
  return (
    <div className="more" data-open={open || undefined}>
      <button className="more__head" onClick={() => setOpen(!open)}>
        <svg width="10" height="10" viewBox="0 0 12 12" aria-hidden="true"><path d="M4.5 3 7.5 6 4.5 9" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" /></svg>
        {label}
      </button>
      {open && <div className="more__body">{children}</div>}
    </div>
  )
}

/** One agent's setup. Ready: a single line that opens on a click. Not ready: open, with what is
 *  left. `tone` none is an agent that is not on this machine at all. */
function SetupCard({ harness, name, state, tone, children }: { harness?: string; name: string; state: string; tone: 'ok' | 'todo' | 'none'; children?: React.ReactNode }) {
  const [open, setOpen] = useState<boolean | null>(null)
  const shown = open ?? tone === 'todo'
  return (
    <div className="card setup" data-open={shown || undefined}>
      <button className="setup__head" onClick={() => setOpen(!shown)}>
        {harness && <HarnessMark harness={harness} size={18} />}
        <span className="setup__name">{name}</span>
        <span className="setup__state" data-tone={tone}>{state}</span>
        <svg className="setup__chev" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M4.5 3 7.5 6 4.5 9" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" /></svg>
      </button>
      {shown && children && <div className="setup__steps">{children}</div>}
    </div>
  )
}

/** One setup step. Done: a check and its title, nothing else. Not done: its number, what it is
 *  for and the one thing to press. A step Dispatch cannot check (`onTick`) is ticked by hand:
 *  its mark is the checkbox. */
function Step({ n, done, optional, title, note, sub, onTick, children }: { n: number; done: boolean; optional?: boolean; title: string; note?: string; sub?: string; onTick?: (done: boolean) => void; children?: React.ReactNode }) {
  const mark = <span className="step__mark" data-done={done || undefined} data-todo={(!done && !optional) || undefined}>{done ? '✓' : n}</span>
  return (
    <div className="step" data-done={done || undefined}>
      {onTick ? (
        <button className="step__tick" title={done ? t('setup.untick') : t('setup.tick')} onClick={() => onTick(!done)}>{mark}</button>
      ) : (
        mark
      )}
      <div className="step__body">
        <div className="step__title">
          {title}
          {optional && <span className="step__optional">{t('setup.optional')}</span>}
          {done && note && <span className="step__note">{note}</span>}
        </div>
        {!done && sub && <div className="step__sub">{sub}</div>}
        {!done && children}
        {!done && onTick && (
          <div className="step__act">
            <button className="btn btn--small" onClick={() => onTick(true)}>{t('setup.tick')}</button>
          </div>
        )}
      </div>
    </div>
  )
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

/** What sending to this session does right now, in plain words: how it gets the message and
 *  what, if anything, the user has to do. `ok` it gets it at once; `auto` Dispatch opens it;
 *  `wait` it gets it when its turn ends; `you` the user has to act; `off` it cannot get it. */
function plainReach(s: Session): { tone: 'ok' | 'auto' | 'wait' | 'you' | 'off'; title: string; detail: string; todo?: string } {
  const p = s.reach.plain
  if (p) {
    const tone = ({ now: 'ok', auto: 'auto', you: 'you', turn: 'wait', speak: 'you', machine: 'wait', off: 'off' } as const)[p.kind]
    return { tone, title: p.title, detail: p.detail, todo: p.todo ?? undefined }
  }
  if (s.reach.level === 'now') return { tone: 'ok', title: t('reach2.nowTitle'), detail: t('reach2.now') }
  if (s.reach.level === 'none') return { tone: 'off', title: t('reach2.offTitle'), detail: s.reach.text }
  if (s.harness === 'Codex') return { tone: 'auto', title: t('reach2.autoTitle'), detail: t('reach2.codex') }
  if (!s.running) return { tone: 'auto', title: t('reach2.autoTitle'), detail: s.desktop ? t('reach2.claudeDesktop') : t('reach2.claudeTerminal') }
  if (s.status === 'busy' || s.status === 'running') return { tone: 'wait', title: t('reach2.turnTitle'), detail: t('reach2.turn') }
  return { tone: 'you', title: t('reach2.speakTitle'), detail: t('reach2.speak'), todo: t('reach2.speakTodo') }
}

/** The sessions this one exchanged messages with: how many each way, and the latest. */
type Tie = { key: string; name: string; harness?: string | null; host?: string | null; got: number; sent: number; last: string; dispatch: string }
function tiesOf(page: SessionPageData): Tie[] {
  const m = new Map<string, Tie>()
  const add = (key: string, name: string, harness: string | null | undefined, host: string | null | undefined, dir: 'in' | 'out', at: string, dispatch: string) => {
    const t0 = m.get(key) ?? { key, name, harness, host, got: 0, sent: 0, last: '', dispatch }
    if (dir === 'in') t0.got += 1
    else t0.sent += 1
    if (at > t0.last) {
      t0.last = at
      t0.dispatch = dispatch
    }
    m.set(key, t0)
  }
  for (const d of page.dispatches) {
    if (d.direction === 'in') add(`${d.from_host_name ?? ''}|${d.from}`, d.from_name, d.from_harness, d.from_host_name, 'in', d.created, d.dispatch_id)
    else for (const r of d.recipients) add(`${r.to_host_name ?? ''}|${r.to}`, r.to_name, r.to_harness, r.to_host_name, 'out', d.created, d.dispatch_id)
  }
  return [...m.values()].sort((a, b) => b.got + b.sent - (a.got + a.sent))
}

/** This session in the middle, the ones it wrote with around it. An arrow points at whoever got
 *  messages (both ends when they wrote both ways), the line carries how many, and pointing at a
 *  line, a picture or a row in the list shows each way and when they last wrote. */
function TieMap({ s, ties, hot, onHot, onPick }: { s: Session; ties: Tie[]; hot: string | null; onHot: (key: string | null) => void; onPick: (t: Tie) => void }) {
  const shown = ties.slice(0, 6)
  const W = 300
  const H = 176
  const cx = W / 2
  const cy = H / 2
  const R0 = 21
  const R1 = 15
  // spread around the middle, never straight above or below it, where the lines would be stubs
  const n = Math.max(shown.length, 1)
  const start = n === 1 ? 0 : -Math.PI / 2 + Math.PI / n
  const spots = shown.map((_, i) => {
    const a = start + (i * 2 * Math.PI) / n
    return { x: cx + Math.cos(a) * 118, y: cy + Math.sin(a) * 66 }
  })
  const most = Math.max(1, ...shown.map((x) => x.got + x.sent))
  const tip = shown.findIndex((x) => x.key === hot)
  return (
    <div className="tiemap" style={{ width: W, height: H + 20 }} onMouseLeave={() => onHot(null)}>
      <svg width={W} height={H} aria-hidden="true">
        <defs>
          <marker id="tie-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerUnits="userSpaceOnUse" markerWidth="9" markerHeight="9" orient="auto-start-reverse">
            <path d="M0 0.5 7.5 4 0 7.5z" className="tiemap__head" />
          </marker>
          <marker id="tie-arrow-hot" viewBox="0 0 8 8" refX="7" refY="4" markerUnits="userSpaceOnUse" markerWidth="9" markerHeight="9" orient="auto-start-reverse">
            <path d="M0 0.5 7.5 4 0 7.5z" className="tiemap__head tiemap__head--hot" />
          </marker>
        </defs>
        {shown.map((x, i) => {
          const dx = spots[i].x - cx
          const dy = spots[i].y - cy
          const len = Math.hypot(dx, dy) || 1
          const ux = dx / len
          const uy = dy / len
          const on = x.key === hot
          const arrow = on ? 'url(#tie-arrow-hot)' : 'url(#tie-arrow)'
          return (
            <line
              key={x.key}
              x1={cx + ux * (R0 + 3)}
              y1={cy + uy * (R0 + 3)}
              x2={spots[i].x - ux * (R1 + 3)}
              y2={spots[i].y - uy * (R1 + 3)}
              className="tiemap__line"
              data-hot={on || undefined}
              strokeWidth={1.5 + (2.5 * (x.got + x.sent)) / most}
              markerEnd={x.sent > 0 ? arrow : undefined}
              markerStart={x.got > 0 ? arrow : undefined}
            />
          )
        })}
      </svg>
      {shown.map((x, i) => (
        <button
          key={`n${x.key}`}
          className="tiemap__count"
          data-hot={x.key === hot || undefined}
          style={{ left: (cx + spots[i].x) / 2, top: (cy + spots[i].y) / 2 }}
          onMouseEnter={() => onHot(x.key)}
          onClick={() => onPick(x)}
        >
          {x.got + x.sent}
        </button>
      ))}
      <span className="tiemap__node tiemap__node--me" style={{ left: cx, top: cy }}>
        <Avatar name={s.name} look={s.avatar} appName={s.app_name} size={R0 * 2} />
      </span>
      {shown.map((x, i) => (
        <button key={x.key} className="tiemap__node" data-hot={x.key === hot || undefined} style={{ left: spots[i].x, top: spots[i].y }} onMouseEnter={() => onHot(x.key)} onClick={() => onPick(x)}>
          <Avatar name={x.name} size={R1 * 2} />
        </button>
      ))}
      <div className="tiemap__caption" style={{ top: H }}>
        {tip >= 0 ? (
          <>
            <b>{withHost(plain(shown[tip].name), shown[tip].host)}</b>
            {' · '}
            {t('sp.tieWays', { sent: shown[tip].sent, got: shown[tip].got })}
            {' · '}
            {t('sp.tieLast', { at: shown[tip].last.slice(5) })}
          </>
        ) : (
          t('sp.tieHow')
        )}
      </div>
    </div>
  )
}

/** A day heading for the history: 今天, 昨天, else the date. */
function dayOf(at: string | null | undefined): string {
  if (!at) return ''
  const d = new Date(at)
  const today = new Date()
  const y = new Date(today.getTime() - 86400000)
  const same = (a: Date, b: Date) => a.toDateString() === b.toDateString()
  if (same(d, today)) return t('day.today')
  if (same(d, y)) return t('day.yesterday')
  return `${d.getMonth() + 1}-${String(d.getDate()).padStart(2, '0')}`
}

function hhmm(at: string | null | undefined): string {
  if (!at) return ''
  const d = new Date(at)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

function SessionPage({ id, groups, onFiled, onCompose, onFeedPick, onBack }: { id: string; groups: Group[]; onFiled: () => void; onCompose: (to: string) => void; onFeedPick: (dispatchId: string) => void; onBack: () => void }) {
  const lang = useLang()
  const [page, setPage] = useState<SessionPageData | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [editing, setEditing] = useState<EditTarget | null>(null)
  const [only, setOnly] = useState<'all' | 'you' | 'dispatch'>('all')
  const [openRound, setOpenRound] = useState<number | null>(null)
  const [hotTie, setHotTie] = useState<string | null>(null)
  const { open, notice, clear } = useOpen()
  const load = useCallback(() => api.sessionPage(id).then((p) => { setPage(p); setErr(null) }).catch((e) => setErr(String(e))), [id, lang])
  useEffect(() => {
    load()
    const h = setInterval(load, 5000)
    return () => clearInterval(h)
  }, [load])
  const s = page?.session
  const edit = () => s && setEditing({ address: addressOf(s), name: s.app_name ?? s.name, contact: s.provisional ? null : s.contact, about: s.about, avatar: s.avatar })
  const reach = s ? plainReach(s) : null
  const ties = page ? tiesOf(page) : []
  const asked = (page?.rounds ?? []).map((r, i) => ({ r, i })).filter((x) => x.r.user)
  const counts = { all: asked.length, you: asked.filter((x) => !x.r.user!.via_dispatch).length, dispatch: asked.filter((x) => x.r.user!.via_dispatch).length }
  const listed = asked.filter((x) => only === 'all' || (only === 'dispatch') === x.r.user!.via_dispatch).reverse()
  let lastDay = ''
  return (
    <>
      <header className="bar" data-tauri-drag-region>
        <button className="btn btn--ghost btn--small" onClick={onBack}>‹ {t('session.back')}</button>
        <span className="bar__title" data-tauri-drag-region>{s ? plain(s.name) : ''}</span>
        <span className="bar__spacer" />
        {s && <button className="btn btn--primary" onClick={() => onCompose(addressOf(s))}>{t('action.message')}</button>}
      </header>
      {err && <div className="empty">{err}</div>}
      {s && page && reach && (
        <div className="sp">
          <section className="sp__card sp__idcard">
            <div className="sp__id">
              <button className="sp__face" title={t('action.edit')} onClick={edit}>
                <Avatar name={s.name} look={s.avatar} appName={s.app_name} size={48} harness={s.harness} />
              </button>
              <div className="sp__who">
                <button className="sp__name" onClick={edit}>
                  {plain(s.name)}
                  <Icon name="edit" size={13} />
                </button>
                <button className="sp__about" data-empty={!s.about || undefined} onClick={edit}>{s.about || t('sp.addAbout')}</button>
                <div className="sp__chips">
                  <ProjectPicker s={s} groups={groups} onMoved={() => { load(); onFiled() }} />
                  <span className="sp__chip">{s.harness}{s.host ? ` · ${s.host.name}` : ` · ${t('side.local')}`}</span>
                  {s.cwd_short && <span className="sp__chip sp__chip--path" title={s.cwd ?? ''}>{s.cwd_short}</span>}
                </div>
                {!s.host && (
                  <div className="sp__share">
                    <span className="sp__sharelabel">{t('share.label')}</span>
                    <span className="seg seg--small">
                      {(['none', 'basic', 'asks', 'rounds'] as const).map((l) => (
                        <button key={l} data-active={(s.share ?? 'basic') === l || undefined} title={t(`share.tip.${l}` as const)} onClick={() => api.shareSet(s.id, l).then(load)}>
                          {t(`share.${l}` as const)}
                        </button>
                      ))}
                    </span>
                  </div>
                )}
              </div>
              <button className="btn btn--small btn--with-icon" onClick={() => open(addressOf(s))}>
                <Icon name="open" size={13} />
                {t('action.open')}
              </button>
            </div>
            <div className="sp__reach" data-tone={reach.tone}>
              <span className="sp__dot" />
              <div className="sp__reachtext">
                <div className="sp__label">{t('sp.ifSent')}</div>
                <div className="sp__state">{reach.title}</div>
                <p className="sp__detail">{reach.detail}</p>
                {reach.todo && <p className="sp__todo">{reach.todo}</p>}
              </div>
              <div className="sp__reachside">
                <span className="sp__seen">{presence(s)}{s.last_active ? ` · ${t('sp.lastActive')} ${localStamp(new Date(s.last_active).toISOString())}` : ''}</span>
                {(reach.tone === 'auto' || reach.tone === 'you') && (
                  <button className="btn btn--small btn--with-icon" onClick={() => open(addressOf(s))}>
                    <Icon name="open" size={13} />
                    {t('sp.wake')}
                  </button>
                )}
              </div>
            </div>
          </section>

          <section className="sp__card sp__ties">
            <div className="sp__label">{t('sp.ties')}</div>
            {ties.length === 0 ? (
              <p className="sp__detail">{t('sp.noTies')}</p>
            ) : (
              <div className="sp__tiesbody">
                <TieMap s={s} ties={ties} hot={hotTie} onHot={setHotTie} onPick={(x) => onFeedPick(x.dispatch)} />
                <div className="sp__tielist" onMouseLeave={() => setHotTie(null)}>
                  {ties.map((x) => (
                    <button key={x.key} className="sp__tie" data-hot={x.key === hotTie || undefined} onMouseEnter={() => setHotTie(x.key)} onClick={() => onFeedPick(x.dispatch)}>
                      <Avatar name={x.name} size={18} />
                      <span className="sp__tiename">{withHost(plain(x.name), x.host)}</span>
                      <span className="sp__tiecount">{x.sent > 0 && <span title={t('sp.sentTip')}>→ {x.sent}</span>}{x.got > 0 && <span title={t('sp.gotTip')}>← {x.got}</span>}</span>
                    </button>
                  ))}
                </div>
              </div>
            )}
          </section>

          <section className="sp__card sp__history">
            <div className="sp__histhead">
              <span className="sp__label">{t('sp.history')}</span>
              <span className="seg seg--small">
                {(['all', 'you', 'dispatch'] as const).map((k) => (
                  <button key={k} data-active={only === k || undefined} onClick={() => setOnly(k)}>
                    {t(`sp.only.${k}` as const)} <i>{counts[k]}</i>
                  </button>
                ))}
              </span>
            </div>
            {!page.transcript_available && <p className="sp__detail">{t('session.noTranscript')}</p>}
            {page.transcript_available && listed.length === 0 && <p className="sp__detail">{t('session.emptyTranscript')}</p>}
            <div className="sp__asks">
              {listed.map(({ r, i }) => {
                const u = r.user!
                const day = dayOf(u.at)
                const head = day !== lastDay ? day : null
                lastDay = day
                const isOpen = openRound === i
                return (
                  <div key={i}>
                    {head && <div className="sp__day">{head}</div>}
                    <div className="askblock" data-open={isOpen || undefined}>
                    <button className="ask" data-dispatch={u.via_dispatch || undefined} data-open={isOpen || undefined} onClick={() => setOpenRound(isOpen ? null : i)}>
                      <span className="ask__time">{hhmm(u.at)}</span>
                      <span className="ask__who">
                        {u.via_dispatch ? <Avatar name={u.from_agent ?? '?'} size={18} /> : <span className="ask__you">{t('sp.you')}</span>}
                      </span>
                      <span className="ask__body">
                        {u.via_dispatch && <span className="ask__from">{t('sp.viaDispatch', { from: u.from_agent ?? '?' })}</span>}
                        <span className="ask__text">{isOpen ? u.text.trim() : firstLine(u.text)}</span>
                      </span>
                      <svg className="ask__chev" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M4.5 3 7.5 6 4.5 9" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" /></svg>
                    </button>
                    {isOpen && (
                      <div className="ask ask--answer">
                        <span className="ask__time" />
                        <span className="ask__who">
                          <Avatar name={s.name} look={s.avatar} appName={s.app_name} size={18} />
                        </span>
                        <span className="ask__body">
                          <span className="ask__from ask__from--answer">{t('sp.answer')}</span>
                          {r.replies.length === 0 ? <span className="ask__text ask__text--faint">{t('round.pending')}</span> : <Clamp text={r.replies.map((x) => x.text).join('\n\n')} />}
                        </span>
                        <span />
                      </div>
                    )}
                    </div>
                  </div>
                )
              })}
            </div>
          </section>
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
  const [link, setLink] = useState('')
  const [shown, setShown] = useState<{ code: string; until: number } | null>(null)
  const [linkShown, setLinkShown] = useState<{ link: string; until: number } | null>(null)
  const [copied, setCopied] = useState(false)
  const [relay, setRelay] = useState<string | null>(null)
  const [now, setNow] = useState(Date.now())
  const [adding, setAdding] = useState<boolean | null>(null)
  const [where, setWhere] = useState<'near' | 'far'>('near')
  const [detail, setDetail] = useState<string | null>(null)
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
      setLink('')
      setAdding(false)
    })
  const left = shown ? Math.max(0, Math.round((shown.until - now) / 1000)) : 0
  const linkLeft = linkShown ? Math.max(0, Math.round((linkShown.until - now) / 1000)) : 0
  const addrOf = (s: { addrs: string[]; port: number }) => `${s.addrs.find((a) => !a.includes(':')) ?? s.addrs[0] ?? ''}:${s.port}`
  const peers = ov?.peers ?? []
  const seen = ov?.seen ?? []
  // nothing paired yet: the guide is the page
  const guide = adding ?? (ov !== null && peers.length === 0)
  return (
    <>
      <div className="section-head">
        <div className="section-head__title">{t('settings.peers')}</div>
        <div className="section-head__sub">{t('peers.intro')}</div>
      </div>
      <div className="card">
        <div className="host">
          <div className="host__who">
            {renaming !== null ? (
              <span className="inline">
                <input className="input" value={renaming} onChange={(e) => setRenaming(e.target.value)} autoFocus />
                <button className="btn btn--small" disabled={busy || !renaming.trim()} onClick={() => run(async () => { await api.peerSetName(renaming); setRenaming(null) })}>{t('peers.save')}</button>
                <button className="btn btn--ghost btn--small" onClick={() => setRenaming(null)}>{t('action.cancel')}</button>
              </span>
            ) : (
              <span className="host__name">{t('peers.me', { name: ov?.me.name ?? '' })}</span>
            )}
            <span className="host__found host__found--wrap">{ov?.me.listening ? t('peers.meSub') : t('peers.notListening')}</span>
          </div>
          {renaming === null && <button className="btn btn--ghost btn--small" disabled={busy} onClick={() => setRenaming(ov?.me.name ?? '')}>{t('peers.rename')}</button>}
        </div>
        {peers.some((p) => p.blocked) && (
          <div className="host host--code">
            <span className="host__found host__found--wrap">{t('peers.blocked')}</span>
            <button className="btn btn--small" onClick={() => run(api.openLocalNetworkSettings)}>{t('peers.openPrivacy')}</button>
          </div>
        )}
        {peers.map((p) => (
          <div key={p.id} className="peer" data-open={detail === p.id || undefined}>
            <button className="peer__head" onClick={() => setDetail(detail === p.id ? null : p.id)}>
              <span className="presence" data-on={p.online || undefined} />
              <span className="peer__name">{p.name}</span>
              <span className="peer__state">
                {p.online ? (p.link ? t(`peers.link.${p.link}` as const) : t('peers.online')) : t('peers.offline')}
                {!p.verified && ` · ${t('peers.needsConfirm')}`}
              </span>
              <svg className="setup__chev" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M4.5 3 7.5 6 4.5 9" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" /></svg>
            </button>
            {(detail === p.id || !p.verified) && (
              <div className="peer__body">
                {!p.online && p.problem && <div className="peer__line">{p.problem}</div>}
                {p.superseded && <div className="peer__line">{t('peers.superseded')}</div>}
                <div className="peer__line">
                  <span className="fp">{t('peers.fingerprint', { fp: p.fingerprint })}</span>
                  {' · '}
                  {p.verified ? t('peers.confirmed') : t('peers.unconfirmed')}
                  {!p.verified && <button className="btn btn--primary btn--small" disabled={busy} onClick={() => run(async () => { const r = await api.peerVerify(p.id); setMsg(t('peers.pairedWith', { name: r.name })) })}>{t('peers.confirm')}</button>}
                </div>
                <div className="peer__line">
                  <span>{t('peers.rule')}</span>
                  <span className="seg seg--small">
                    {(['all', 'contacts', 'ask'] as const).map((r) => (
                      <button key={r} data-active={p.rule === r || undefined} onClick={() => run(() => api.peerRule(p.id, r))}>{t(`peers.rule.${r}` as const)}</button>
                    ))}
                  </span>
                </div>
                <div className="peer__line peer__line--faint">
                  <span>{p.address ?? (p.seen ? addrOf(p.seen) : t('peers.pairedAt', { t: p.paired_at }))}</span>
                  <button className="btn btn--ghost btn--small btn--danger" disabled={busy} onClick={() => run(() => api.peerRemove(p.id))}>{t('peers.remove')}</button>
                </div>
              </div>
            )}
          </div>
        ))}
        {!guide && (
          <div className="host">
            <div className="host__who">
              <span className="host__found host__found--wrap">{t('peers.many')}</span>
            </div>
            <button className="btn btn--primary btn--small" onClick={() => setAdding(true)}>{t('peers.add')}</button>
          </div>
        )}
      </div>

      {guide && (
        <div className="card setup" data-open>
          <div className="setup__head setup__head--plain">
            <span className="setup__name">{t('peers.add')}</span>
            <span className="seg seg--small setup__where">
              <button data-active={where === 'near' || undefined} onClick={() => setWhere('near')}>{t('peers.near')}</button>
              <button data-active={where === 'far' || undefined} onClick={() => setWhere('far')}>{t('peers.far')}</button>
            </span>
            {peers.length > 0 && <button className="btn btn--ghost btn--small" onClick={() => setAdding(false)}>{t('action.cancel')}</button>}
          </div>
          <div className="setup__steps">
            {where === 'near' ? (
              <>
                <Step n={1} done={false} title={t('peers.near1')} sub={t('peers.near1Sub')} />
                <Step n={2} done={false} title={t('peers.near2')} sub={seen.length === 0 ? t('peers.noneSeen') : undefined}>
                  {seen.map((s) => (
                    <div key={s.id} className="found">
                      <span className="found__name">{s.name || s.id.slice(0, 8)}</span>
                      {pairing === s.id ? (
                        <span className="inline">
                          <input className="input input--code" placeholder={t('peers.codePlaceholder')} value={code} onChange={(e) => setCode(e.target.value)} autoFocus />
                          <button className="btn btn--primary btn--small" disabled={busy || code.replace(/\s/g, '').length !== 6} onClick={() => pair(addrOf(s), code.replace(/\s/g, ''))}>{t('peers.pair')}</button>
                          <button className="btn btn--ghost btn--small" onClick={() => setPairing(null)}>{t('action.cancel')}</button>
                        </span>
                      ) : (
                        <button className="btn btn--primary btn--small" disabled={busy} onClick={() => { setPairing(s.id); setCode('') }}>{t('peers.pair')}</button>
                      )}
                    </div>
                  ))}
                  {seen.length === 0 && (
                    <div className="step__act">
                      <button className="btn btn--small" disabled={busy} onClick={() => run(api.peerScan)}>{t('peers.scan')}</button>
                    </div>
                  )}
                </Step>
                <Step n={3} done={false} title={t('peers.near3')} sub={t('peers.near3Sub')} />
                <div className="step step--aside">
                  <div className="step__body">
                    <div className="step__sub">{t('peers.otherWay')}</div>
                    {shown && left > 0 ? (
                      <div className="showcode">
                        <span className="code">{shown.code}</span>
                        <span className="step__sub">{t('peers.codeShown', { s: left })}</span>
                      </div>
                    ) : (
                      <div className="step__act">
                        <button className="btn btn--small" disabled={busy || !ov?.me.listening} onClick={showCode}>{t('peers.showCode')}</button>
                      </div>
                    )}
                  </div>
                </div>
              </>
            ) : (
              <>
                <Step n={1} done={!!ov?.internet} title={t('peers.far1')} sub={t('peers.far1Sub')}>
                  <div className="step__act">
                    <button className="btn btn--primary btn--small" disabled={busy} onClick={() => run(() => api.setPeerInternet(true))}>{t('peers.far1On')}</button>
                  </div>
                </Step>
                <Step n={2} done={false} title={t('peers.far2')} sub={t('peers.far2Sub')}>
                  {linkShown && linkLeft > 0 ? (
                    <>
                      <div className="copy">
                        <code className="copy__text">{linkShown.link}</code>
                        <button className="btn btn--primary btn--small" onClick={copyLink}>{copied ? t('peers.copied') : t('peers.copy')}</button>
                      </div>
                      <div className="step__sub">{t('peers.linkShown', { s: linkLeft })}</div>
                    </>
                  ) : (
                    <div className="step__act">
                      <button className="btn btn--small" disabled={busy || !ov?.internet || !ov?.me.internet_listening} onClick={makeLink}>{t('peers.makeLink')}</button>
                      {ov?.internet && !ov?.me.internet_listening && <span className="step__sub">{t('peers.restartFirst')}</span>}
                    </div>
                  )}
                </Step>
                <Step n={3} done={false} title={t('peers.far3')} sub={t('peers.far3Sub')}>
                  <div className="found">
                    <input className="input input--wide" placeholder={t('peers.linkPlaceholder')} value={link} onChange={(e) => setLink(e.target.value)} />
                    <button className="btn btn--small" disabled={busy || !link.trim().startsWith('dispatch-pair:')} onClick={() => pair(link.trim(), null)}>{t('peers.pair')}</button>
                  </div>
                </Step>
              </>
            )}
          </div>
        </div>
      )}

      <More label={t('peers.advanced')}>
        <div className="card">
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
              <span className="host__name">{t('peers.manual')}</span>
              <span className="host__found host__found--wrap">{t('peers.manualSub')}</span>
            </div>
            <span className="inline">
              <input className="input" placeholder={t('peers.manualTarget')} value={manual} onChange={(e) => setManual(e.target.value)} />
              <input className="input input--code" placeholder={t('peers.codePlaceholder')} value={manualCode} onChange={(e) => setManualCode(e.target.value)} />
              <button className="btn btn--small" disabled={busy || !manual.trim() || manualCode.replace(/\s/g, '').length !== 6} onClick={() => pair(manual.trim(), manualCode.replace(/\s/g, ''))}>{t('peers.pair')}</button>
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
          {ov?.me.listening && (
            <div className="host">
              <div className="host__who">
                <span className="host__found host__found--wrap">{t('peers.listening', { p: ov.me.listening })}</span>
              </div>
            </div>
          )}
        </div>
      </More>
      {msg && <div className="notice" onClick={() => setMsg(null)}>{msg}</div>}
    </>
  )
}

type SettingsTab = 'agents' | 'peers' | 'general'

function Settings() {
  const lang = useLang()
  const [tab, setTab] = useState<SettingsTab>('agents')
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
  const run = async (what: string, f: () => Promise<string | void>, say?: string) => {
    setBusy(what)
    try {
      const r = await f()
      setMsg(say ?? (typeof r === 'string' ? r : null))
      await load()
    } catch (e) {
      setMsg(String(e))
    } finally {
      setBusy(null)
    }
  }
  const check = () => run('check', async () => { setUpd(await api.updateCheck()) })
  const ticked = (key: string) => !!st?.setup_done?.includes(key)
  const tick = (key: string) => (done: boolean) => run(key, async () => { await api.setupMark(key, done) })
  const plugin = (harness: 'claude' | 'codex') => run(`plugin-${harness}`, async () => { await api.installPlugin(harness) }, t(harness === 'claude' ? 'setup.claudePluginDone' : 'setup.codexPluginDone'))
  const left = (n: number) => t('setup.left', { n })
  // Claude Code
  const claudeLeft = [st?.hook_installed, st?.claude_plugin].filter((x) => !x).length
  // Codex
  const codexLeft = [st?.codex_mcp].filter((x) => !x).length
  // Grok Bot: receiving needs the app, its sign-in and gbot; sending needs two things only the user can see
  const grokIn = !!st?.grok_app && !!st?.grok_signed_in && !!st?.gbot
  const grokOutLeft = [ticked('grok_computer'), ticked('grok_skill')].filter((x) => !x).length
  const grokLeft = [st?.grok_signed_in, !!st?.gbot].filter((x) => !x).length + grokOutLeft
  return (
    <>
      <header className="bar bar--page" data-tauri-drag-region>
        <span className="bar__title" data-tauri-drag-region>{t('nav.settings')}</span>
        <span className="seg seg--small bar__tabs">
          {(['agents', 'peers', 'general'] as const).map((k) => (
            <button key={k} data-active={tab === k || undefined} onClick={() => { setTab(k); setMsg(null) }}>{t(`settings.tab.${k}` as const)}</button>
          ))}
        </span>
      </header>
      <div className="settings">
        {tab === 'agents' && st && (
          <>
            <div className="section-head">
              <div className="section-head__title">{t('settings.agents')}</div>
              <div className="section-head__sub">{t('setup.sub')}</div>
            </div>
            {!st.binary && (
              <div className="card">
                <div className="host host--code">
                  <div className="host__who">
                    <span className="host__name">{t('setup.first')}</span>
                    <span className="host__found host__found--wrap">{st.bundled_cli ? t('setup.cliWhat') : t('settings.cliNone')}</span>
                  </div>
                  <button className="btn btn--primary btn--small" disabled={busy !== null || !st.bundled_cli} onClick={() => run('install', api.installCli)}>{busy === 'install' ? t('setup.installing') : t('settings.install')}</button>
                </div>
              </div>
            )}

            <SetupCard harness="Claude" name="Claude Code" tone={claudeLeft === 0 ? 'ok' : 'todo'} state={claudeLeft === 0 ? t('setup.ready') : left(claudeLeft)}>
              <Step n={1} done={st.hook_installed} title={t('setup.claudeHook')} sub={t('setup.claudeHookSub')}>
                <div className="step__act">
                  <button className="btn btn--primary btn--small" disabled={busy !== null || !st.bundled_cli} onClick={() => run('hook', st.binary ? api.installHook : api.installCli)}>{t('settings.install')}</button>
                </div>
              </Step>
              <Step n={2} done={st.claude_plugin} title={t('setup.claudePlugin')} sub={t('setup.claudePluginSub')}>
                {st.claude_cli ? (
                  <>
                    <div className="step__act">
                      <button className="btn btn--primary btn--small" disabled={busy !== null} onClick={() => plugin('claude')}>{busy === 'plugin-claude' ? t('setup.installing') : t('setup.installPlugin')}</button>
                    </div>
                    <More label={t('setup.byHand')}>
                      <div className="step__sub">{t('setup.claudePluginWhere')}</div>
                      <Cmd text="/plugin marketplace add IvyYang1999/dispatch" />
                      <Cmd text="/plugin install dispatch@dispatch" />
                    </More>
                  </>
                ) : (
                  <>
                    <div className="step__sub">{t('setup.claudePluginWhere')}</div>
                    <Cmd text="/plugin marketplace add IvyYang1999/dispatch" />
                    <Cmd text="/plugin install dispatch@dispatch" />
                  </>
                )}
              </Step>
              <div className="step step--pref">
                <div className="step__body">
                  <div className="step__title">
                    {t('settings.inbound')}
                    <span className="step__optional">{t('setup.optional')}</span>
                  </div>
                  <div className="step__sub">{st.inbound && st.inbound !== 'accept' ? `${t('settings.inboundOther', { v: st.inbound })} ` : ''}{t('settings.inboundShort')}</div>
                  <More label={t('setup.details')}>
                    <div className="step__sub">{t('settings.inboundSub')}</div>
                  </More>
                </div>
                <span className="seg seg--small">
                  <button data-active={st.inbound === 'accept' || undefined} onClick={() => run('inbound', async () => { await api.setInbound(true) })}>{t('settings.notifyOn')}</button>
                  <button data-active={st.inbound !== 'accept' || undefined} onClick={() => run('inbound', async () => { await api.setInbound(false) })}>{t('settings.notifyOff')}</button>
                </span>
              </div>
            </SetupCard>

            {st.codex ? (
              <SetupCard harness="Codex" name="Codex" tone={codexLeft === 0 ? 'ok' : 'todo'} state={codexLeft === 0 ? t('setup.ready') : left(codexLeft)}>
                <Step n={1} done={!!st.codex_mcp} title={t('setup.codexMcp')} sub={t('setup.codexMcpSub')}>
                  <div className="step__act">
                    <button className="btn btn--primary btn--small" disabled={busy !== null || !st.bundled_cli} onClick={() => run('install', api.installCli)}>{t('setup.register')}</button>
                  </div>
                </Step>
                <Step n={2} done={!!st.codex_plugin} optional title={t('setup.codexPlugin')} sub={t('setup.codexPluginSub')}>
                  <div className="step__act">
                    <button className="btn btn--small" disabled={busy !== null} onClick={() => plugin('codex')}>{busy === 'plugin-codex' ? t('setup.installing') : t('setup.installPlugin')}</button>
                  </div>
                  <More label={t('setup.byHand')}>
                    <div className="step__sub">{t('setup.inTerminal')}</div>
                    <Cmd text="codex plugin marketplace add IvyYang1999/dispatch" />
                    <Cmd text="codex plugin add dispatch@dispatch" />
                  </More>
                </Step>
              </SetupCard>
            ) : (
              <SetupCard harness="Codex" name="Codex" tone="none" state={t('setup.notHere')}>
                <div className="step"><div className="step__body"><div className="step__sub">{t('setup.codexNone')}</div></div></div>
              </SetupCard>
            )}

            {st.grok_app ? (
              <SetupCard harness="Grok Bot" name="Grok Bot" tone={grokLeft === 0 ? 'ok' : 'todo'} state={grokLeft === 0 ? t('setup.ready') : grokIn ? t('setup.grokHalf', { n: grokOutLeft }) : left(grokLeft)}>
                <div className="setup__part">{t('setup.grokIn')}{grokIn && !st.grok_error && <span className="step__note">{t('setup.grokBots', { n: st.grok_bots })}</span>}</div>
                <Step n={1} done={st.grok_signed_in} title={t('setup.grokApp')} sub={t('settings.grokSignIn')} />
                <Step n={2} done={!!st.gbot} title={t('setup.grokGbot')} sub={t('setup.inTerminal')}>
                  <Cmd text="npm i -g grok-bot-cli" />
                </Step>
                {grokIn && st.grok_error && <div className="step"><div className="step__body"><div className="step__sub step__sub--warn">{t('settings.grokOffline', { e: st.grok_error })}</div></div></div>}
                <div className="setup__part">{t('setup.grokOut')}</div>
                <Step n={3} done={ticked('grok_computer')} onTick={tick('grok_computer')} title={t('setup.grokComputer')} sub={t('setup.grokComputerSub')} />
                <Step n={4} done={ticked('grok_skill')} onTick={tick('grok_skill')} title={t('setup.grokSkill')} sub={t('setup.grokSkillSub')}>
                  <Cmd text={t('settings.grokCmd')} />
                </Step>
                <div className="step step--pref">
                  <div className="step__body">
                    <div className="step__title">{t('setup.grokSections')}</div>
                    <div className="step__sub">{t(`setup.grokSections.${st.grok_sections ?? 'start'}Sub` as const)}{(st.grok_sections ?? 'start') !== 'off' ? ` ${t('setup.grokSectionsFound', { n: st.grok_section_count ?? 0 })}` : ''}</div>
                  </div>
                  <span className="seg seg--small">
                    {(['start', 'follow', 'off'] as const).map((m) => (
                      <button key={m} data-active={(st.grok_sections ?? 'start') === m || undefined} onClick={() => run('grok-sections', async () => { await api.setGrokSections(m) })}>{t(`setup.grokSections.${m}` as const)}</button>
                    ))}
                  </span>
                </div>
              </SetupCard>
            ) : (
              <SetupCard harness="Grok Bot" name="Grok Bot" tone="none" state={t('setup.notHere')}>
                <div className="step"><div className="step__body"><div className="step__sub">{t('settings.grokMissing')}</div></div></div>
              </SetupCard>
            )}

            {st.binary && (
              <div className="repair">
                <span>{t('setup.cliOk', { p: st.binary })}</span>
                <button className="repair__btn" disabled={busy !== null || !st.bundled_cli} onClick={() => run('install', api.installCli)}>{busy === 'install' ? t('setup.installing') : t('setup.repair')}</button>
              </div>
            )}
          </>
        )}

        {tab === 'peers' && <Peers />}

        {tab === 'general' && (
          <div className="card">
            <div className="host">
              <div className="host__who">
                <span className="host__name">{t('settings.language')}</span>
                <span className="host__found">{t('settings.languageSub')}</span>
              </div>
              <span className="seg">
                {LANGS.map((l) => (
                  <button key={l.key} data-active={lang === l.key || undefined} onClick={() => setLang(l.key)}>{l.label}</button>
                ))}
              </span>
            </div>
            <div className="host">
              <div className="host__who">
                <span className="host__name">{t('settings.theme')}</span>
                <span className="host__found">{t('settings.themeSub')}</span>
              </div>
              <span className="seg">
                {(['system', 'light', 'dark'] as const).map((th) => (
                  <button key={th} data-active={(st?.theme ?? 'system') === th || undefined} onClick={() => run('theme', async () => { await api.setTheme(th) })}>{t(`theme.${th}` as const)}</button>
                ))}
              </span>
            </div>
            <div className="host">
              <div className="host__who">
                <span className="host__name">{t('settings.review')}</span>
                <span className="host__found host__found--wrap">{t('settings.reviewShort')}</span>
                <More label={t('setup.details')}>
                  <span className="host__found host__found--wrap">{t('settings.reviewSub')}</span>
                </More>
                {(st?.review_trusted ?? 0) > 0 && (
                  <span className="host__found host__found--wrap">
                    {t('settings.reviewTrusted', { n: st?.review_trusted ?? 0 })}{' '}
                    <button className="repair__btn" onClick={() => run('review', async () => { await api.reviewForget() })}>{t('settings.reviewForget')}</button>
                  </span>
                )}
              </div>
              <span className="seg">
                <button data-active={(st?.review ?? 'new') === 'new' || undefined} onClick={() => run('review', async () => { await api.setReview('new') })}>{t('settings.reviewNew')}</button>
                <button data-active={st?.review === 'all' || undefined} onClick={() => run('review', async () => { await api.setReview('all') })}>{t('settings.reviewAll')}</button>
                <button data-active={st?.review === 'off' || undefined} onClick={() => run('review', async () => { await api.setReview('off') })}>{t('settings.reviewOff')}</button>
              </span>
            </div>
            <div className="host">
              <div className="host__who">
                <span className="host__name">{t('settings.wake')}</span>
                <span className="host__found host__found--wrap">{t('settings.wakeShort')}</span>
                <More label={t('setup.details')}>
                  <span className="host__found host__found--wrap">{t('settings.wakeSub')}</span>
                </More>
              </div>
              <span className="seg">
                {(['auto', 'ask', 'off'] as const).map((m) => (
                  <button key={m} data-active={(st?.wake ?? 'auto') === m || undefined} onClick={() => run('wake', async () => { await api.setWake(m) })}>{t(m === 'auto' ? 'settings.wakeAuto' : m === 'ask' ? 'settings.wakeAsk' : 'settings.wakeOff')}</button>
                ))}
              </span>
            </div>
            <div className="host">
              <div className="host__who">
                <span className="host__name">{t('settings.notify')}</span>
                <span className="host__found host__found--wrap">{t('settings.notifySub')}</span>
              </div>
              <span className="seg">
                <button data-active={st?.notify !== false || undefined} onClick={() => run('notify', async () => { await api.setNotify(true) })}>{t('settings.notifyOn')}</button>
                <button data-active={st?.notify === false || undefined} onClick={() => run('notify', async () => { await api.setNotify(false) })}>{t('settings.notifyOff')}</button>
              </span>
            </div>
            <div className="host">
              <div className="host__who">
                <span className="host__name">{t('settings.storeWhere')}</span>
                <span className="host__found">{st?.store_root}</span>
              </div>
            </div>
            <div className="host">
              <div className="host__who">
                <span className="host__name">Dispatch {st?.version ?? ''}</span>
                <span className="host__found host__found--wrap">
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
        )}
        {msg && <div className="notice" onClick={() => setMsg(null)}>{msg}</div>}
      </div>
    </>
  )
}

// ---- Sidebar: sessions by project ----

const WEEK_MS = 7 * 24 * 3600 * 1000
const NOT_FILED = 'none'
/** A folder or a project the user made: something can be filed under it. The bots' own groups (Grok Bot's sections) and "not filed" cannot. */
const takesDrops = (id: string) => id !== NOT_FILED && !id.startsWith('harness:') && !id.startsWith('grok:')
const hostKey = (s: Session) => s.host?.id ?? ''

/** A set of ids kept in this window's storage (which groups are folded). It works without storage too. */
function useKeptSet(key: string): [Set<string>, (id: string) => void] {
  const [set, setSet] = useState<Set<string>>(() => {
    try {
      return new Set(JSON.parse(localStorage.getItem(key) ?? '[]') as string[])
    } catch {
      return new Set()
    }
  })
  const toggle = (id: string) =>
    setSet((old) => {
      const next = new Set(old)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      try {
        localStorage.setItem(key, JSON.stringify([...next]))
      } catch {
        /* folded for now only */
      }
      return next
    })
  return [set, toggle]
}

interface SideGroup { id: string; name: string; auto: boolean; rows: Session[]; hosts: Set<string>; latest: number }

/** Every project that has a session worth showing: in the order the user dragged them into,
 *  ones never placed (new ones) on top with the liveliest first, and "not filed" last. */
function groupsOf(sessions: Session[], order: string[]): SideGroup[] {
  const by = new Map<string, SideGroup>()
  for (const s of sessions) {
    const g = s.group ?? { id: NOT_FILED, name: '', auto: true }
    const have = by.get(g.id) ?? { ...g, rows: [], hosts: new Set<string>(), latest: 0 }
    have.rows.push(s)
    have.hosts.add(hostKey(s))
    have.latest = Math.max(have.latest, s.last_active ?? 0)
    by.set(g.id, have)
  }
  const rank = (s: Session) => (isContact(s) ? 0 : 1)
  for (const g of by.values()) g.rows.sort((a, b) => rank(a) - rank(b) || b.last_active - a.last_active)
  const at = (g: SideGroup) => order.indexOf(g.id)
  return [...by.values()].sort((a, b) => Number(a.id === NOT_FILED) - Number(b.id === NOT_FILED) || at(a) - at(b) || b.latest - a.latest)
}

type Held = { kind: 'session'; s: Session; from: string; label: string } | { kind: 'group'; from: string; label: string }

/** The sidebar's list: sessions under their projects. A session, or a whole project, is dragged
 *  onto another project to file it there; nothing is merged without the user doing it. */
function SideProjects({ sessions, suggestions, order, wrote, active, onOpen, reload }: { sessions: Session[]; suggestions: MergeSuggestion[]; order: string[]; wrote: Set<string>; active: string | null; onOpen: (address: string) => void; reload: () => Promise<void> }) {
  const [closed, toggleClosed] = useKeptSet('dispatch.side.closed')
  const [all, setAll] = useState<Set<string>>(new Set())
  const [menu, setMenu] = useState<string | null>(null)
  const [renaming, setRenaming] = useState<{ id: string; name: string } | null>(null)
  const [naming, setNaming] = useState<{ s: Session; name: string } | null>(null)
  const [note, setNote] = useState<{ text: string; act?: { label: string; run: () => Promise<unknown> } } | null>(null)
  const [drag, setDrag] = useState<{ label: string; kind: Held['kind']; x: number; y: number; over: string | null } | null>(null)
  const held = useRef<(Held & { x: number; y: number; on: boolean }) | null>(null)
  const justDragged = useRef(false)

  const now = Date.now()
  const shown = sessions.filter((s) => isContact(s) || s.running || now - s.last_active < WEEK_MS || wrote.has(s.id))
  const groups = groupsOf(shown, order)
  const nameOf = (id: string) => groups.find((g) => g.id === id)?.name ?? ''

  const act = async (f: () => Promise<unknown>) => {
    try {
      await f()
    } catch (e) {
      setNote({ text: String(e) })
    }
    await reload()
  }

  const drop = async (h: Held, over: string) => {
    if (h.kind === 'session') {
      if (over === 'new') {
        setNaming({ s: h.s, name: '' })
        return
      }
      if (over === h.from) return
      try {
        const r = await api.projectMove(hostKey(h.s), h.s.id, over)
        await reload()
        // only this one moved: the rest of its folder can follow, if the user says so
        if (r.behind > 0 && r.folder && h.from.startsWith('folder:')) {
          const folder = r.folder
          setNote({ text: t('proj.movedOne', { name: plain(h.s.name), n: r.behind }), act: { label: t('proj.moveFolder'), run: () => api.projectFolder(folder, r.group) } })
        } else {
          setNote(null)
        }
      } catch (e) {
        setNote({ text: String(e) })
      }
      return
    }
    if (over === 'new' || over === h.from) return
    // dropped between two projects: that is its place from now on
    if (over === 'end' || over.startsWith('before:')) {
      const ids = groups.map((g) => g.id).filter((id) => id !== h.from && id !== NOT_FILED)
      const i = over === 'end' ? -1 : ids.indexOf(over.slice('before:'.length))
      ids.splice(i < 0 ? ids.length : i, 0, h.from)
      await act(() => api.projectOrder(ids))
      return
    }
    const into = nameOf(over)
    await act(() => api.projectMerge(h.from, over))
    // a folder that was merged in can be taken out again; two made projects stay merged
    setNote({ text: t('proj.merged', { a: h.label, b: into }), act: h.from.startsWith('folder:') ? { label: t('proj.undo'), run: () => api.projectRelease(h.from.slice('folder:'.length)) } : undefined })
  }

  const hold = (e: React.PointerEvent, h: Held) => {
    if (e.button !== 0) return
    held.current = { ...h, x: e.clientX, y: e.clientY, on: false }
    const move = (ev: PointerEvent) => {
      const cur = held.current
      if (!cur) return
      if (!cur.on && Math.hypot(ev.clientX - cur.x, ev.clientY - cur.y) < 6) return
      cur.on = true
      const under = document.elementFromPoint(ev.clientX, ev.clientY)?.closest('[data-drop]')?.getAttribute('data-drop') ?? null
      const slot = !!under && (under === 'end' || under.startsWith('before:'))
      setDrag({ label: cur.label, kind: cur.kind, x: ev.clientX, y: ev.clientY, over: under && under !== cur.from && under !== `before:${cur.from}` && (cur.kind === 'group' ? under !== 'new' : !slot) ? under : null })
    }
    const up = (ev: PointerEvent) => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
      window.removeEventListener('pointercancel', up)
      const cur = held.current
      held.current = null
      setDrag(null)
      if (!cur?.on) return
      // the click that ends a drag must not open the row it started on
      justDragged.current = true
      setTimeout(() => (justDragged.current = false), 0)
      if (ev.type === 'pointercancel') return
      const under = document.elementFromPoint(ev.clientX, ev.clientY)?.closest('[data-drop]')?.getAttribute('data-drop')
      const slot = !!under && (under === 'end' || under.startsWith('before:'))
      if (under && (cur.kind === 'group' || !slot)) void drop(cur, under)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
    window.addEventListener('pointercancel', up)
  }

  const sug = suggestions[0]
  const places = (p: (string | null)[]) => [...new Set(p.map((x) => x ?? t('proj.here')))].join(t('proj.and'))
  return (
    <div className="side__sessions" data-dragging={drag ? drag.kind : undefined}>
      {sug && !note && (
        <div className="sidehint">
          <span>{t('proj.suggest', { name: sug.name, places: places(sug.places) })}</span>
          <span className="sidehint__acts">
            <button className="sidehint__yes" onClick={() => act(() => api.projectMerge(sug.from, sug.into))}>{t('proj.yes')}</button>
            <button onClick={() => act(() => api.projectDismiss(sug.key))}>{t('proj.no')}</button>
          </span>
        </div>
      )}
      {note && (
        <div className="sidehint">
          <span>{note.text}</span>
          <span className="sidehint__acts">
            {note.act && <button className="sidehint__yes" onClick={() => { const run = note.act!.run; setNote(null); void act(run) }}>{note.act.label}</button>}
            <button onClick={() => setNote(null)}>{t('proj.ok')}</button>
          </span>
        </div>
      )}
      <div className="projs">
        {groups.map((g) => {
          const open = !closed.has(g.id)
          const rows = all.has(g.id) ? g.rows : g.rows.slice(0, 5)
          const can = takesDrops(g.id)
          return (
            <div key={g.id} className="projwrap">
            <div className="projslot" data-drop={g.id === NOT_FILED ? undefined : `before:${g.id}`} data-over={(drag?.over === `before:${g.id}`) || undefined} />
            <div className="proj" data-drop={can ? g.id : undefined} data-over={(drag?.over === g.id) || undefined}>
              {renaming?.id === g.id ? (
                <form className="proj__form" onSubmit={(e) => { e.preventDefault(); const r = renaming; setRenaming(null); if (r.name.trim()) void act(() => api.projectRename(r.id, r.name)) }}>
                  <input className="input" autoFocus value={renaming.name} onChange={(e) => setRenaming({ id: g.id, name: e.target.value })} onBlur={() => setRenaming(null)} onKeyDown={(e) => e.key === 'Escape' && setRenaming(null)} />
                </form>
              ) : (
                <div className="proj__head" title={can ? t('proj.dragTip') : undefined} onPointerDown={can ? (e) => hold(e, { kind: 'group', from: g.id, label: g.name }) : undefined}>
                  <button className="proj__toggle" onClick={() => !justDragged.current && toggleClosed(g.id)}>
                    <svg className="proj__chev" data-open={open || undefined} width="10" height="10" viewBox="0 0 12 12" aria-hidden="true"><path d="M4.5 3 7.5 6 4.5 9" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" /></svg>
                    <span className="proj__name">{g.name || t('proj.none')}</span>
                  </button>
                  <span className="proj__count">{g.rows.length}</span>
                  {can && (
                    <button className="proj__more" title={t('proj.rename')} onPointerDown={(e) => e.stopPropagation()} onClick={() => setMenu(menu === g.id ? null : g.id)}>
                      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><circle cx="2.5" cy="6" r="1" fill="currentColor" /><circle cx="6" cy="6" r="1" fill="currentColor" /><circle cx="9.5" cy="6" r="1" fill="currentColor" /></svg>
                    </button>
                  )}
                </div>
              )}
              {menu === g.id && (
                <div className="proj__menu" onMouseLeave={() => setMenu(null)}>
                  <button onClick={() => { setMenu(null); setRenaming({ id: g.id, name: g.name }) }}>{t('proj.rename')}</button>
                  {!g.auto && <button title={t('proj.dissolveTip')} onClick={() => { setMenu(null); void act(() => api.projectDissolve(g.id)) }}>{t('proj.dissolve')}</button>}
                </div>
              )}
              {open && rows.map((s) => (
                <div key={`${hostKey(s)}|${s.id}`} className="agent agent--link" title={[s.host?.name, s.cwd].filter(Boolean).join(' · ')} data-active={active === addressOf(s) || undefined} onPointerDown={(e) => hold(e, { kind: 'session', s, from: g.id, label: plain(s.name) })} onClick={() => !justDragged.current && onOpen(addressOf(s))}>
                  <Avatar name={s.name} look={s.avatar} appName={s.app_name} size={20} harness={s.harness} />
                  <span className="agent__name">{plain(s.name)}</span>
                  {g.hosts.size > 1 && s.host && <span className="agent__host">{s.host.name}</span>}
                  <span className="presence" data-on={s.running || undefined} title={s.running ? t('presence.onTip') : t('presence.offlineTip')} />
                </div>
              ))}
              {open && g.rows.length > 5 && (
                <button className="proj__rest" onClick={() => setAll((old) => { const n = new Set(old); if (n.has(g.id)) n.delete(g.id); else n.add(g.id); return n })}>
                  {all.has(g.id) ? t('proj.less') : t('proj.rest', { n: g.rows.length - 5 })}
                </button>
              )}
            </div>
            </div>
          )
        })}
        <div className="projslot" data-drop="end" data-over={(drag?.over === 'end') || undefined} />
        {drag?.kind === 'session' && (
          <div className="proj proj--new" data-drop="new" data-over={(drag.over === 'new') || undefined}>＋ {t('proj.new')}</div>
        )}
        {naming && (
          <form className="proj__form" onSubmit={(e) => { e.preventDefault(); const n = naming; setNaming(null); if (n.name.trim()) void act(() => api.projectMove(hostKey(n.s), n.s.id, 'new', n.name)) }}>
            <input className="input" autoFocus placeholder={t('proj.newName')} value={naming.name} onChange={(e) => setNaming({ s: naming.s, name: e.target.value })} onKeyDown={(e) => e.key === 'Escape' && setNaming(null)} />
            <button className="btn btn--primary btn--small" disabled={!naming.name.trim()}>{t('proj.create')}</button>
          </form>
        )}
      </div>
      {drag && <div className="dragghost" style={{ left: drag.x + 12, top: drag.y + 10 }}>{drag.label || t('proj.none')}</div>}
    </div>
  )
}

/** The project tag on a session's page: click it to file the session somewhere else. */
function ProjectPicker({ s, groups, onMoved }: { s: Session; groups: Group[]; onMoved: () => void }) {
  const [open, setOpen] = useState(false)
  const [name, setName] = useState<string | null>(null)
  const now = s.group
  const move = async (target: string, called?: string) => {
    setOpen(false)
    setName(null)
    try {
      await api.projectMove(hostKey(s), s.id, target, called)
    } finally {
      onMoved()
    }
  }
  return (
    <span className="picker">
      <button className="sp__chip sp__chip--project" title={t('proj.pick')} onClick={() => setOpen(!open)}>
        {now?.name || t('proj.none')}
        <svg width="9" height="9" viewBox="0 0 12 12" aria-hidden="true"><path d="M3 4.5 6 7.5 9 4.5" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" /></svg>
      </button>
      {open && (
        <div className="picker__menu" onMouseLeave={() => name === null && setOpen(false)}>
          <div className="picker__label">{t('proj.pick')}</div>
          {groups.filter((g) => takesDrops(g.id)).map((g) => (
            <button key={g.id} data-active={g.id === now?.id || undefined} onClick={() => move(g.id)}>{g.name}</button>
          ))}
          {name === null ? (
            <button className="picker__new" onClick={() => setName('')}>＋ {t('proj.new')}</button>
          ) : (
            <form className="picker__form" onSubmit={(e) => { e.preventDefault(); if (name.trim()) void move('new', name) }}>
              <input className="input" autoFocus placeholder={t('proj.newName')} value={name} onChange={(e) => setName(e.target.value)} />
            </form>
          )}
        </div>
      )}
    </span>
  )
}

// ---- Shell ----

const NAV: { key: Screen; label: 'nav.feed' | 'nav.agents' | 'nav.compose' | 'nav.settings'; icon: 'feed' | 'agents' | 'compose' | 'settings' }[] = [
  { key: 'feed', label: 'nav.feed', icon: 'feed' },
  { key: 'agents', label: 'nav.agents', icon: 'agents' },
]

export function App() {
  const lang = useLang()
  const [screen, setScreen] = useState<Screen>('feed')
  const [composeTo, setComposeTo] = useState<string | undefined>(undefined)
  const [live, setLive] = useState<Session[]>([])
  const [suggestions, setSuggestions] = useState<MergeSuggestion[]>([])
  const [order, setOrder] = useState<string[]>([])
  const [wrote, setWrote] = useState<Set<string>>(new Set())
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
  // every session with its project, and who has written through Dispatch (those stay in the sidebar)
  const tick = useCallback(async () => {
    try {
      const [s, f] = await Promise.all([api.sessions(false), api.feed(100)])
      setLive(s.sessions)
      setSuggestions(s.suggestions ?? [])
      setOrder(s.project_order ?? [])
      setWrote(new Set(f.dispatches.flatMap((d) => [d.from, ...d.recipients.map((r) => r.to)])))
      // only what waits on the user: a message held for review, or one held for approval
      setQueued(f.dispatches.filter((d) => d.recipients.some((r) => r.kind === 'review' || r.kind === 'held')).length)
    } catch {
      /* the backend may not be up yet */
    }
  }, [])
  useEffect(() => {
    tick()
    const h = setInterval(tick, 12000)
    return () => clearInterval(h)
  }, [tick, lang])

  const compose = (to?: string) => {
    setComposeTo(to)
    setScreen('compose')
  }

  // every project there is, for the picker on a session's page
  const groups = [...new Map(live.filter((s) => s.group).map((s) => [s.group!.id, s.group!])).values()].sort((a, b) => a.name.localeCompare(b.name))

  return (
    <div className="shell">
      <nav className="side" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region>Dispatch</div>
        <div className="group" data-tauri-drag-region>
          {NAV.map((n) => (
            <button key={n.key} className="nav" data-active={screen === n.key || undefined} onClick={() => setScreen(n.key)}>
              <span className="nav__icon"><Icon name={n.icon} size={17} /></span>
              <span className="nav__label">{t(n.label)}</span>
              {n.key === 'feed' && queued > 0 && <span className="count" title={t('nav.waiting', { n: queued })}>{queued}</span>}
            </button>
          ))}
        </div>
        <SideProjects sessions={live} suggestions={suggestions} order={order} wrote={wrote} active={screen === 'session' ? sessionId : null} onOpen={openPage} reload={tick} />
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
        {screen === 'session' && sessionId && <SessionPage id={sessionId} groups={groups} onFiled={tick} onCompose={compose} onFeedPick={feedPick} onBack={() => setScreen('agents')} />}
        {screen === 'compose' && <Compose presetTo={composeTo} onSent={() => setScreen('feed')} />}
        {screen === 'settings' && <Settings />}
      </main>
    </div>
  )
}
