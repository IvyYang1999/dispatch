import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react'
import { Avatar, Bar, Button, Check, Collapse, Prompt, Roll, Stage, States, Stream, Window, type Act } from './kit'
import './demo.css'

// The page's demo: two sessions, the wire between them, and Dispatch on the wire. One session
// writes to the other; the message stops at Dispatch until the visitor (not a drawn cursor)
// chooses Send or Don't send in the review card; then it goes on, the other session answers,
// and the answer comes back into the first one's conversation.
//
// Four scenarios share the scene:
// - handoff: both sessions on this Mac.
// - machines: the recipient is on another Mac (the titles say so, the wire says "encrypted").
// - closed: the recipient is not open: a dashed slot where its window will be; after Send,
//   Dispatch opens it (the window arrives) and the message goes in.
// - unknown: the sender is a command line nobody is recognized behind: a terminal, a question
//   mark in the card, no reply to come back.
//
// The visitor's click is real. If nobody clicks for eight seconds the story goes on as Send, so
// the demo also plays by itself. It plays once; switching scenario or Replay remounts it.
//
// Real strings: what was asked, what the sender says, the message, the card's line about what
// will happen, the two buttons, the result, the reply. Everything else is bars.

export type ScenarioId = 'handoff' | 'machines' | 'closed' | 'unknown'
export type Lang = 'en' | 'zh'

type Cfg = { terminal: boolean; closed: boolean; machines: boolean; replies: boolean }
const CFG: Record<ScenarioId, Cfg> = {
  handoff: { terminal: false, closed: false, machines: false, replies: true },
  machines: { terminal: false, closed: false, machines: true, replies: true },
  closed: { terminal: false, closed: true, machines: false, replies: true },
  unknown: { terminal: true, closed: false, machines: false, replies: false },
}

type Status = 'none' | 'held' | 'sent' | 'replied' | 'declined'
type S = {
  /** Words the sending agent has said. */
  said: number
  tool: boolean
  status: Status
  /** The message on the wire: at the sender's end, at Dispatch, at the recipient's end. */
  pulseAt: 'a' | 'hub' | 'b'
  pulseOn: boolean
  card: 'none' | 'review' | 'sent' | 'declined'
  pressed: 'send' | 'decline' | null
  /** The recipient's window is open (always, except in `closed` before Dispatch opens it). */
  open: boolean
  got: boolean
  /** Words of the reply the recipient has written. */
  reply: number
  replyDone: boolean
  /** The reply is back in the sender's conversation. */
  back: boolean
}

const COPY = {
  en: {
    a: 'website',
    b: 'release',
    bot: 'deploy-bot',
    claude: 'Claude Code',
    codex: 'Codex',
    macA: 'MacBook Air',
    macB: 'Mac mini',
    ask: 'Ask the release session to ship 1.4.',
    say: "I'll write to release.",
    msg: 'Please ship 1.4 once CI is green, and reply here when it is out.',
    botMsg: 'Ship 1.4 now.',
    held: 'Waiting for you',
    sent: 'Sent',
    replied: 'Replied',
    declined: 'Not sent',
    from: 'From website, through Dispatch',
    fromBot: 'From a command line calling itself "deploy-bot"',
    reply: 'Shipped 1.4. Release notes are up.',
    replyFrom: 'release replied',
    unknownNote: 'Sender not recognized: asked every time',
    now: 'Gets it at once',
    nowOn: 'Gets it at once, on Mac mini',
    opens: 'Dispatch opens it for you',
    dont: "Don't send",
    send: 'Send',
    sentNow: 'release sees it right away',
    sentOn: 'On its way to Mac mini',
    sentOpen: 'Dispatch is opening release',
    notSent: 'Not sent',
    notSentSub: 'Nothing left this Mac',
    hint: 'Your call',
    closed: 'release is not open',
    encrypted: 'encrypted end to end',
    termHeld: 'Held until you choose Send in Dispatch',
    termSent: 'Delivered to release',
    termDeclined: 'Not sent: declined by the user',
  },
  zh: {
    a: 'website',
    b: 'release',
    bot: 'deploy-bot',
    claude: 'Claude Code',
    codex: 'Codex',
    macA: 'MacBook Air',
    macB: 'Mac mini',
    ask: '让 release 会话把 1.4 发出去。',
    say: '好，我给 release 写封信。',
    msg: '等 CI 通过后请发布 1.4，发完在这里回我一句。',
    botMsg: '现在就发 1.4。',
    held: '等你点头',
    sent: '已发出',
    replied: '已回信',
    declined: '没发',
    from: '来自 website，经 Dispatch 送达',
    fromBot: '来自一个自称「deploy-bot」的命令行',
    reply: '1.4 已发布，更新说明也放上去了。',
    replyFrom: 'release 回信了',
    unknownNote: '认不出发信方，每次都会问',
    now: '马上收到',
    nowOn: '马上收到，在 Mac mini 上',
    opens: 'Dispatch 会帮你打开它',
    dont: '不发',
    send: '发送',
    sentNow: 'release 马上就会看到',
    sentOn: '正在送往 Mac mini',
    sentOpen: 'Dispatch 正在打开 release',
    notSent: '没发',
    notSentSub: '什么都没离开这台 Mac',
    hint: '由你决定',
    closed: 'release 没开着',
    encrypted: '端到端加密',
    termHeld: '已扣下，等你在 Dispatch 里点「发送」',
    termSent: '已送到 release',
    termDeclined: '没发：用户没同意',
  },
} as const

const words = (text: string) => text.split(' ').length

function initialOf(cfg: Cfg): S {
  return { said: 0, tool: false, status: 'none', pulseAt: 'a', pulseOn: false, card: 'none', pressed: null, open: !cfg.closed, got: false, reply: 0, replyDone: false, back: false }
}

// The still most people would keep: the message went, the answer is back
function posterOf(cfg: Cfg, lang: Lang): S {
  const t = COPY[lang]
  return { said: words(t.say), tool: true, status: cfg.replies ? 'replied' : 'sent', pulseAt: 'a', pulseOn: false, card: 'sent', pressed: null, open: true, got: true, reply: words(t.reply), replyDone: true, back: cfg.replies }
}

type Pick = (a: Act<S>) => Promise<{ choice: 'send' | 'decline'; auto: boolean }>

function scriptOf(cfg: Cfg, lang: Lang, pick: Pick) {
  const t = COPY[lang]
  return async (a: Act<S>) => {
    await a.wait(800)
    if (!cfg.terminal) {
      for (let i = 1; i <= words(t.say); i++) {
        a.put({ said: i })
        await a.wait(80)
      }
      await a.wait(350)
    }
    // the sender calls Dispatch: the message sets off and stops at Dispatch
    a.mark('held')
    a.put({ tool: true, status: 'held' })
    await a.wait(500)
    a.put({ pulseOn: true })
    await a.wait(80)
    a.put({ pulseAt: 'hub' })
    await a.wait(620)
    a.put({ pulseOn: false, card: 'review' })
    a.mark('review')
    const { choice, auto } = await pick(a)
    if (auto) {
      a.put({ pressed: 'send' })
      await a.wait(200)
      a.put({ pressed: null })
    }
    if (choice === 'decline') {
      a.mark('declined')
      a.put({ card: 'declined', status: 'declined' })
      await a.wait(2600)
      return
    }
    a.mark('sent')
    a.put({ card: 'sent', status: 'sent' })
    await a.wait(450)
    if (cfg.closed) {
      a.put({ open: true })
      await a.wait(900)
    }
    a.put({ pulseOn: true })
    await a.wait(80)
    a.put({ pulseAt: 'b' })
    await a.wait(620)
    a.mark('got')
    a.put({ pulseOn: false, got: true })
    await a.wait(1300)
    if (!cfg.replies) {
      await a.wait(1400)
      return
    }
    for (let i = 1; i <= words(t.reply); i++) {
      a.put({ reply: i })
      await a.wait(90)
    }
    await a.wait(300)
    a.put({ replyDone: true })
    await a.wait(600)
    // the answer goes back the way the message came
    a.put({ pulseOn: true })
    await a.wait(80)
    a.put({ pulseAt: 'a' })
    await a.wait(900)
    a.mark('back')
    a.put({ pulseOn: false, back: true, status: 'replied' })
    await a.wait(2400)
  }
}

export function Demo({ scenario, lang, dark }: { scenario: ScenarioId; lang: Lang; dark: boolean }) {
  const cfg = CFG[scenario]
  const t = COPY[lang]
  // the visitor's answer, handed to the script while it waits at the review card
  const answer = useRef<((c: 'send' | 'decline') => void) | null>(null)
  const script = useMemo(() => {
    const pick: Pick = (a) =>
      new Promise((resolve, reject) => {
        let settled = false
        answer.current = (choice) => {
          if (settled) return
          settled = true
          resolve({ choice, auto: false })
        }
        a.wait(8000).then(() => {
          if (settled) return
          settled = true
          resolve({ choice: 'send', auto: true })
        }, reject)
      })
    return scriptOf(cfg, lang, pick)
  }, [cfg, lang])
  const initial = useMemo(() => initialOf(cfg), [cfg])
  const poster = useMemo(() => posterOf(cfg, lang), [cfg, lang])

  // The story waits until most of the stage is on screen (the kit would start it at the first
  // pixel), so the visitor is looking when the card asks. Until then: the opening frame.
  const wrap = useRef<HTMLDivElement>(null)
  const [seen, setSeen] = useState(false)
  useEffect(() => {
    const el = wrap.current
    if (!el || seen) return
    const io = new IntersectionObserver(([e]) => e.intersectionRatio >= 0.6 && setSeen(true), { threshold: [0.6] })
    io.observe(el)
    return () => io.disconnect()
  }, [seen])

  const act = (choice: 'send' | 'decline') => ({
    role: 'button',
    tabIndex: 0,
    onClick: () => answer.current?.(choice),
    onKeyDown: (e: KeyboardEvent) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault()
        answer.current?.(choice)
      }
    },
  })

  const titleA = cfg.terminal ? 'zsh' : `${t.a} — ${cfg.machines ? t.macA : t.claude}`
  const titleB = `${t.b} — ${cfg.machines ? t.macB : t.codex}`
  const senderName = cfg.terminal ? t.bot : t.a
  const message = cfg.terminal ? t.botMsg : t.msg
  const willTitle = cfg.closed ? t.opens : cfg.machines ? t.nowOn : t.now
  const sentSub = cfg.closed ? t.sentOpen : cfg.machines ? t.sentOn : t.sentNow

  return (
    <div ref={wrap}>
    <Stage width={1040} height={354} compact={{ below: 760, width: 358, height: 584 }} initial={initial} poster={seen ? poster : initial} script={seen ? script : undefined} play="once" theme={dark ? 'dark' : 'light'} className="dp">
      {(s) => {
        const statusText = s.status === 'declined' ? t.declined : s.status === 'replied' ? t.replied : s.status === 'sent' ? t.sent : t.held
        const termState = s.status === 'declined' ? 'declined' : s.status === 'held' || s.status === 'none' ? 'held' : 'sent'
        return (
          <>
            {/* the sender */}
            {cfg.terminal ? (
              <Window chrome="terminal" title={titleA} className="dp-win dp-win--a">
                {/* two short lines: a terminal is a character grid, and this window is 42 columns wide */}
                <Prompt path="~">{`dispatch send --from ${t.bot} \\`}</Prompt>
                <div className="dp-cont">{`    --to ${t.b} "${t.botMsg}"`}</div>
                <Collapse open={s.tool}>
                  <States
                    value={termState}
                    align="start"
                    states={{
                      held: <div>{t.termHeld}</div>,
                      sent: <div className="ok">{t.termSent}</div>,
                      declined: <div className="err">{t.termDeclined}</div>,
                    }}
                  />
                </Collapse>
              </Window>
            ) : (
              <Window title={titleA} className="dp-win dp-win--a">
                <div className="dp-body">
                  <div className="dp-you">{t.ask}</div>
                  <div className="dp-say">
                    <Stream text={t.say} count={s.said} />
                  </div>
                  <Collapse open={s.tool}>
                    <div className="dp-tool">
                      <Check on={s.status === 'sent' || s.status === 'replied'} pending={s.status === 'held'} size={14} />
                      <span className="dp-tool__name">dispatch_send</span>
                      <Roll className="dp-tool__state" align="end">
                        {statusText}
                      </Roll>
                    </div>
                  </Collapse>
                  <Collapse open={s.back} reveal="drop" from="below">
                    <div className="dp-in">
                      <span className="dp-in__from">{t.replyFrom}</span>
                      <span className="dp-in__text">{t.reply}</span>
                    </div>
                  </Collapse>
                </div>
              </Window>
            )}

            {/* the recipient: a dashed slot while it is not open, then its window */}
            {cfg.closed && (
              <div className="dp-slot" data-gone={s.open || undefined}>
                <span className="dp-slot__label">{t.closed}</span>
              </div>
            )}
            <Window title={titleB} className="dp-win dp-win--b" data-open={s.open || undefined}>
              <div className="dp-body">
                <div className="dp-filler">
                  <Bar w={180} h={7} r={3.5} tone={2} />
                  <Bar w={132} h={7} r={3.5} tone={2} />
                </div>
                <Collapse open={s.got} reveal="drop" from="below">
                  <div className="dp-in">
                    <span className="dp-in__from">{cfg.terminal ? t.fromBot : t.from}</span>
                    <span className="dp-in__text">{message}</span>
                  </div>
                </Collapse>
                {cfg.replies && (
                  <div className="dp-reply" data-on={s.reply > 0 || undefined}>
                    <Check on={s.replyDone} pending={s.reply > 0 && !s.replyDone} size={14} />
                    <Stream text={t.reply} count={s.reply} />
                  </div>
                )}
              </div>
            </Window>

            {/* the wire, Dispatch on it, and the message travelling */}
            <div className="dp-wire" aria-hidden="true" />
            {cfg.machines && <span className="dp-wire__label">{t.encrypted}</span>}
            <span className="dp-end dp-end--a" aria-hidden="true" />
            <span className="dp-end dp-end--b" data-off={!s.open || undefined} aria-hidden="true" />
            <span className="dp-pulse" data-at={s.pulseAt} data-on={s.pulseOn || undefined} aria-hidden="true" />
            <img className="dp-hub" src="./icon.png" alt="Dispatch" width={36} height={36} />

            {/* the review card: the visitor decides */}
            <div className="dp-card" data-state={s.card} data-unknown={cfg.terminal || undefined}>
              <div className="dp-card__review" aria-hidden={s.card !== 'review'}>
                <div className="dp-card__who">
                  {cfg.terminal ? <span className="dp-q">?</span> : <Avatar name={t.a} size={22} />}
                  <span className="dp-card__name">{senderName}</span>
                  <span className="dp-card__arrow">→</span>
                  <Avatar name={t.b} size={22} tone="accent" />
                  <span className="dp-card__name">{t.b}</span>
                </div>
                <div className="dp-card__msg">{message}</div>
                <div className="dp-card__will">
                  <span className="dp-card__willtitle">{willTitle}</span>
                  {cfg.terminal && <span className="dp-card__note">{t.unknownNote}</span>}
                </div>
                <div className="dp-card__acts">
                  <span className="dp-card__cue">{t.hint}</span>
                  <Button {...act('decline')} className="dp-btn" data-pressed={s.pressed === 'decline' || undefined} data-live={s.card === 'review' || undefined}>
                    {t.dont}
                  </Button>
                  <Button {...act('send')} variant="primary" className="dp-btn" data-pressed={s.pressed === 'send' || undefined} data-live={s.card === 'review' || undefined}>
                    {t.send}
                  </Button>
                </div>
              </div>
              <div className="dp-card__result" aria-hidden={s.card !== 'sent' && s.card !== 'declined'}>
                <span className="dp-tick" data-kind={s.card === 'declined' ? 'no' : 'yes'} data-on={s.card === 'sent' || s.card === 'declined' || undefined}>
                  <svg viewBox="0 0 28 28" width="28" height="28" aria-hidden="true">
                    <circle className="dp-tick__ring" cx="14" cy="14" r="12" />
                    <path className="dp-tick__yes" d="M8.5 14.5l3.6 3.6 7.4-8" />
                    <path className="dp-tick__no" d="M9 14h10" />
                  </svg>
                </span>
                <span className="dp-card__resulttext">
                  <Roll className="dp-card__resulttitle">{s.card === 'declined' ? t.notSent : t.sent}</Roll>
                  <span className="dp-card__resultsub">{s.card === 'declined' ? t.notSentSub : sentSub}</span>
                </span>
              </div>
            </div>
          </>
        )
      }}
    </Stage>
    </div>
  )
}
