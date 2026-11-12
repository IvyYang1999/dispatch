// Every outside link on the page, in one place. A null link shows as "soon" and goes nowhere:
// fill it in before publishing.
export const REPO = 'IvyYang1999/dispatch'

export const LINKS = {
  github: `https://github.com/${REPO}`,
  docs: `https://github.com/${REPO}#readme`,
  docsZh: `https://github.com/${REPO}/blob/main/README.zh-CN.md`,
  releases: `https://github.com/${REPO}/releases/latest`,
  latestApi: `https://api.github.com/repos/${REPO}/releases/latest`,
  // TODO before publishing: confirm the handle with yyt
  x: null as string | null,
  // TODO before publishing: the invite link of the Discord server
  discord: null as string | null,
  // TODO before publishing: the support address
  email: null as string | null,
}
