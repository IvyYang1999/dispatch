import { invoke } from '@tauri-apps/api/core'

export type ReachLevel = 'now' | 'later' | 'none'
/** How the user wants a session pictured: an emoji and/or a `#rrggbb` color. */
export interface Look {
  emoji?: string | null
  color?: string | null
}
export interface Reach {
  level: ReachLevel
  text: string
  fix: string | null
  /** Claude Code's permission mode or Codex's approval policy, for a running session. */
  mode?: string | null
  /** desktop, terminal, exec (Codex without a person), or what Codex calls its front end. */
  client?: string | null
  /** What sending does, in a person's words (see dispatch_core::reach::plain). */
  plain?: Plain
}
/** What a session lets other sessions see: its name only, name and about, plus recent questions, plus recent rounds. */
export type ShareLevel = 'none' | 'basic' | 'asks' | 'rounds'
export interface Plain {
  kind: 'now' | 'auto' | 'you' | 'turn' | 'speak' | 'machine' | 'off'
  title: string
  detail: string
  todo?: string | null
}
export interface Host {
  id: string
  name: string
}
export interface Session {
  /** What it shares with other sessions (on this machine's sessions). */
  share?: ShareLevel
  id: string
  short_id: string
  name: string
  /** The name its own app shows (the Claude app's session name, the Codex sidebar title). */
  app_name?: string | null
  harness: string
  title?: string | null
  project?: string | null
  cwd: string | null
  cwd_short: string | null
  running: boolean
  status: string | null
  desktop: boolean
  host_session_id: string | null
  last_active: number
  /** The machine it is on, when that is another one. */
  host: Host | null
  /** `machine/name` for a remote session, the name alone here. */
  address: string
  /** Its name in the address book, when it has one. */
  contact?: string | null
  /** One line on what it is for, from the address book. */
  about?: string | null
  /** The picture the user chose for it, from the address book. */
  avatar?: Look | null
  /** The name came from a search, not a person; shown apart until confirmed. */
  provisional?: boolean
  reach: Reach
  reason?: string
}
export type Kind = 'review' | 'queued' | 'delivered' | 'working' | 'replied' | 'failed' | 'cancelled' | 'held'
/** Where the user's review of a held copy stands, as the sender sees it too. */
export interface ReviewState {
  state: 'asked' | 'shown' | 'closed' | 'approved' | 'trusted' | 'declined' | 'withdrawn' | 'expired'
  at: string
  app_running?: boolean | null
  steps: { step: string; at: string }[]
}
export interface Run {
  at: string
  action: string
  result: string
  detail: string
}
export interface Copy {
  letter_id: string
  short_id: string
  from: string
  from_name: string
  from_harness?: string | null
  from_host_name?: string | null
  to: string
  to_name: string
  to_harness?: string | null
  to_host?: string | null
  to_host_name?: string | null
  status: string
  stage: string
  kind: Kind
  created: string
  deadline?: string
  path: string
  body: string
  runs?: Run[]
  review?: ReviewState
  reply?: { letter_id: string; body: string; created: string; status: string }
}
export interface Dispatch {
  dispatch_id: string
  from: string
  from_name: string
  from_harness?: string | null
  from_host_name?: string | null
  created: string
  urgency: string
  text: string
  recipients: Copy[]
}
export interface SetupStatus {
  hook_installed: boolean
  binary: string | null
  bundled_cli: string | null
  codex: string | null
  codex_daemon: boolean
  codex_loaded: number
  grok_app: boolean
  grok_signed_in: boolean
  gbot: string | null
  grok_bots: number
  grok_error: string | null
  store_root: string
  claude_plugin: boolean
  notify: boolean
  theme: 'system' | 'light' | 'dark'
  /** Claude Code's crossSessionInbound in the user's settings: accept / hold / refuse, or null. */
  inbound: string | null
  /** When a session's message waits for the user's review: off, new (first time per pair), all. */
  review: 'off' | 'new' | 'all'
  review_trusted: number
  /** The Dispatch MCP server is registered in ~/.codex/config.toml. */
  codex_mcp?: boolean
  /** The Dispatch plugin is installed in Codex. */
  codex_plugin?: boolean
  /** A message waiting for its session to be opened: auto (remind, open it when nobody is here), ask, off. */
  wake: 'auto' | 'ask' | 'off'
  version: string
  cli_sync_error: string | null
}
export interface UpdateCheck {
  available: boolean
  current: string
  version?: string
  notes?: string | null
  date?: string | null
}
/** One side of a conversation: a session here or elsewhere, or the person (`you`). */
export interface Party {
  key: string
  id: string
  name: string
  app_name: string | null
  harness: string | null
  host_id: string | null
  host_name: string | null
  person: boolean
  /** What to hand compose and open: `machine-id/session-id` elsewhere, the id here; null for the person. */
  address: string | null
  running: boolean
  status: string | null
  cwd_short: string | null
  avatar: Look | null
  /** Its name in the address book, when the user gave it one. */
  contact: string | null
  about: string | null
  /** The command line or an MCP caller with no recognized session behind it: not the person. */
  unknown?: boolean
}
/** One message in a conversation: a stored copy with the side that wrote it. */
export interface ThreadMessage extends Copy {
  dispatch_id: string
  side: 'a' | 'b'
  /** When it happened on this machine's clock, `YYYY-MM-DD HH:MM`. */
  at: string
  /** The same message went to these as well. */
  also_to?: string[]
}
/** Two parties and everything between them, oldest message first. */
export interface Thread {
  key: string
  a: Party
  b: Party
  last_at: string
  count: number
  /** The most pressing state that needs the user: review, failed, held or queued. */
  attention: Kind | null
  last: { side: 'a' | 'b'; body: string; kind: Kind; stage: string }
  messages: ThreadMessage[]
}
export interface Turn {
  role: 'user' | 'assistant'
  text: string
  at: string | null
  via_dispatch: boolean
  from_agent: string | null
}
export interface Round {
  user: Turn | null
  replies: Turn[]
}
export interface SessionPageData {
  session: Session
  dispatches: (Dispatch & { direction: 'in' | 'out' })[]
  rounds: Round[]
  transcript_available: boolean
}
/** One message held for review, as the review window shows it. */
export interface ReviewInfo {
  letter_id: string
  dispatch_id: string
  waiting: boolean
  from: string
  from_name: string
  from_harness: string | null
  from_avatar?: Look | null
  from_unknown?: boolean
  to: string
  to_name: string
  to_harness: string | null
  to_host: string | null
  to_host_name: string | null
  body: string
  urgency: string
  created: string
  deadline: string
  session: Session | null
  /** The last thing the person asked that session (else the last message it got). */
  last_query: Turn | null
  transcript_available: boolean
  review: ReviewState | null
  first_time: boolean
  mode: 'off' | 'new' | 'all'
  queued: number
}
export interface ReviewSettings {
  mode: 'off' | 'new' | 'all'
  trusted: number
}
export interface PeerRow {
  id: string
  name: string
  address: string | null
  paired_at: string
  online: boolean
  seen: { addrs: string[]; port: number } | null
  /** Why it is offline, when it is: the last failure in words. */
  problem?: string | null
  /** How it was last reached: lan, direct (over the internet) or relay. */
  link?: 'lan' | 'direct' | 'relay' | null
  /** What both machines show; a person compares them. */
  fingerprint: string
  verified: boolean
  rule: 'all' | 'contacts' | 'ask'
  /** Its internet key is known, so it can be reached off the local network. */
  internet: boolean
  /** A later pairing has the same name, so the name no longer reaches this record. */
  superseded?: boolean
  /** This system refused the connection (the Local Network permission, or a firewall). */
  blocked?: boolean
}
export interface SeenRow {
  id: string
  name: string
  addrs: string[]
  port: number
}
export interface PeerOverview {
  me: { id: string; name: string; public_key: string; listening: number | null; iroh_id: string | null; internet_listening: boolean }
  internet: boolean
  relay: string | null
  peers: PeerRow[]
  seen: SeenRow[]
  code: { code: string; seconds_left: number } | null
}
/** A message waiting for its session to be opened, on the machine where the session lives. */
export interface WaitItem {
  letter_id: string
  from_name: string
  from_host_name: string | null
  /** When it started waiting (RFC 3339). */
  since: string
  /** The session was already opened for it, by the user or by the app. */
  woken: boolean
  /** Why it needs the user: remind only is set (ask), opening failed, opened and still waiting (stuck), no way to open it here (manual). */
  why?: 'ask' | 'failed' | 'stuck' | 'manual'
  session: { id: string; name: string; app_name: string | null; harness: string; desktop: boolean; avatar: Look | null; url: string | null }
}

export interface OpenResult {
  kind: 'url' | 'command' | 'app' | 'note'
  url?: string
  app?: string
  command?: string
  cwd?: string | null
  exact: boolean
  note?: string
}

export const api = {
  sessions: (runningOnly: boolean) => invoke<{ hook_installed: boolean; sessions: Session[] }>('sessions', { runningOnly }),
  find: (query: string) => invoke<{ candidates: Session[] }>('find', { query }),
  feed: (limit = 200) => invoke<{ dispatches: Dispatch[] }>('feed', { limit }),
  threads: (limit = 200) => invoke<{ count: number; threads: Thread[] }>('threads', { limit }),
  status: (id: string) => invoke<{ dispatch_id: string; copies: Copy[] }>('dispatch_status', { id }),
  send: (to: string[], text: string, urgency: string, deadline?: string) => invoke<{ dispatch_id: string; recipients: { to: { name: string; address?: string }; route: string; detail: string }[] }>('send', { to, text, urgency, deadline }),
  reply: (id: string, text: string) => invoke<{ route: string; detail: string }>('reply', { id, text }),
  retry: (id: string) => invoke<{ retried: number; copies: { to_name: string; detail: string }[]; text: string }>('retry', { id }),
  cancel: (id: string) => invoke<{ cancelled: number; text: string }>('cancel', { id }),
  openSession: (id: string) => invoke<OpenResult>('open_session', { id }),
  setupStatus: () => invoke<SetupStatus>('setup_status'),
  installHook: () => invoke<string>('install_hook'),
  installCli: () => invoke<string>('install_cli'),
  updateCheck: () => invoke<UpdateCheck>('update_check'),
  updateInstall: () => invoke<void>('update_install'),
  getLang: () => invoke<string>('get_lang'),
  setLang: (lang: string) => invoke<string>('set_lang', { lang }),
  setNotify: (on: boolean) => invoke<boolean>('set_notify', { on }),
  setInbound: (on: boolean) => invoke<{ inbound: string | null }>('set_inbound', { on }),
  openLocalNetworkSettings: () => invoke<boolean>('open_local_network_settings'),
  setTheme: (theme: string) => invoke<string>('set_theme', { theme }),
  peersOverview: () => invoke<PeerOverview>('peers_overview'),
  peerScan: () => invoke<PeerOverview>('peer_scan'),
  peerSetName: (name: string) => invoke<{ id: string; name: string }>('peer_set_name', { name }),
  peerCode: () => invoke<{ code: string; seconds_left: number }>('peer_code'),
  peerPair: (target: string, code: string | null) => invoke<{ id: string; name: string; verified: boolean; fingerprint: string }>('peer_pair', { target, code }),
  peerLink: () => invoke<{ link: string; seconds_left: number }>('peer_link'),
  peerVerify: (name: string) => invoke<{ name: string; released: number }>('peer_verify', { name }),
  peerRule: (name: string, rule: string) => invoke<{ name: string; rule: string }>('peer_rule', { name, rule }),
  setPeerInternet: (on: boolean) => invoke<boolean>('set_peer_internet', { on }),
  setPeerRelay: (url: string | null) => invoke<{ relay: string | null }>('set_peer_relay', { url }),
  peerRemove: (name: string) => invoke<boolean>('peer_remove', { name }),
  contactSet: (name: string, session: string, about: string | null, avatar?: Look) => invoke<{ name: string; address: string; text: string }>('contact_set', { name, session, about, avatar: avatar ?? null }),
  searchSessions: (query: string) => invoke<{ sessions: Session[] }>('search_sessions', { query }),
  sessionPage: (id: string) => invoke<SessionPageData>('session_page', { id }),
  harnessIcons: () => invoke<Record<string, string>>('harness_icons'),
  meSet: (name: string | null, avatar: Look) => invoke<{ name?: string; avatar?: Look }>('me_set', { name, avatar }),
  contactRemove: (name: string) => invoke<{ removed: string }>('contact_remove', { name }),
  reviewList: () => invoke<string[]>('review_list'),
  reviewInfo: (id: string) => invoke<ReviewInfo>('review_info', { id }),
  reviewDecide: (id: string, decision: 'send' | 'trust' | 'decline') => invoke<{ letter_id: string; decision: string; route?: string; detail?: string; text: string; plain?: string }>('review_decide', { id, decision }),
  reviewClose: () => invoke<void>('review_close'),
  reviewOpen: () => invoke<void>('review_open'),
  reviewSettings: () => invoke<ReviewSettings>('review_settings'),
  setReview: (mode: 'off' | 'new' | 'all') => invoke<ReviewSettings>('set_review', { mode }),
  reviewForget: () => invoke<ReviewSettings>('review_forget'),
  waitList: () => invoke<{ mode: string; waiting: WaitItem[] }>('wait_list'),
  waitOpen: (id: string) => invoke<OpenResult>('wait_open', { id }),
  waitDismiss: () => invoke<void>('wait_dismiss'),
  setWake: (mode: 'auto' | 'ask' | 'off') => invoke<string>('set_wake', { mode }),
  shareSet: (id: string, level: ShareLevel) => invoke<string>('share_set', { id, level }),
  peerSetAddress: (name: string, address: string | null) => invoke<{ id: string; name: string; address: string | null }>('peer_set_address', { name, address }),
}
