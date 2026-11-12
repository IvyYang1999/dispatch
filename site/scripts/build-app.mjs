// Builds the desktop app's own frontend into public/app, for the landing page to embed, and puts
// the demo's stand-in backend (app-demo/) in front of it. The page then shows the real interface,
// answering from made-up data.
import { execFileSync } from 'node:child_process'
import { cpSync, existsSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const site = join(dirname(fileURLToPath(import.meta.url)), '..')
const app = join(site, '..', 'apps', 'desktop')
const out = join(site, 'public', 'app')
const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm'
const npx = process.platform === 'win32' ? 'npx.cmd' : 'npx'

if (!existsSync(join(app, 'node_modules'))) {
  execFileSync(npm, ['ci'], { cwd: app, stdio: 'inherit' })
}
rmSync(out, { recursive: true, force: true })
execFileSync(npx, ['vite', 'build', '--base', './', '--outDir', out, '--emptyOutDir'], { cwd: app, stdio: 'inherit' })
cpSync(join(site, 'app-demo', 'mock.js'), join(out, 'mock.js'))
cpSync(join(site, 'app-demo', 'harness-icons.js'), join(out, 'harness-icons.js'))

const index = join(out, 'index.html')
const html = readFileSync(index, 'utf8')
// after the charset, before the app's own script
const anchor = '<meta charset="UTF-8" />'
if (!html.includes(anchor)) throw new Error('the app index.html has no charset line to put the demo backend after')
writeFileSync(index, html.replace(anchor, anchor + '\n    <meta name="robots" content="noindex" />\n    <script src="./harness-icons.js"></script>\n    <script src="./mock.js"></script>'))
console.log('app demo built into', out)
