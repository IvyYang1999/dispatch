import { useCallback, useEffect, useMemo, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { api, type Copy, type Dispatch, type Kind, type OpenResult, type Session, type SetupStatus, type UpdateCheck } from './api'
import { loadLang, setLang, t, useLang, type Lang } from './i18n'

type Screen = 'feed' | 'agents' | 'compose' | 'settings'

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

function presence(s: Session): string {
  if (!s.running) return t('presence.offline')
  const st = s.status ?? 'idle'
  if (st === 'idle' || st === 'busy' || st === 'running' || st === 'waiting') return t(`presence.${st}` as const)
  return st
}

function useOpen() {
  const [notice, setNotice] = useState<string | null>(null)
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
  return { open, notice, clear: () => setNotice(null) }
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
                  <span className="row__from">{d.from_name}</span>
                  <span className="row__text">{d.text}</span>
                  <span className="row__time">{d.created.slice(11)}</span>
                </div>
                <div className="row__rcpts">
                  {d.recipients.map((r) => (
                    <span key={r.letter_id} className="chip" data-kind={r.kind}>
                      <span className="chip__dot" />
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
                  <span className="detail__name">{current.from_name}</span>
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
                    <span className="rcpt__name">{r.to_name}</span>
                    <Badge kind={r.kind} />
                    <span className="rcpt__time">{(r.reply?.created ?? r.created).slice(11)}</span>
                    {(r.kind === 'queued' || r.kind === 'failed') && (
                      <span className="rcpt__acts">
                        <button className="btn btn--ghost btn--small" onClick={() => act(() => api.retry(r.letter_id))}>{t('action.retry')}</button>
                        <button className="btn btn--ghost btn--small" onClick={() => act(() => api.cancel(r.letter_id))}>{t('action.cancel')}</button>
                      </span>
                    )}
                    <button className="btn btn--ghost btn--icon" title={t('action.open')} onClick={() => open(r.to)}>↗</button>
                  </div>
                  {r.reply ? (
                    <>
                      <p className="rcpt__reply">{r.reply.body}</p>
                      <div className="rcpt__foot">
                        <button className="btn btn--ghost btn--small" onClick={() => onCompose(r.to)}>{t('action.reply')}</button>
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

function Agents({ onCompose }: { onCompose: (to: string) => void }) {
  const lang = useLang()
  const [sessions, setSessions] = useState<Session[]>([])
  const [tool, setTool] = useState<string>('all')
  const [showAll, setShowAll] = useState(false)
  const { open, notice, clear } = useOpen()
  const load = useCallback(async () => {
    try {
      const r = await api.sessions(!showAll)
      setSessions(r.sessions)
    } catch {
      /* keep the last list */
    }
  }, [showAll, lang])
  useEffect(() => {
    load()
    const h = setInterval(load, 10000)
    return () => clearInterval(h)
  }, [load])
  const tools = useMemo(() => Array.from(new Set(sessions.map((s) => s.harness))), [sessions])
  const shown = sessions.filter((s) => tool === 'all' || s.harness === tool)
  return (
    <>
      <header className="bar" data-tauri-drag-region>
        <span className="bar__title" data-tauri-drag-region>{t('nav.agents')}</span>
        <span className="tabs">
          <button className="tab" data-active={tool === 'all' || undefined} onClick={() => setTool('all')}>{t('tabs.all')} <i>{sessions.length}</i></button>
          {tools.map((h) => (
            <button key={h} className="tab" data-active={tool === h || undefined} onClick={() => setTool(h)}>{h} <i>{sessions.filter((s) => s.harness === h).length}</i></button>
          ))}
        </span>
        <span className="bar__spacer" />
        <label className="check"><input type="checkbox" checked={showAll} onChange={(e) => setShowAll(e.target.checked)} /> {t('agents.showAll')}</label>
      </header>
      <div className="lines">
        {shown.map((s) => (
          <div key={s.id} className="line">
            <Avatar name={s.name} />
            <span className="line__name" title={s.id}>{s.name}</span>
            <span className="tag">{s.harness}</span>
            <span className="line__where" title={s.cwd ?? ''}>{s.cwd_short ?? ''}</span>
            <span className="line__presence" data-presence={s.running ? (s.status ?? 'idle') : 'offline'}>
              <span className="line__dot" />
              {presence(s)}
            </span>
            <span className="line__how" data-reach={s.reach.level} title={s.reach.text}>{s.reach.text}</span>
            <span className="line__actions">
              <button className="btn btn--ghost btn--icon" title={t('action.message')} onClick={() => onCompose(s.id)}>✎</button>
              <button className="btn btn--ghost btn--icon" title={t('action.open')} onClick={() => open(s.id)}>↗</button>
            </span>
          </div>
        ))}
        {shown.length === 0 && <div className="empty">{t('agents.empty')}</div>}
        {notice && <div className="notice" onClick={clear}>{notice}</div>}
      </div>
    </>
  )
}

// ---- Compose ----

const URGENCIES = ['normal', 'urgent'] as const
const DEADLINES = ['default', '30m', '2h', '1d'] as const

function reasonText(c: Session): string {
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
      const hit = r.candidates.find((c) => c.id === presetTo) ?? r.candidates[0]
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
      const r = await api.send(to.map((x) => x.id), text, urgency, deadline === 'default' ? undefined : deadline)
      setResult(r.recipients.map((x) => `${x.to.name}: ${x.detail}`).join('\n'))
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
              {x.name}
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
                    <span className="cand__name">{c.name}</span>
                    <span className="tag">{c.harness}</span>
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

  useEffect(() => {
    loadLang()
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
          {live.slice(0, 8).map((s) => (
            <div key={s.id} className="agent" title={s.cwd ?? ''}>
              <Avatar name={s.name} size={16} />
              <span className="agent__name">{s.name}</span>
              <span className="presence" data-on={s.running || undefined} />
            </div>
          ))}
        </div>
      </nav>
      <main className="panel">
        {screen === 'feed' && <Feed onCompose={compose} pick={pick} />}
        {screen === 'agents' && <Agents onCompose={compose} />}
        {screen === 'compose' && <Compose presetTo={composeTo} onSent={() => setScreen('feed')} />}
        {screen === 'settings' && <Settings />}
      </main>
    </div>
  )
}
