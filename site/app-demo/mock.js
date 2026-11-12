// The stand-in for Dispatch's backend on the landing page. The page embeds the real app (the
// same frontend the desktop app ships) twice, the main window and the review window, and this
// file answers what they ask the backend for, from made-up data. Nothing here touches a network
// or a disk, and none of it is anyone's real session.
//
// Both frames share one store, kept on the page that embeds them (window.parent.__dispatchDemo),
// so approving a message in the review window shows in the main window. The page supplies
// showReview / hideReview / resizeReview there; opened on its own, this file makes do without.
;(() => {
  const qs = new URLSearchParams(location.search)
  const lang = qs.get('lang') === 'zh' ? 'zh' : 'en'
  const version = qs.get('v') || 'demo'
  const L = (en, zh) => (lang === 'zh' ? zh : en)

  let host = window
  try {
    if (window.parent && window.parent !== window && window.parent.__dispatchDemo) host = window.parent
  } catch {
    /* another origin: stay with this window */
  }
  const hub = host.__dispatchDemo || (host.__dispatchDemo = {})
  hub.frames = (hub.frames || []).filter((f) => {
    try {
      return f.win && !f.win.closed && f.win.document && f.win !== window
    } catch {
      return false
    }
  })
  const me = { win: window, handlers: [] }
  hub.frames.push(me)
  const emit = (event, payload) => {
    for (const f of hub.frames) {
      for (const h of f.handlers) {
        if (h.event !== event) continue
        try {
          f.win['_' + h.id]({ event, payload, id: h.id })
        } catch {
          /* a frame that went away */
        }
      }
    }
  }

  // ---- the made-up world ----

  const pad = (n) => String(n).padStart(2, '0')
  const stamp = (ms) => {
    const d = new Date(ms)
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
  }
  const MIN = 60000
  const HOUR = 60 * MIN

  const MINI = { id: 'mac-mini', name: 'Mac mini' }
  const PC = { id: 'studio-pc', name: 'Studio PC' }

  const plainNow = { kind: 'now', title: L('Gets it at once', '马上收到'), detail: L('It is open and sees the message as soon as it arrives.', '它开着，信一到就能看到。') }
  const plainCodex = {
    kind: 'auto',
    title: L('Dispatch opens it for you', 'Dispatch 会帮你打开它'),
    detail: L('If it is open in Codex, it gets the message within ten seconds; if not, Dispatch opens it in Codex and it gets it then. Nothing for you to do.', '它在 Codex 里开着的话，十秒内收到；没开着，Dispatch 会在 Codex 里把它打开，打开就收到。你不用做什么。'),
  }
  const plainClosed = {
    kind: 'auto',
    title: L('Dispatch opens it for you', 'Dispatch 会帮你打开它'),
    detail: L('It is not open. Dispatch starts it again in a Terminal window and it gets the message then. Nothing for you to do.', '它没开着。Dispatch 会开一个终端窗口接着跑它，跑起来就收到。你不用做什么。'),
  }
  const reachNow = { level: 'now', text: L('At once', '立刻送到'), fix: null, mode: 'auto', client: 'desktop', plain: plainNow }
  const reachCodex = { level: 'later', text: L('Into its queue in Codex', '进它在 Codex 里的队列'), fix: null, mode: 'on-request', client: 'desktop', plain: plainCodex }
  const reachClosed = { level: 'later', text: L('Delivered when it is opened', '打开它时送到'), fix: null, plain: plainClosed }

  function makeStore() {
    const now = Date.now()
    const session = (key, o) => ({
      key,
      share: 'basic',
      id: o.id,
      short_id: o.id.slice(0, 8),
      name: o.name,
      app_name: o.app ?? o.name,
      harness: o.harness,
      title: null,
      project: o.project,
      cwd: `/Users/you/code/${o.project}`,
      cwd_short: `~/code/${o.project}`,
      running: o.running !== false,
      status: o.running === false ? null : o.status ?? 'idle',
      desktop: o.desktop !== false,
      host_session_id: null,
      last_active: now - (o.ago ?? 5) * MIN,
      host: o.host ?? null,
      address: o.host ? `${o.host.name}/${o.name}` : o.name,
      contact: o.contact === false ? null : o.name,
      about: o.about ?? null,
      avatar: o.avatar ?? null,
      provisional: false,
      reach: o.reach ?? reachNow,
    })
    const sessions = [
      session('web', { id: '3f6c1a52-8d0e-4b7a-9c11-5e2a7d90b1c4', name: 'Web', app: 'Storefront web', harness: 'Claude', project: 'storefront', ago: 2, about: L('The storefront frontend', '商城前端'), avatar: { emoji: '🛍️', color: '#e8efff' } }),
      session('api', { id: '8b21d7e0-4c5f-4e1d-a3b6-0f9c2e7a6d18', name: 'API', app: 'Orders API', harness: 'Codex', project: 'storefront-api', ago: 4, about: L('Orders and payments', '订单和支付接口'), avatar: { emoji: '🧾', color: '#e6f6ec' }, reach: reachCodex }),
      session('scout', { id: 'scout', name: 'Scout', harness: 'Grok Bot', project: 'research', ago: 12, about: L('Looks things up and watches the news', '查资料、盯新闻'), avatar: { emoji: '🔭', color: '#fff4dc' } }),
      session('docs', { id: 'c47e9a13-2b6d-4f80-8e5a-7d1f3b0c9e26', name: 'Docs', app: 'Help center', harness: 'Claude', project: 'help-center', ago: 26 * 60, running: false, desktop: false, about: L('Writes the help center', '写帮助文档'), avatar: { emoji: '📚', color: '#f1f1ef' }, reach: reachClosed }),
      session('release', { id: '019b3c7d-55aa-7e02-b4f1-6a8d0c2e9f37', name: 'Release', app: 'Ship builds', harness: 'Codex', project: 'storefront', host: MINI, ago: 7, about: L('Signs and ships the builds', '签名和发版'), avatar: { emoji: '🚀', color: '#fdeaea' }, reach: reachCodex }),
      session('ios', { id: '5d0a8f31-9e4c-4a6b-b2d7-1c3e5f7a9b02', name: 'iOS app', harness: 'Claude', project: 'storefront-ios', host: MINI, ago: 1, status: 'busy', contact: false }),
      session('qa', { id: 'e2b4c6d8-1a3f-4c5e-9b7d-0f2a4c6e8b1d', name: 'QA', app: 'Windows tests', harness: 'Claude', project: 'storefront', host: PC, ago: 33, about: L('Runs the Windows test suite', '跑 Windows 上的测试'), avatar: { emoji: '🧪', color: '#eef2fd' } }),
    ]
    let n = 0
    const letter = (from, to, agoMin, kind, body) => ({ id: `demo-${++n}`.padEnd(12, '0'), dispatch: `d-${n}`, from, to, at: now - agoMin * MIN, kind, body })
    const letters = [
      letter('web', 'release', 26 * 60, 'replied', L('Please ship 1.4 once CI is green, and reply here when it is out.', '等 CI 通过后请发布 1.4，发完在这里回我一句。')),
      letter('release', 'web', 25 * 60 + 20, 'delivered', L('Shipped 1.4. Signed, notarized, and the release notes are up.', '1.4 已发布：签名、公证都做了，更新说明也放上去了。')),
      letter('web', 'release', 25 * 60, 'delivered', L('Thanks. The download button on the site now points at 1.4.', '谢谢。官网的下载按钮已经指向 1.4 了。')),
      letter('api', 'web', 5 * 60, 'replied', L('The orders endpoint now returns `shipping_eta`. Can you show it on the order page?', '订单接口现在会返回 `shipping_eta`，你能在订单页上显示出来吗？')),
      letter('web', 'api', 4 * 60 + 40, 'replied', L('Done, it shows under the address. One question: is the time UTC?', '加好了，显示在地址下面。有个问题：这个时间是 UTC 吗？')),
      letter('api', 'web', 4 * 60 + 35, 'delivered', L('Yes: UTC, ISO 8601.', '是的，UTC，ISO 8601 格式。')),
      letter('web', 'scout', 3 * 60, 'replied', L('What changed in the payment provider’s API this month that could affect checkout?', '支付服务商这个月的接口变动里，有哪些会影响结账？')),
      letter('scout', 'web', 2 * 60 + 50, 'delivered', L('Two things: card tokens now expire after 30 days, and the webhook signature header was renamed. Neither breaks you today; the first needs a change before March.', '两件事：卡片 token 现在 30 天过期；webhook 签名的请求头改了名字。今天都不会出问题，第一项要在三月前改。')),
      letter('ios', 'api', 95, 'replied', L('Which endpoint do I register push tokens with?', '推送 token 应该注册到哪个接口？')),
      letter('api', 'ios', 92, 'delivered', L('`POST /v2/devices` with `{token, platform}`. It is idempotent, so call it on every launch.', '`POST /v2/devices`，带 `{token, platform}`。它是幂等的，每次启动都调一次就行。')),
      letter('qa', 'web', 70, 'delivered', L('The Windows run is green again: 412 of 412. The flaky one was the date picker.', 'Windows 上的测试又全绿了：412 项全过。之前不稳定的是日期选择器那一项。')),
      letter('you', 'web', 40, 'replied', L('How is the checkout redesign going?', '结账页改版进展怎么样了？')),
      letter('web', 'you', 38, 'delivered', L('The new layout is in. Left to do: the address form on small screens. I asked API for the tax preview endpoint.', '新布局已经做好。还差小屏幕上的地址表单。我已经找 API 要了税费预览的接口。')),
    ]
    const held = letter('api', 'release', 2, 'review', L('Tag v2.0-rc1 from main and build the installers. Reply here when they are signed.', '请从 main 打 v2.0-rc1 的 tag 并构建安装包，签好名后在这里回我。'))
    letters.push(held)
    return { lang, now, sessions, letters, heldId: held.id, me: { name: null, avatar: null }, timers: [] }
  }

  if (!hub.store || hub.store.lang !== lang) hub.store = makeStore()
  const store = () => hub.store

  const sessionOf = (key) => store().sessions.find((s) => s.key === key)
  const publicSession = (s) => {
    const { key, ...rest } = s
    return rest
  }
  const partyOf = (key) => {
    if (key === 'you') {
      const m = store().me
      return { key: 'you', id: 'app', name: m.name || L('You', '你'), app_name: null, harness: L('Person', '人'), host_id: null, host_name: null, person: true, address: null, running: false, status: null, cwd_short: null, avatar: m.avatar, contact: null, about: null }
    }
    const s = sessionOf(key)
    return {
      key: `${s.host ? s.host.id : ''}|${s.id}`,
      id: s.id,
      name: s.name,
      app_name: s.app_name,
      harness: s.harness,
      host_id: s.host ? s.host.id : null,
      host_name: s.host ? s.host.name : null,
      person: false,
      address: s.host ? `${s.host.id}/${s.id}` : s.id,
      running: s.running,
      status: s.status,
      cwd_short: s.cwd_short,
      avatar: s.avatar,
      contact: s.contact,
      about: s.about,
    }
  }
  const STAGE = {
    replied: L('Replied', '已回复'),
    delivered: L('Delivered', '已送达'),
    working: L('Working on it', '处理中'),
    review: L('Waiting for your review', '等你审核'),
    cancelled: L('Not sent: you declined', '没发：你没同意'),
    queued: L('Waiting', '等待中'),
  }
  const code = (harness) => ({ Claude: 'claude', Codex: 'codex', 'Grok Bot': 'grok' })[harness] ?? 'human'
  const copyOf = (l) => {
    const from = partyOf(l.from)
    const to = partyOf(l.to)
    return {
      letter_id: l.id,
      short_id: l.id.slice(0, 8),
      from: from.id,
      from_name: from.name,
      from_harness: from.harness,
      from_host_name: from.host_name,
      to: to.id,
      to_name: to.name,
      to_harness: to.harness,
      to_host: to.host_id,
      to_host_name: to.host_name,
      status: STAGE[l.kind],
      stage: STAGE[l.kind],
      kind: l.kind,
      created: stamp(l.at),
      deadline: stamp(l.at + 24 * HOUR),
      path: '',
      body: l.body,
      review: l.kind === 'review' ? { state: 'shown', at: new Date(l.at).toISOString(), app_running: true, steps: [] } : undefined,
    }
  }
  const pairKey = (l) => [l.from, l.to].sort().join('~')
  function threads() {
    const groups = new Map()
    for (const l of store().letters) {
      const k = pairKey(l)
      if (!groups.has(k)) groups.set(k, [])
      groups.get(k).push(l)
    }
    const out = []
    for (const [k, list] of groups) {
      list.sort((x, y) => x.at - y.at)
      const [ka, kb] = k.split('~')
      const last = list[list.length - 1]
      const side = (l) => (l.from === ka ? 'a' : 'b')
      out.push({
        key: k,
        a: partyOf(ka),
        b: partyOf(kb),
        last_at: stamp(last.at),
        _at: last.at,
        count: list.length,
        attention: list.some((l) => l.kind === 'review') ? 'review' : null,
        last: { side: side(last), body: last.body, kind: last.kind, stage: STAGE[last.kind] },
        messages: list.map((l) => ({ ...copyOf(l), dispatch_id: l.dispatch, side: side(l), at: stamp(l.at) })),
      })
    }
    out.sort((x, y) => y._at - x._at)
    return { count: out.length, threads: out }
  }

  // What each session was asked by its own person, to sit beside what Dispatch brought it
  const OWN = {
    web: [
      [L('Make the cart badge update without a reload.', '让购物车角标不用刷新就能更新。'), L('Done: the badge now listens for cart events.', '改好了：角标现在会监听购物车事件。')],
      [L('Ask the release session to ship 1.4.', '让 release 会话把 1.4 发出去。'), L('I wrote to Release. It waits for your review, then goes to the Mac mini.', '我给 Release 写了信。它在等你审核，之后会送到 Mac mini。')],
    ],
    api: [[L('Add shipping_eta to the orders endpoint.', '给订单接口加上 shipping_eta。'), L('Added, with tests. I told Web so the order page can show it.', '加好了，测试也写了。我告诉了 Web，订单页可以显示它了。')]],
    release: [[L('Set up notarization for the new certificate.', '给新证书配好公证。'), L('Done. The next build uses it.', '配好了，下一次构建就会用它。')]],
    ios: [[L('Add push notifications for order updates.', '给订单状态变化加上推送通知。'), L('Working on it. I asked API where to register tokens.', '正在做。我问了 API 该把 token 注册到哪里。')]],
    qa: [[L('Run the Windows suite on every merge.', '每次合并后都跑一遍 Windows 测试。'), L('Scheduled. I report to Web when it changes color.', '已安排。结果变了我会告诉 Web。')]],
    docs: [[L('Rewrite the returns page in plain words.', '用大白话重写退货说明。'), L('Rewritten: half the length, same rules.', '重写好了：篇幅减半，规则没变。')]],
    scout: [[L('Watch the payment provider’s changelog.', '盯着支付服务商的更新日志。'), L('Watching. I write to Web when something affects checkout.', '在盯着。有影响结账的变动我会告诉 Web。')]],
  }
  function sessionPage(address) {
    const id = String(address).split('/').pop()
    const s = store().sessions.find((x) => x.id === id || x.name === id) ?? store().sessions[0]
    const mine = store().letters.filter((l) => l.from === s.key || l.to === s.key)
    const dispatches = mine
      .slice()
      .sort((x, y) => y.at - x.at)
      .map((l) => {
        const c = copyOf(l)
        return { dispatch_id: l.dispatch, from: c.from, from_name: c.from_name, from_harness: c.from_harness, from_host_name: c.from_host_name, created: c.created, urgency: 'normal', text: l.body, recipients: [c], direction: l.to === s.key ? 'in' : 'out' }
      })
    const turn = (role, text, at, via, from) => ({ role, text, at: new Date(at).toISOString(), via_dispatch: !!via, from_agent: from ?? null })
    const rounds = []
    const base = store().now - 30 * HOUR
    ;(OWN[s.key] ?? []).forEach(([q, a], i) => rounds.push({ _at: base + i * HOUR, user: turn('user', q, base + i * HOUR), replies: [turn('assistant', a, base + i * HOUR + MIN)] }))
    for (const l of mine.filter((x) => x.to === s.key && x.kind !== 'review' && x.kind !== 'cancelled')) {
      const from = partyOf(l.from)
      const answer = store().letters.find((r) => r.from === s.key && r.to === l.from && r.at > l.at)
      rounds.push({ _at: l.at, user: turn('user', l.body, l.at, true, from.host_name ? `${from.name} @ ${from.host_name}` : from.name), replies: answer ? [turn('assistant', answer.body, answer.at)] : [] })
    }
    rounds.sort((x, y) => x._at - y._at)
    return { session: publicSession(s), dispatches, rounds: rounds.map(({ _at, ...r }) => r), transcript_available: true }
  }

  const heldLetter = () => store().letters.find((l) => l.id === store().heldId && l.kind === 'review')
  function reviewInfo() {
    const l = heldLetter()
    if (!l) throw new Error(L('Nothing is waiting for review', '没有信在等审核'))
    const c = copyOf(l)
    const from = sessionOf(l.from)
    const to = sessionOf(l.to)
    return {
      letter_id: l.id,
      short_id: c.short_id,
      dispatch_id: l.dispatch,
      status: c.status,
      waiting: true,
      from: c.from,
      from_name: c.from_name,
      from_harness: code(from.harness),
      from_avatar: from.avatar,
      from_unknown: false,
      to: c.to,
      to_name: c.to_name,
      to_harness: code(to.harness),
      to_host: c.to_host,
      to_host_name: c.to_host_name,
      body: l.body,
      urgency: 'normal',
      created: c.created,
      deadline: c.deadline,
      session: publicSession(to),
      last_query: { role: 'user', text: L('Set up notarization for the new certificate.', '给新证书配好公证。'), at: new Date(store().now - 3 * HOUR).toISOString(), via_dispatch: false, from_agent: null },
      transcript_available: true,
      review: c.review,
      first_time: true,
      mode: 'new',
      queued: 1,
    }
  }
  const later = (ms, fn) => store().timers.push(setTimeout(fn, ms))
  function changed() {
    emit('dispatch:changed')
    emit('review:changed')
  }
  function decide(decision) {
    const l = heldLetter()
    if (!l) throw new Error(L('Already decided', '已经处理过了'))
    if (decision === 'decline') {
      l.kind = 'cancelled'
      changed()
      return { letter_id: l.id, decision: 'declined', text: STAGE.cancelled }
    }
    l.kind = 'delivered'
    l.at = Date.now()
    changed()
    // the other session gets to work, then answers
    later(2500, () => {
      l.kind = 'working'
      changed()
    })
    later(7000, () => {
      l.kind = 'replied'
      store().letters.push({ id: `demo-reply-${Date.now()}`, dispatch: l.dispatch + '-r', from: l.to, to: l.from, at: Date.now(), kind: 'delivered', body: L('Tagged v2.0-rc1. The installers are building; I will write again when they are signed.', 'v2.0-rc1 的 tag 打好了，安装包正在构建，签好名我再告诉你。') })
      changed()
    })
    return {
      letter_id: l.id,
      decision: decision === 'trust' ? 'trusted' : 'approved',
      route: 'inbox',
      detail: STAGE.delivered,
      text: STAGE.delivered,
      plain: L('Sent. If it is open in Codex it gets it within ten seconds; if not, Dispatch opens it in Codex for you.', '已送出。它在 Codex 里开着的话十秒内收到；没开着，Dispatch 会帮你在 Codex 里打开它。'),
    }
  }
  // A fresh message to review, for the page's "Review" button after the first one was decided
  hub.holdOne = () => {
    if (heldLetter()) return
    const st = store()
    const l = { id: `demo-held-${Date.now()}`, dispatch: `d-held-${Date.now()}`, from: 'api', to: 'release', at: Date.now(), kind: 'review', body: L('Please publish the v2.0-rc1 installers to the beta channel.', '请把 v2.0-rc1 的安装包发布到 beta 渠道。') }
    st.letters.push(l)
    st.heldId = l.id
    changed()
  }

  function send(args) {
    const st = store()
    const dispatch = `d-you-${Date.now()}`
    const recipients = []
    for (const who of args.to ?? []) {
      const id = String(who).split('/').pop()
      const s = st.sessions.find((x) => x.id === id || x.name.toLowerCase() === id.toLowerCase())
      if (!s) continue
      const l = { id: `demo-you-${Date.now()}-${s.key}`, dispatch, from: 'you', to: s.key, at: Date.now(), kind: 'delivered', body: args.text }
      st.letters.push(l)
      recipients.push({ to: { name: s.name, address: s.address }, route: 'inbox', detail: STAGE.delivered })
      later(5000, () => {
        l.kind = 'replied'
        st.letters.push({ id: `demo-ack-${Date.now()}-${s.key}`, dispatch: dispatch + '-r', from: s.key, to: 'you', at: Date.now(), kind: 'delivered', body: L('Got it. (This is a demo, so nobody is really here, but in the app the session answers you like this.)', '收到。（这里是演示，其实没有人在；在真正的 App 里，会话会像这样回你。）') })
        changed()
      })
    }
    changed()
    return { dispatch_id: dispatch, recipients }
  }

  const matches = (s, q) => {
    const hay = [s.name, s.app_name, s.about, s.project, s.harness, s.host ? s.host.name : L('this Mac', '本机')].join(' ').toLowerCase()
    return q
      .toLowerCase()
      .split(/\s+/)
      .filter(Boolean)
      .every((w) => hay.includes(w))
  }

  const settings = { notify: true, theme: 'system', inbound: 'accept', review: 'new', wake: 'auto', internet: true }
  const peers = () => ({
    me: { id: 'this-mac', name: 'MacBook Pro', public_key: 'demo', listening: 47831, iroh_id: 'demo', internet_listening: settings.internet },
    internet: settings.internet,
    relay: null,
    peers: [
      { id: MINI.id, name: MINI.name, address: null, paired_at: stamp(store().now - 20 * 24 * HOUR), online: true, seen: null, link: 'lan', fingerprint: 'amber lake 4821', verified: true, rule: 'all', internet: true },
      { id: PC.id, name: PC.name, address: null, paired_at: stamp(store().now - 6 * 24 * HOUR), online: true, seen: null, link: 'direct', fingerprint: 'quiet fox 0937', verified: true, rule: 'ask', internet: true },
    ],
    seen: [],
    code: null,
  })

  async function invoke(cmd, args = {}) {
    switch (cmd) {
      case 'get_lang':
        return lang
      case 'set_lang':
        hub.setLang?.(args.lang === 'zh' ? 'zh' : 'en')
        return args.lang
      case 'harness_icons':
        return window.__harnessIcons || {}
      case 'threads':
        return threads()
      case 'feed':
        return { dispatches: heldLetter() ? [{ dispatch_id: heldLetter().dispatch, recipients: [{ kind: 'review' }] }] : [] }
      case 'sessions': {
        const all = store().sessions.filter((s) => !args.runningOnly || s.running || s.contact).map(publicSession)
        return { hook_installed: true, sessions: all }
      }
      case 'search_sessions':
        return { sessions: store().sessions.filter((s) => matches(s, args.query ?? '')).map(publicSession) }
      case 'find':
        return { candidates: store().sessions.filter((s) => matches(s, args.query ?? '')).map((s) => ({ ...publicSession(s), reason: 'name' })) }
      case 'session_page':
        return sessionPage(args.id)
      case 'send':
        return send(args)
      case 'reply':
        return { route: 'inbox', detail: STAGE.delivered }
      case 'retry':
        return { retried: 0, copies: [], text: '' }
      case 'cancel':
        return { cancelled: 0, text: '' }
      case 'dispatch_status':
        return { dispatch_id: args.id, copies: [] }
      case 'open_session':
      case 'wait_open':
        return { kind: 'note', exact: false, note: L('In the app, this opens the session in Claude, Codex or Grok Bot.', '在真正的 App 里，这一步会在 Claude、Codex 或 Grok Bot 里打开这个会话。') }
      case 'review_list':
        return heldLetter() ? [heldLetter().id] : []
      case 'review_info':
        return reviewInfo()
      case 'review_decide':
        return decide(args.decision)
      case 'review_close':
        hub.hideReview?.()
        return null
      case 'review_open':
        hub.showReview?.()
        return null
      case 'review_settings':
      case 'review_forget':
        return { mode: settings.review, trusted: 2 }
      case 'set_review':
        settings.review = args.mode
        return { mode: settings.review, trusted: 2 }
      case 'wait_list':
        return { mode: settings.wake, waiting: [] }
      case 'wait_dismiss':
        return null
      case 'set_wake':
        settings.wake = args.mode
        return args.mode
      case 'set_notify':
        settings.notify = !!args.on
        return settings.notify
      case 'set_theme':
        settings.theme = args.theme
        return args.theme
      case 'set_inbound':
        settings.inbound = args.on ? 'accept' : 'hold'
        return { inbound: settings.inbound }
      case 'share_set': {
        const s = store().sessions.find((x) => x.id === args.id)
        if (s) s.share = args.level
        return args.level
      }
      case 'contact_set': {
        const id = String(args.session).split('/').pop()
        const s = store().sessions.find((x) => x.id === id)
        if (s) {
          s.name = args.name
          s.contact = args.name
          s.about = args.about
          if (args.avatar) s.avatar = args.avatar
        }
        changed()
        return { name: args.name, address: args.session, text: '' }
      }
      case 'contact_remove':
        return { removed: args.name }
      case 'me_set':
        store().me = { name: args.name, avatar: args.avatar }
        changed()
        return store().me
      case 'setup_status':
        return {
          hook_installed: true,
          binary: '/Users/you/.dispatch/bin/dispatch',
          bundled_cli: '/Applications/Dispatch.app/Contents/MacOS/dispatch',
          cli_sync_error: null,
          codex: '/Applications/Codex.app/Contents/Resources/codex',
          codex_daemon: true,
          codex_loaded: 1,
          codex_mcp: true,
          codex_plugin: true,
          grok_app: true,
          grok_signed_in: true,
          gbot: '/usr/local/bin/gbot',
          grok_bots: 1,
          grok_error: null,
          store_root: '/Users/you/Library/Application Support/Dispatch',
          claude_plugin: true,
          notify: settings.notify,
          theme: settings.theme,
          inbound: settings.inbound,
          review: settings.review,
          review_trusted: 2,
          wake: settings.wake,
          version,
        }
      case 'install_cli':
      case 'install_hook':
        return L('This is a demo: nothing was installed.', '这里是演示，没有真的安装任何东西。')
      case 'update_check':
        return { available: false, current: version }
      case 'update_install':
        return null
      case 'peers_overview':
      case 'peer_scan':
        return peers()
      case 'set_peer_internet':
        settings.internet = !!args.on
        return settings.internet
      case 'set_peer_relay':
        return { relay: null }
      case 'peer_code':
        return { code: '482 915', seconds_left: 300 }
      case 'peer_link':
        return { link: 'dispatch://pair/demo', seconds_left: 300 }
      case 'peer_set_name':
        return { id: 'this-mac', name: args.name }
      case 'peer_pair':
      case 'peer_verify':
      case 'peer_rule':
      case 'peer_remove':
      case 'peer_set_address':
      case 'open_local_network_settings':
        throw new Error(L('This is a demo: pairing is not available here.', '这里是演示，不能真的配对。'))
      case 'plugin:event|listen':
        me.handlers.push({ event: args.event, id: args.handler })
        return args.handler
      case 'plugin:event|unlisten':
        me.handlers = me.handlers.filter((h) => h.id !== args.eventId)
        return null
      case 'plugin:window|set_size': {
        const find = (v) => (v && typeof v === 'object' ? (typeof v.height === 'number' ? v.height : Object.values(v).map(find).find((x) => x != null)) : null)
        const h = find(args.value)
        if (h) hub.resizeReview?.(h)
        return null
      }
      default:
        return null
    }
  }

  let next = 1
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} }
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: location.hash === '#review' ? 'review' : 'main' }, currentWebview: { windowLabel: 'main', label: 'main' } },
    transformCallback(cb) {
      const id = next++
      window['_' + id] = cb
      return id
    },
    unregisterCallback(id) {
      delete window['_' + id]
    },
    convertFileSrc: (p) => p,
    invoke: (cmd, args) =>
      new Promise((resolve, reject) => {
        // a short pause, as a real backend has
        setTimeout(() => invoke(cmd, args).then(resolve, (e) => reject(String(e && e.message ? e.message : e))), 20)
      }),
  }
})()
