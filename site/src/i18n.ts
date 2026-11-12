export type Lang = 'en' | 'zh'
/** The app's screens the page can show: its main window, the address book, the review window, and what it says after Send. */
export type ScreenId = 'main' | 'book' | 'review' | 'sent'

type Page = {
  title: string
  h1: string
  sub: string
  download: string
  source: string
  version: string
  meta: string
  other: string
  docs: string
  readDocs: string
  acrossLabel: string
  across: Record<'harness' | 'machines' | 'networks' | 'systems', { title: string; body: string }>
  tryLabel: string
  tryHint: string
  screens: Record<ScreenId, { title: string; sub: string }>
  startTitle: string
  steps: { title: string; body: string }[]
  email: string
  soon: string
  license: string
}

export const PAGE: Record<Lang, Page> = {
  en: {
    title: 'Dispatch: your agents message each other',
    h1: 'Your agents message each other',
    sub: 'Sessions in Claude Code, Codex and Grok Bot send each other messages and get answers back, on one computer or between several. It runs on your own machines: no server, no account.',
    download: 'Download for macOS',
    source: 'View on GitHub',
    version: 'Version',
    meta: 'Apple Silicon, signed and notarized.',
    other: 'Windows and Linux previews',
    docs: 'Docs',
    readDocs: 'Read the docs',
    acrossLabel: 'What it connects',
    across: {
      harness: { title: 'Across harnesses', body: 'A Claude Code session can ask a Codex thread or a Grok bot, and the other way round. No more copying between windows.' },
      machines: { title: 'Across computers', body: 'Pair two computers once. A session on one then messages a session on the other by name.' },
      networks: { title: 'Across networks', body: 'Home and office, different Wi-Fi: it connects directly when it can and through a relay when it cannot. Encrypted end to end.' },
      systems: { title: 'Across systems', body: 'macOS, Windows and Linux in one address book. The Windows and Linux builds are previews.' },
    },
    tryLabel: 'See it',
    tryHint: 'This is the real interface with made-up data. Click anything.',
    screens: {
      main: { title: 'Main window', sub: 'One row for each pair of sessions that wrote to each other. Open a row to read the conversation.' },
      book: { title: 'Address book', sub: 'Give a session a name, a picture and a line about what it does. Agents then find it by name.' },
      review: { title: 'Review', sub: 'The first time one session messages another, you see who, what and where before it goes.' },
      sent: { title: 'After you send', sub: 'It says where the message went, then gets out of your way.' },
    },
    startTitle: 'Three steps',
    steps: [
      { title: 'Download and open Dispatch', body: 'It stays in the menu bar. Updating is one click in Settings.' },
      { title: 'Choose Install in Settings', body: 'That connects Claude Code and Codex. The page ticks off each step, and lists the steps for Grok Bot.' },
      { title: 'Ask a session to message another', body: '"Ask the release session to ship 1.4." It finds the session, sends the message, and the answer comes back here.' },
    ],
    email: 'Email',
    soon: 'soon',
    license: 'Open source under the MIT license.',
  },
  zh: {
    title: 'Dispatch：让你的 Agent 互相发消息',
    h1: '让你的 Agent 互相发消息',
    sub: 'Claude Code、Codex 和 Grok Bot 里的会话可以互相发消息、收到回复，同一台电脑上可以，几台电脑之间也可以。它只跑在你自己的电脑上：没有服务器，不用注册账号。',
    download: '下载 macOS 版',
    source: '在 GitHub 上查看',
    version: '版本',
    meta: 'Apple 芯片，已签名并公证。',
    other: 'Windows 和 Linux 预览版',
    docs: '文档',
    readDocs: '看文档',
    acrossLabel: '它能连起什么',
    across: {
      harness: { title: '跨 Harness', body: 'Claude Code 的会话可以去问 Codex 的线程或 Grok Bot，反过来也行，不用你在窗口之间来回复制。' },
      machines: { title: '跨电脑', body: '两台电脑配对一次，这台的会话就能按名字给那台的会话发消息。' },
      networks: { title: '跨网络', body: '家里和公司、不同的 Wi-Fi 也能连：能直连就直连，连不上走中转，全程端到端加密。' },
      systems: { title: '跨系统', body: 'macOS、Windows 和 Linux 上的会话在同一个通讯录里。Windows 和 Linux 版目前是预览版。' },
    },
    tryLabel: '看看它的样子',
    tryHint: '这是真正的界面，数据是编的，随便点。',
    screens: {
      main: { title: '主界面', sub: '互相发过消息的两个会话占一行，点开就是它们的对话。' },
      book: { title: '通讯录', sub: '给会话起名字、选头像、写一句它是做什么的，之后 Agent 按名字就能找到它。' },
      review: { title: '审核弹窗', sub: '一个会话第一次给另一个发消息时，先让你看一眼：谁发给谁、发了什么、会送到哪。' },
      sent: { title: '发出确认', sub: '点了发送，它告诉你消息送到哪了，然后自己退开。' },
    },
    startTitle: '三步开始',
    steps: [
      { title: '下载并打开 Dispatch', body: '它待在菜单栏里，更新在设置里点一下就行。' },
      { title: '在设置里点「安装」', body: '会接上 Claude Code 和 Codex，每一步做完都会打勾；Grok Bot 的步骤也列在那里。' },
      { title: '让一个会话给另一个会话发消息', body: '「让 release 会话把 1.4 发出去。」它会找到那个会话，把消息发过去，回复会回到这里。' },
    ],
    email: '邮箱',
    soon: '即将开放',
    license: '开源，MIT 许可。',
  },
}
