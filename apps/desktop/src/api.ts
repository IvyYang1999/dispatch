import { invoke } from '@tauri-apps/api/core'

export type ReachLevel = 'now' | 'later' | 'none'
export interface Reach {
  level: ReachLevel
  text: string
  fix: string | null
}
export interface Host {
  id: string
  name: string
}
export interface Session {
  id: string
  short_id: string
  name: string
  harness: string
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
  reach: Reach
  reason?: string
}
export type Kind = 'queued' | 'delivered' | 'working' | 'replied' | 'failed' | 'cancelled'
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
  from_host_name?: string | null
  to: string
  to_name: string
  to_host_name?: string | null
  status: string
  stage: string
  kind: Kind
  created: string
  deadline?: string
  path: string
  body: string
  runs?: Run[]
  reply?: { letter_id: string; body: string; created: string; status: string }
}
export interface Dispatch {
  dispatch_id: string
  from: string
  from_name: string
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
export interface PeerRow {
  id: string
  name: string
  address: string | null
  paired_at: string
  online: boolean
  seen: { addrs: string[]; port: number } | null
}
export interface SeenRow {
  id: string
  name: string
  addrs: string[]
  port: number
}
export interface PeerOverview {
  me: { id: string; name: string; public_key: string; listening: number | null }
  peers: PeerRow[]
  seen: SeenRow[]
  code: { code: string; seconds_left: number } | null
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
  setTheme: (theme: string) => invoke<string>('set_theme', { theme }),
  peersOverview: () => invoke<PeerOverview>('peers_overview'),
  peerScan: () => invoke<PeerOverview>('peer_scan'),
  peerSetName: (name: string) => invoke<{ id: string; name: string }>('peer_set_name', { name }),
  peerCode: () => invoke<{ code: string; seconds_left: number }>('peer_code'),
  peerPair: (target: string, code: string) => invoke<{ id: string; name: string }>('peer_pair', { target, code }),
  peerRemove: (name: string) => invoke<boolean>('peer_remove', { name }),
  peerSetAddress: (name: string, address: string | null) => invoke<{ id: string; name: string; address: string | null }>('peer_set_address', { name, address }),
}
