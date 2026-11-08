#!/usr/bin/env node
// Build the dispatch command line in release mode and put it where the Tauri app expects its
// sidecar: apps/desktop/src-tauri/binaries/dispatch-<host triple>[.exe]. Pass --no-build to copy only.
import { execSync, spawnSync } from 'node:child_process'
import { copyFileSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const host = execSync('rustc -vV').toString().match(/^host: (.+)$/m)[1].trim()
const exe = process.platform === 'win32' ? '.exe' : ''

if (!process.argv.includes('--no-build')) {
  const r = spawnSync('cargo', ['build', '--release', '-p', 'dispatch'], { stdio: 'inherit', cwd: root })
  if (r.status !== 0) process.exit(r.status ?? 1)
}
const dir = join(root, 'apps', 'desktop', 'src-tauri', 'binaries')
mkdirSync(dir, { recursive: true })
const dst = join(dir, `dispatch-${host}${exe}`)
copyFileSync(join(root, 'target', 'release', `dispatch${exe}`), dst)
console.log(`sidecar: ${dst}`)
