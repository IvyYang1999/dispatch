import { invoke } from '@tauri-apps/api/core'

export type ReachLevel = 'now' | 'later' | 'none'
export interface Reach {
  level: ReachLevel
  text: string
  fix: string | null
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
  reach: Reach
  reason?: string
}
export type Kind = 'queued' | 'delivered' | 'working' | 'replied' | 'failed'
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
  to: string
  to_name: string
  status: string
  stage: string
  kind: Kind
  created: string
  path: string
  body: string
  runs?: Run[]
  reply?: { letter_id: string; body: string; created: string; status: string }
}
export interface Dispatch {
  dispatch_id: string
  from: string
  from_name: string
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
  version: string
}
export interface UpdateCheck {
  available: boolean
  current: string
  version?: string
  notes?: string | null
  date?: string | null
}
export interface OpenResult {
  kind: 'url' | 'command' | 'app'
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
  send: (to: string[], text: string, urgency: string) => invoke<{ dispatch_id: string; recipients: { to: { name: string }; route: string; detail: string }[] }>('send', { to, text, urgency }),
  reply: (id: string, text: string) => invoke<{ route: string; detail: string }>('reply', { id, text }),
  openSession: (id: string) => invoke<OpenResult>('open_session', { id }),
  setupStatus: () => invoke<SetupStatus>('setup_status'),
  installHook: () => invoke<string>('install_hook'),
  installCli: () => invoke<string>('install_cli'),
  updateCheck: () => invoke<UpdateCheck>('update_check'),
  updateInstall: () => invoke<void>('update_install'),
  getLang: () => invoke<string>('get_lang'),
  setLang: (lang: string) => invoke<string>('set_lang', { lang }),
}
