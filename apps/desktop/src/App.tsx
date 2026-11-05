import { useCallback, useEffect, useMemo, useState } from 'react'
import { api, type Copy, type Dispatch, type Kind, type OpenResult, type Session, type SetupStatus } from './api'

type Screen = 'feed' | 'agents' | 'compose' | 'settings'

const KIND_TEXT: Record<Kind, string> = { queued: '排队中', delivered: '已送达', working: '处理中', replied: '已回复', failed: '没送到' }

function initial(name: string): string {
  const t = name.replace(/^[^\p{L}\p{N}]+/u, '').trim()
  return (t[0] ?? '?').toUpperCase()
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
      {text ?? KIND_TEXT[kind]}
    </span>
  )
}

function summary(rs: Copy[]): string {
  const replied = rs.filter((r) => r.kind === 'replied').length
  if (rs.length === 1) return rs[0].stage
  if (replied === rs.length) return '全部已回复'
  return `${replied}/${rs.length} 已回复`
}

function useOpen() {
  const [notice, setNotice] = useState<string | null>(null)
  const open = useCallback(async (id: string) => {
    try {
      const r: OpenResult = await api.openSession(id)
      if (r.kind === 'command') setNotice(`${r.note ?? ''}：${r.command}`)
      else setNotice(null)
    } catch (e) {
      setNotice(String(e))
    }
  }, [])
  return { open, notice, clear: () => setNotice(null) }
}

// ---- Feed ----

function Feed({ onCompose }: { onCompose: (to?: string) => void }) {
  const [items, setItems] = useState<Dispatch[]>([])
  const [selected, setSelected] = useState<string | null>(null)
  const [filter, setFilter] = useState<'all' | 'queued' | 'failed'>('all')
  const [err, setErr] = useState<string | null>(null)
  const { open, notice, clear } = useOpen()

  const load = useCallback(async () => {
    try {
      const r = await api.feed()
      setItems(r.dispatches)
      setErr(null)
    } catch (e) {
      setErr(String(e))
    }
  }, [])
  useEffect(() => {
    load()
    const t = setInterval(load, 5000)
    return () => clearInterval(t)
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

  return (
    <>
      <header className="bar" data-tauri-drag-region>
        <span className="bar__title" data-tauri-drag-region>派发</span>
        <span className="tabs">
          <button className="tab" data-active={filter === 'all' || undefined} onClick={() => setFilter('all')}>全部</button>
          <button className="tab" data-active={filter === 'queued' || undefined} onClick={() => setFilter('queued')}>排队中 <i>{counts.queued}</i></button>
          <button className="tab" data-active={filter === 'failed' || undefined} onClick={() => setFilter('failed')}>没送到 <i>{counts.failed}</i></button>
        </span>
        <span className="bar__spacer" />
        <button className="btn btn--primary" onClick={() => onCompose()}>发送</button>
      </header>
      <div className="split">
        <div className="list">
          {err && <div className="empty">{err}</div>}
          {!err && shown.length === 0 && <div className="empty">还没有派发。点右上角「发送」，或让任何一个 Agent 用 Dispatch 发一条。</div>}
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
                  <span className="detail__meta">{current.created} · 发给 {current.recipients.length} 个</span>
                </div>
              </div>
              <p className="detail__body">{current.text}</p>
              <div className="rule" />
              <div className="section-title">收件人</div>
              {current.recipients.map((r) => (
                <div key={r.letter_id} className="rcpt">
                  <div className="rcpt__head">
                    <Avatar name={r.to_name} size={20} />
                    <span className="rcpt__name">{r.to_name}</span>
                    <Badge kind={r.kind} />
                    <span className="rcpt__time">{(r.reply?.created ?? r.created).slice(11)}</span>
                    <button className="btn btn--ghost btn--icon" title="打开会话" onClick={() => open(r.to)}>↗</button>
                  </div>
                  {r.reply ? <p className="rcpt__reply">{r.reply.body}</p> : <p className="rcpt__note">{r.stage}</p>}
                </div>
              ))}
              {notice && (
                <div className="notice" onClick={clear}>
                  {notice}
                </div>
              )}
            </>
          ) : (
            <div className="empty">选一条派发看详情</div>
          )}
        </aside>
      </div>
    </>
  )
}

// ---- Agents ----

const PRESENCE: Record<string, string> = { idle: '空闲', busy: '工作中', running: '工作中', waiting: '等待输入' }

function Agents({ onCompose }: { onCompose: (to: string) => void }) {
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
  }, [showAll])
  useEffect(() => {
    load()
    const t = setInterval(load, 10000)
    return () => clearInterval(t)
  }, [load])
  const tools = useMemo(() => Array.from(new Set(sessions.map((s) => s.harness))), [sessions])
  const shown = sessions.filter((s) => tool === 'all' || s.harness === tool)
  return (
    <>
      <header className="bar" data-tauri-drag-region>
        <span className="bar__title" data-tauri-drag-region>名录</span>
        <span className="tabs">
          <button className="tab" data-active={tool === 'all' || undefined} onClick={() => setTool('all')}>全部 <i>{sessions.length}</i></button>
          {tools.map((t) => (
            <button key={t} className="tab" data-active={tool === t || undefined} onClick={() => setTool(t)}>{t} <i>{sessions.filter((s) => s.harness === t).length}</i></button>
          ))}
        </span>
        <span className="bar__spacer" />
        <label className="check"><input type="checkbox" checked={showAll} onChange={(e) => setShowAll(e.target.checked)} /> 含最近 48 小时没开着的</label>
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
              {s.running ? (PRESENCE[s.status ?? 'idle'] ?? s.status) : '离线'}
            </span>
            <span className="line__how" data-reach={s.reach.level}>{s.reach.text}</span>
            <span className="line__actions">
              <button className="btn btn--ghost btn--icon" title="发消息" onClick={() => onCompose(s.id)}>✎</button>
              <button className="btn btn--ghost btn--icon" title="打开会话" onClick={() => open(s.id)}>↗</button>
            </span>
          </div>
        ))}
        {shown.length === 0 && <div className="empty">没有找到会话。开着的 Claude Code 会话会自动出现在这里。</div>}
        {notice && <div className="notice" onClick={clear}>{notice}</div>}
      </div>
    </>
  )
}

// ---- Compose ----

function Compose({ presetTo, onSent }: { presetTo?: string; onSent: () => void }) {
  const [query, setQuery] = useState('')
  const [cands, setCands] = useState<Session[]>([])
  const [to, setTo] = useState<Session[]>([])
  const [text, setText] = useState('')
  const [urgency, setUrgency] = useState('普通')
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
    const h = setTimeout(() => api.find(query).then((r) => setCands(r.candidates.filter((c) => !to.some((t) => t.id === c.id)))), 150)
    return () => clearTimeout(h)
  }, [query, to])

  const submit = async () => {
    if (to.length === 0 || !text.trim()) return
    setBusy(true)
    try {
      const r = await api.send(to.map((t) => t.id), text, urgency)
      setResult(r.recipients.map((x) => `${x.to.name}：${x.detail}`).join('\n'))
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
        <span className="bar__title" data-tauri-drag-region>发送</span>
      </header>
      <div className="form">
        <label className="label">收件人</label>
        <div className="to">
          {to.map((t) => (
            <span key={t.id} className="to__chip">
              {t.name}
              <button onClick={() => setTo(to.filter((x) => x.id !== t.id))}>×</button>
            </span>
          ))}
          <input className="to__input" placeholder={to.length ? '再加一个…' : '名字、目录，或它正在做的事'} value={query} onChange={(e) => setQuery(e.target.value)} />
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
                    <span className="cand__presence">{c.running ? (PRESENCE[c.status ?? 'idle'] ?? c.status) : '离线'}</span>
                  </div>
                  <span className="cand__reason">{c.reason === 'name' ? '名字匹配' : c.reason === 'cwd' ? `目录 ${c.cwd_short}` : c.reason === 'id' ? 'id 匹配' : '正文提到'} · {c.reach.text}</span>
                </div>
              </div>
            ))}
            <div className="picker__hint">只有唯一命中才会发出；多个时由你选</div>
          </div>
        )}
        <label className="label">内容</label>
        <textarea className="body" rows={6} placeholder="像给同事写一段话：要什么、相关的 id 或路径、什么时候要。对方可以用 dispatch_reply 回你。" value={text} onChange={(e) => setText(e.target.value)} />
        <div className="form__row">
          <span className="label">紧急度</span>
          <span className="seg">
            {['普通', '紧急'].map((u) => (
              <button key={u} data-active={urgency === u || undefined} onClick={() => setUrgency(u)}>{u}</button>
            ))}
          </span>
        </div>
        <div className="form__actions">
          <button className="btn btn--primary" disabled={busy || to.length === 0 || !text.trim()} onClick={submit}>发送</button>
        </div>
        {result && <pre className="result">{result}</pre>}
      </div>
    </>
  )
}

// ---- Settings ----

function Settings() {
  const [st, setSt] = useState<SetupStatus | null>(null)
  const [msg, setMsg] = useState<string | null>(null)
  const load = useCallback(() => api.setupStatus().then(setSt).catch((e) => setMsg(String(e))), [])
  useEffect(() => {
    load()
  }, [load])
  const install = async () => {
    try {
      setMsg(await api.installHook())
      load()
    } catch (e) {
      setMsg(String(e))
    }
  }
  return (
    <>
      <header className="bar" data-tauri-drag-region>
        <span className="bar__title" data-tauri-drag-region>设置</span>
      </header>
      <div className="settings">
        <div className="section-head">
          <div className="section-head__title">这台 Mac 上的 Agent</div>
          <div className="section-head__sub">装上插件，它们就能互相派发消息，也知道怎么用。</div>
        </div>
        <div className="card">
          <div className="host">
            <div className="host__who">
              <span className="host__name">Claude Code</span>
              <span className="host__found">{st?.hook_installed ? 'hook 已装' : 'hook 没装：没开着或跳过权限确认的会话收不到消息'}</span>
            </div>
            {st?.hook_installed ? <Badge kind="replied" text={st.claude_plugin ? '插件和 hook 已装' : 'hook 已装'} /> : <button className="btn btn--primary btn--small" onClick={install}>安装 hook</button>}
          </div>
          <div className="host">
            <div className="host__who">
              <span className="host__name">Codex</span>
              <span className="host__found">{st?.codex ? `找到 ${st.codex}` : '本机没找到 codex 命令'}</span>
            </div>
            <code className="cmd">codex plugin marketplace add IvyYang1999/dispatch && codex plugin add dispatch</code>
          </div>
          <div className="host">
            <div className="host__who">
              <span className="host__name">Claude Code 插件</span>
              <span className="host__found">MCP 工具 + skill + hook，一步装进每个会话</span>
            </div>
            <code className="cmd">/plugin install dispatch --marketplace IvyYang1999/dispatch</code>
          </div>
        </div>
        <div className="section-head">
          <div className="section-head__title">消息</div>
        </div>
        <div className="card">
          <div className="host">
            <div className="host__who">
              <span className="host__name">消息存在哪</span>
              <span className="host__found">{st?.store_root}</span>
            </div>
          </div>
          <div className="host">
            <div className="host__who">
              <span className="host__name">命令行</span>
              <span className="host__found">{st?.binary ?? '还没运行 dispatch install'}</span>
            </div>
          </div>
        </div>
        {msg && <div className="notice" onClick={() => setMsg(null)}>{msg}</div>}
      </div>
    </>
  )
}

// ---- Shell ----

const NAV: { key: Screen; label: string; icon: string }[] = [
  { key: 'feed', label: '派发', icon: '▤' },
  { key: 'agents', label: '名录', icon: '◉' },
  { key: 'compose', label: '发送', icon: '✎' },
  { key: 'settings', label: '设置', icon: '⚙' },
]

export function App() {
  const [screen, setScreen] = useState<Screen>('feed')
  const [composeTo, setComposeTo] = useState<string | undefined>(undefined)
  const [live, setLive] = useState<Session[]>([])
  const [queued, setQueued] = useState(0)

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
    const t = setInterval(tick, 12000)
    return () => clearInterval(t)
  }, [])

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
              <span className="nav__label">{n.label}</span>
              {n.key === 'feed' && queued > 0 && <span className="count">{queued}</span>}
            </button>
          ))}
        </div>
        <div className="group">
          <div className="section">本机</div>
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
        {screen === 'feed' && <Feed onCompose={compose} />}
        {screen === 'agents' && <Agents onCompose={compose} />}
        {screen === 'compose' && <Compose presetTo={composeTo} onSent={() => setScreen('feed')} />}
        {screen === 'settings' && <Settings />}
      </main>
    </div>
  )
}
