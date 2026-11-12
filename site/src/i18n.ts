import type { Lang, ScenarioId } from './Demo'

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
  tryLabel: string
  tryHint: string
  replay: string
  scenarios: Record<ScenarioId, { title: string; sub: string }>
  points: { title: string; body: string }[]
  startTitle: string
  steps: { title: string; body: string }[]
  email: string
  soon: string
  license: string
}

export const PAGE: Record<Lang, Page> = {
  en: {
    title: 'Dispatch: your agents write to each other, you decide what goes out',
    h1: 'Your agents write to each other. You decide what goes out.',
    sub: 'Dispatch carries messages between Claude Code, Codex and Grok Bot sessions, on one Mac or between your Macs. It runs on your own machines: no server, no account.',
    download: 'Download for macOS',
    source: 'View on GitHub',
    version: 'Version',
    meta: 'Apple Silicon, signed and notarized.',
    other: 'Linux and Windows builds',
    docs: 'Docs',
    readDocs: 'Read the docs',
    tryLabel: 'Try it',
    tryHint: 'This is a demo you can use: when the review card comes up, the choice is yours.',
    replay: 'Play again',
    scenarios: {
      handoff: { title: 'Hand work over', sub: 'One session asks another. The answer comes back into its own conversation.' },
      machines: { title: 'Between your Macs', sub: 'Pair two Macs once with a six-digit code. Messages go straight across, encrypted end to end.' },
      closed: { title: 'A session that is closed', sub: 'Dispatch opens it in its own app, and the message goes in.' },
      unknown: { title: 'A sender it cannot recognize', sub: 'Whatever Dispatch cannot tie to a session is asked about every time.' },
    },
    points: [
      { title: 'Nothing leaves without you', body: 'The first message from one session to another waits in a review window: who is writing to whom, what it says, and what will happen once it goes. After that, a pair you trust can go straight through.' },
      { title: 'It finds the session', body: 'Open, busy or closed: Dispatch delivers at once, waits for the turn to end, or opens the session itself. The sender can follow every step, and so can you.' },
      { title: 'Private by default', body: 'Other agents see a session’s name and the line you wrote about it, and nothing else unless you share more. Messages are plain files on your Mac.' },
      { title: 'Works with what you run', body: 'Claude Code in the desktop app or a terminal, Codex in its app or CLI, and Grok Bot. An MCP server, a command line and a hook, set up from one Settings page.' },
    ],
    startTitle: 'Three steps',
    steps: [
      { title: 'Download and open Dispatch', body: 'It stays in the menu bar. Updating is one click in Settings.' },
      { title: 'Choose Install in Settings', body: 'That adds the command line, the Claude Code hook and the Codex registration. The page ticks off each step.' },
      { title: 'Ask a session to write to another', body: '"Ask the release session to ship 1.4." It finds the session, you approve, and the answer comes back.' },
    ],
    email: 'Email',
    soon: 'soon',
    license: 'Open source under the MIT license.',
  },
  zh: {
    title: 'Dispatch：让你的 Agent 互相写信，发不发由你定',
    h1: '让你的 Agent 互相写信，发不发由你定',
    sub: 'Dispatch 在 Claude Code、Codex 和 Grok Bot 的会话之间送信，同一台 Mac 上可以，几台 Mac 之间也可以。它只跑在你自己的电脑上：没有服务器，不用注册账号。',
    download: '下载 macOS 版',
    source: '在 GitHub 上查看',
    version: '版本',
    meta: 'Apple 芯片，已签名并公证。',
    other: 'Linux 和 Windows 版',
    docs: '文档',
    readDocs: '看文档',
    tryLabel: '试一下',
    tryHint: '这个演示可以点：审核卡片出现后，发不发由你决定。',
    replay: '再放一遍',
    scenarios: {
      handoff: { title: '把事交给另一个会话', sub: '一个会话请另一个会话做事，回信会回到它自己的对话里。' },
      machines: { title: '在你的几台 Mac 之间', sub: '两台 Mac 用 6 位配对码配对一次，信直接传过去，端到端加密。' },
      closed: { title: '对方没开着', sub: 'Dispatch 会在它自己的 App 里把它打开，信随后送进去。' },
      unknown: { title: '认不出的发信方', sub: 'Dispatch 认不出是哪个会话发的，每一封都先问你。' },
    },
    points: [
      { title: '没有你点头，信发不出去', body: '一个会话第一次给另一个会话写信时，信会先停在审核窗口里：谁写给谁、写了什么、发出后会怎样。之后，你信得过的这一对可以直接发。' },
      { title: '它会找到那个会话', body: '开着、正忙、还是没开着：Dispatch 会马上送到、等这一轮做完，或者自己把会话打开。发信方看得到每一步，你也看得到。' },
      { title: '默认不外露', body: '别的 Agent 只看得到会话的名字和你写的那句简介，除非你开放更多。信就是你 Mac 上的普通文件。' },
      { title: '你在用的都支持', body: '桌面端或终端里的 Claude Code、App 或命令行里的 Codex，还有 Grok Bot。一个 MCP server、一个命令行和一个 hook，在一个设置页里装好。' },
    ],
    startTitle: '三步开始',
    steps: [
      { title: '下载并打开 Dispatch', body: '它待在菜单栏里，更新在设置里点一下就行。' },
      { title: '在设置里点「安装」', body: '会装好命令行、Claude Code 的 hook，并把 Dispatch 注册给 Codex。每一步做完都会打勾。' },
      { title: '让一个会话给另一个会话写信', body: '「让 release 会话把 1.4 发出去。」它会找到那个会话，你点头，回信就回来了。' },
    ],
    email: '邮箱',
    soon: '即将开放',
    license: '开源，MIT 许可。',
  },
}
