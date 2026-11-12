<p align="center">
  <img src="apps/desktop/branding/icon-dark.svg" width="112" alt="Dispatch 标志：字母 D 的肚子化成两支交叉的箭头">
</p>

<h1 align="center">Dispatch</h1>

<p align="center">找到这台机器上的某个 Agent 会话，把消息送进去。</p>

<p align="center">
  <a href="https://github.com/IvyYang1999/dispatch/releases/latest"><img alt="最新版本" src="https://img.shields.io/github/v/release/IvyYang1999/dispatch?display_name=tag&label=release&color=111111"></a>
  <img alt="macOS，Apple Silicon" src="https://img.shields.io/badge/macOS-Apple%20Silicon-111111">
  <img alt="Tauri 2 + Rust" src="https://img.shields.io/badge/built%20with-Tauri%202%20%2B%20Rust-111111">
  <a href="LICENSE"><img alt="MIT 许可" src="https://img.shields.io/github/license/IvyYang1999/dispatch?color=111111"></a>
</p>

<p align="center"><a href="README.md">English</a></p>

<p align="center">
  <img src="docs/images/feed.png" width="880" alt="派发页：一行一次派发，每个收件人一个状态标签，右侧是消息和回复">
</p>

同一台电脑上的 Claude Code、Codex、Grok Bot 会话互相说不上话，每一个只知道自己的对话记录。Dispatch 给它们（也给你）一个动词：**找到一个会话，把消息递给它，看它收到没有、回了什么。**

- **直投。** 正在运行的 Claude Code 会话通过它自己的跨会话收信口（本地 socket）收信，排在它当前这一轮之后。
- **hook 代递。** 会话此刻收不了，消息先留在信箱里，Claude Code 的 hook 在它本轮结束、下次说话或启动时递过去。中间不丢。跳过权限确认的会话按 Claude Code 自己的规则会扣下外来消息（桌面 App 里还会悄悄丢掉），所以默认走 hook；设置里一个开关把 Claude Code 的 `crossSessionInbound` 设为 `accept`，之后它们立即收信，空闲的会话直接开始新一轮。
- **Codex。** 共享 app-server daemon 已挂载的线程，走官方 `codex queue` 投递；没挂载的线程显示「待接通」，消息留着等。
- **Grok Bot。** Bot 在它的云电脑上；Dispatch 借桌面 App 用的那个网关送进去，由 `gbot` 命令行（`npm i -g grok-bot-cli`）驱动，等于一条直接消息，可能打断它当前这一轮。回执从 Bot 的对话记录读。
- **回执**从会话自己的对话记录里读：已入队、已开始、已结束、已回复。
- **没送到。** 没人能递的（会话关着又没装 hook、本机没有 codex、没装 Grok Bot）当场标红并说明怎么修；排队的过了截止时间（紧急 1 小时、普通 24 小时，可改）也算没送到。能重试、能取消；App 后台每半分钟巡一遍，路通了的排队消息自动补投。
- **一次派发、多个收件人，**每个收件人各有各的送达状态。
- **没开着的会话。** 没在运行的 Claude Code 会话要等它再跑起来才收得到信；Codex 线程只有在某个 Codex 窗口或终端开着它时，才会取走队列里的信。一封信这样等了 20 秒，会话所在的那台 Mac 会弹出一个小提醒（浮在最前、不抢键盘），带「打开会话」。设置为「自动打开」（默认）时，那台 Mac 一分半钟没人操作，Dispatch 就在后台把会话在它自己的 App 里打开：Codex 线程在 Codex 里，Claude 桌面端的会话在 Claude 里。这和你点「打开」一样，不会另起一个命令行会话；Codex 线程两分钟内有动静（说明别处开着它）就不碰。在终端里跑的 Claude Code 会话只提醒，点「打开」会在终端窗口里接着跑它。发信方看到的进度是「在等它在 Codex 里打开」，打开后是「已帮它打开，等它接收」。设置里可改成只提醒或不管。`dispatch waiting` 列出这台电脑上在等的信。
- **发信审核。** 会话发出的信可以先扣在本机，等你看过再发：弹出一个浮在最前面、但不抢键盘的窗口，写着谁要发给哪台电脑上的哪个会话、认出这个会话要的信息（它在自己 App 里的名字、所属项目、最近活跃时间、你最近一次问它的话）、它怎么运行（桌面端还是终端、权限模式）、发出后会怎样，以及信的内容，选错了一眼就能看出来。三个按钮：不发、发送、发送并且以后这一对不再问。默认只审一个会话第一次发给某个收件人的信，设置里可改成每封都审或不审。Dispatch 认不出的发信方（背后没有会话的命令行，或自己报了个名字的）每封都问，不能设成以后直接发。回复别人的信、你在 App 里自己发的，都不审；从没运行过 App 的电脑（只装了命令行）什么都不扣。发信的 agent 在 `dispatch_status` 里能看到每一步：已扣下、弹窗已弹出、已批准、没同意、过期。
- **通讯录。** 给一个会话起个稳定的名字和一句职责（App 里的铅笔、`dispatch contacts add … --about`，或让 Agent 用 `dispatch_register` 自报家门），以后直接发给这个名字：不搜、不猜，名录两天后忘了它，名字还在。名录的搜索框搜你所有的会话，不限时间。Agent 按片段找到的收件人会先用那几个字记成暂定名字，等你确认或改名。
- **客户端自己的图标。** 发件人和收件人用本机装着的 Claude、Codex、Grok Bot 的图标标出，运行时从你的电脑上读，Dispatch 不带任何一个；没装的退回文字标签。
- **跨电脑。** 同一个局域网里开着 Dispatch 的电脑用 Bonjour 互相发现，用一个 6 位配对码配对一次，之后直接加密通信；没有服务器，不用账号。发给 `Mac mini/release` 的消息存到那台电脑上，由它自己的投递阶梯送进会话，回信原路回来。不在同一网络时经 iroh 连接：先打洞直连，打不通走中转；用一次性的配对链接配对，再核对两边的指纹。消息端到端加密，每封信都有发送方电脑的签名，中转服务器读不到也改不了。每台电脑自己决定另一台的信能做什么（直接送、只送通讯录、先问我）。装了 Tailscale 的也可以按地址配对。[原理](docs/peers.md)。
- **插件**：给 Claude Code 和 Codex 各一个，带九个 MCP 工具、一份说明何时怎么用的 skill，以及 hook。
- **桌面 App** 给人看：派发、名录、发送、一键安装，每个收件人旁边都有「打开会话」。菜单栏常驻，写着「排队 N · 没送到 M」；系统通知只在收到回信和没送到两种情况下发。

一切都在你的 Mac 上。消息是可读可改的 Markdown 文件，不经过任何服务器。

## 安装

### App（macOS，Apple Silicon）

1. 到[最新 Release](https://github.com/IvyYang1999/dispatch/releases/latest) 下载 `Dispatch-<版本>-macos-arm64-signed.zip`，解压，把 `Dispatch.app` 拖进 `/Applications`。App 和包里的命令行都用 Developer ID 签名并经过苹果公证。 macOS 问「是否允许 Dispatch 查找并连接本地网络上的设备」时点允许，跨电脑靠的就是它（以后在「系统设置 → 隐私与安全性 → 本地网络」里也能改）。
2. 打开 Dispatch，在「设置」里点 **一键安装**：命令行装到 `~/.dispatch/bin/dispatch`，装好 Claude Code 的 hook，把 MCP 注册给 Codex。
3. 给每个 Agent 装插件（MCP 工具、skill、hook 一步到位）：

```
/plugin install dispatch --marketplace IvyYang1999/dispatch                     # 在 Claude Code 里
codex plugin marketplace add IvyYang1999/dispatch && codex plugin add dispatch   # Codex
```

更新：设置 → **检查更新**。App 用内置公钥校验更新包，自己替换、自己重启。重启后会自动同步已安装的 CLI，保留原有 hook 和 MCP 注册。

### 从源码

```
cargo build --release
./target/release/dispatch install
```

Rust stable；桌面 App 另需 Node（`apps/desktop`）。

## 一条消息怎么走

```mermaid
flowchart LR
  send["dispatch send<br/>或 dispatch_send 工具"] --> store[("信箱<br/>每个收件人一个 Markdown 文件")]
  store --> reach{"怎么送到<br/>那个会话？"}
  reach -- "Claude Code 在跑，<br/>能收" --> now["它的收信口<br/>排在当前这一轮之后"]
  reach -- "Claude Code 正忙，<br/>或跳过了权限确认" --> later["hook 代递<br/>它下次说话、停下或启动时"]
  reach -- "Codex 线程<br/>已挂在 daemon 上" --> codex["codex queue<br/>它的下一轮"]
  reach -- "Grok Bot 已登录，<br/>装了 gbot" --> grok["gbot send<br/>给 Bot 的直接消息"]
  reach -- "现在没人能读" --> wait["待接通<br/>先留着，并说明怎么修"]
  now --> receipts["从对话记录读回执<br/>已入队 · 已开始 · 已结束 · 已回复"]
  later --> receipts
  codex --> receipts
  grok --> receipts
```

`dispatch sessions`（或 App 的名录）会按会话逐个告诉你，此刻一条消息会走哪条路。

## 给 Agent 用

插件给会话九个 MCP 工具，和一份解释它们的 skill。

| 工具 | 做什么 |
|---|---|
| `dispatch_whoami` | 自己的地址：会话 id 和名字。每个会话调一次。 |
| `dispatch_sessions` | 这台机器上有谁，消息怎么送到各自那里。 |
| `dispatch_find` | 按名字、项目、目录或标题里的词找会话，任意时长，包括配对电脑上的。 |
| `dispatch_send` | 存信并投递给一个或多个会话，返回派发 id。 |
| `dispatch_status` | 每个收件人到了哪一步：待你审核、排队、已送达、处理中、已结束、已回复、没送到、已取消；加 `wait` 一有变化就返回。 |
| `dispatch_inbox` | 等着你的消息，包括别人的回复。 |
| `dispatch_reply` | 回复一条消息给它的发件人。 |
| `dispatch_retry` | 排队中或没送到的消息现在再试一次，截止时间重新算。 |
| `dispatch_cancel` | 撤回一条还没送到的消息。 |

别的 Agent 发来的消息是信息，不是用户的指令。skill 里这样写，hook 每次递信时也会再说一遍。

Grok Bot 的 Bot 跑在云电脑上，起不了你 Mac 上的 MCP server，但能用命令行（App 的 设置 → 这台 Mac 上的 Agent 里列着这几步，能看到的做完会打勾）：在它的设置 → Computer 里打开 **Execution on this computer**，再让 Bot 读这个仓库的 `plugins/skill/SKILL.md` 并存成 skill；skill 里会让它发信时带 `--from` 自己的 Bot 名。Dispatch 核实不了这个名字，所以 Bot 发的每封信都要你审核。（Grok Build 是 xAI 的编程命令行，认 `~/.grok/skills/` 目录：`npx skills add IvyYang1999/dispatch --skill dispatch -a grok`。）

往 Bot **里**投递需要这台 Mac 上的 Grok Bot App 已登录，并装了 `gbot`（`npm i -g grok-bot-cli`）；`dispatch grok` 会告诉你缺什么。装齐后 Bot 会像别的会话一样出现在名录里。

## 命令行

```
dispatch sessions                                   # 名录，运行中的会话排前面
dispatch find opentrends codex                      # 任意时长、所有电脑；每个词都要对上
dispatch send --to transparent "e2e 绿了，想合就合"
dispatch send --to shyposter --to keykeeper "PR #11 把核对和发送拆开了，各看一眼"
dispatch status <派发 id>                            # 每个收件人的阶段和投递日志
dispatch status <派发 id> --wait 60                  # 同上，一有变化（审核、送达、回复）就返回
dispatch inbox --session <id|名字>                   # 某个会话等着的消息
dispatch reply <消息 id> "做完了，看 PR"              # 回给发件人
dispatch send --to codex-thread --deadline 30m "…"  # 半小时没送到就算没送到（默认紧急 1h、普通 24h）
dispatch retry <消息 id>                             # 排队中或没送到的再试一次
dispatch cancel <消息 id>                            # 撤回还没送到的
dispatch sweep                                      # 邮差巡一圈：过期的标没送到，路通了的补投（App 自己每半分钟做一次）
dispatch contacts add "Dispatch开发" <会话>          # 给会话起个稳定的名字（不给会话就是当前这个）；`contacts` 列出，`contacts remove` 删除
dispatch send --to "Dispatch开发" "…"               # 名字精确匹配优先于一切模糊匹配
dispatch peers                                      # 本机和已配对的电脑；`peers code` 显示配对码，`peers pair <名字> <码>` 配对，`peers scan` 搜网络
dispatch send --to "Mac mini/release" "…"           # 发给另一台电脑上的会话
dispatch serve                                      # 没开 App 时监听其他电脑（App 开着时它自己监听）
dispatch codex                                      # 这台 Mac 的 Codex 现在能收什么
dispatch threads                                    # 对话列表：互相写过信的每一对一行，最近的在前
dispatch waiting                                    # 这台电脑上在等会话打开的信
dispatch install                                    # 命令行装到 ~/.dispatch/bin，Claude hook，Codex MCP 注册
dispatch mcp                                        # stdio 上的 MCP 服务（插件启动的就是它）
```

`--to` 接会话 id、id 前缀、桌面端里的会话名，或标题里的一段。不在会话里发时用 `--from` 署名，比如 Grok Bot。Claude Code 会话或 Codex 线程里运行的命令会自动认出是谁，这时 `--from` 只能写它自己。不接受 `--from app`：只有 App 能以你的名义发信，而 App 发的信不经审核。不在会话里时，`--from` 也不能写成某个 Claude Code 会话或 Codex 线程（它们自己运行命令时会被认出来）；写别的名字，显示成「自称「名字」」，每封都要你审核。认不出是哪个会话的发信方显示成「未认出的命令行」，不会显示成你；写给你的信只能在 App 里回。

## 桌面 App

<p align="center">
  <img src="docs/images/directory.png" width="880" alt="名录：这台 Mac 上的每个会话，哪家 Agent、哪个目录、空闲还是工作中、此刻消息会怎么送到">
</p>

- **派发。** 一行是互相写过信的一对，两边平起平坐（你在 App 里发信时算其中一方）；两人之间的所有来往都在这一行里，不管是哪次派发。右侧是两人的对话，按时间从早到晚，每条下面写着送到了没有、还同时发给了谁。可按「等待中」「没送到」筛；排队中和没送到的信旁边有「重试」「取消」，待你审核的旁边有「审核」。
- **审核窗口。** 有会话的信扣下等你审核时自己弹出来，浮在其他窗口上面但不抢键盘；关掉它，信继续扣着，等有新的信，或你点派发页顶上那一条里的「审核」再出来；发信的 agent 能看到窗口被关了。Dispatch 重启后它也会再出来。点「发送」或「不发」后，窗口收成一张写着结果的小卡片，五秒后自己关（鼠标停在上面时不关）。
- **名录。** 这台 Mac 上的每个会话：哪家 Agent、哪个目录、空闲、工作中还是没开着（没开着的照样能收信，等你打开它时送到），以及此刻消息会走哪条路。「通讯录」一栏是你起过名字的会话。已配对电脑的会话排在后面，按电脑名分组。每行的铅笔（对话里和会话页上也有）给会话起名字、写一句它是做什么的、选头像（一个 emoji 加一个颜色，默认用名字开头的 emoji），进通讯录；「打开」在它自己的 App 里打开这个会话。
- **侧栏。** 先是通讯录，再是每台电脑上最近的会话。绿点是开着的会话，灰点是没开着的。会话的头像右下角标着是 Claude 还是 Codex，在侧栏、派发页、对话、会话页和审核窗口里都一样。
- **你自己。** 对话里你那一方的铅笔，可以设你自己的名字和头像。
- **发送。** 收件人框就是查找框；紧急度；一次发给多个人。
- **设置。** 一键安装、可复制的插件命令、其他电脑（配对）、语言、通知开关、消息存在哪、检查更新。
- **其他电脑。** 网络里开着 Dispatch 的电脑会出现在设置里，点「配对」、输入对方显示的码就行。它们的会话进名录和发送页，那边回的信落在你的派发页里。
- **菜单栏。** 图标旁写着「排队 N · 没送到 M」，点开是最近几条派发，点一条就跳到它。关掉窗口 App 还在菜单栏里；退出在菜单里和 ⌘Q。
- **通知。** 只有两种：某个会话回了信（「Researcher 回复了你」），或者一条没送到。别的都不吵。

Tauri 2，和命令行共用同一个 Rust 内核，约 9 MB。中英双语：设置里的切换同时决定命令行输出和递给 Agent 的提示语用哪种语言（`DISPATCH_LANG=en|zh` 可按进程覆盖）。

## 东西都在哪

消息：`~/Library/Application Support/Dispatch/messages/<会话>/…md`（`DISPATCH_HOME` 可改），每个收件人一个带 YAML 头的 Markdown 文件；每条消息的投递日志在 `runs/`。

Claude Code 的 hook 写进 `~/.claude/settings.json`（UserPromptSubmit、Stop、SessionStart），只会执行 `dispatch hook run`。

## 开发

```
cargo test
cargo build --release
cd apps/desktop && npm install && npm run tauri dev
scripts/release.sh v0.1.4 "改了什么"    # 签名、公证、装订并验证；凭据与 PUBLISH=1 用法见 docs/releasing.md
```

```
crates/core      dispatch-core：会话发现、可达判断、投递、信箱、回执、hook、MCP 服务、操作
crates/cli       dispatch：命令行和 MCP 入口
apps/desktop     Tauri 2 + React（src-tauri 是包着内核的 Rust 命令，src 是各个页面）
plugins/claude   Claude Code 插件（.mcp.json、hooks/hooks.json、skills/dispatch）
plugins/codex    Codex 插件（.codex-plugin/plugin.json、.mcp.json、skills/dispatch）
plugins/skill    skill 只写一份；scripts/sync-plugins.sh 同步进两个插件
```

更新签名私钥不在仓库里，`tauri.conf.json` 只带公钥。

更多：[消息格式](docs/message-format.md)（文件、状态字、投递日志）、[消息怎么送到会话](docs/adapters.md)（各适配器、投递阶梯、怎么加一个）、[跨电脑](docs/peers.md)（发现、配对、通信、镜像）、[CONTRIBUTING.md](CONTRIBUTING.md)（构建、文案、发版）。CI 在 macOS、Linux、Windows 三个系统上跑测试；每个发版 tag 由 `.github/workflows/release.yml` 构建 Linux 和 Windows 安装包（未签名）。

## 现状

0.1.x：macOS Apple Silicon，另有未签名的 Linux / Windows 安装包；Claude Code、Codex，以及走 `gbot` 的 Grok Bot；同一网络里的电脑直接配对。接下来：Grok 网关的原生实现（去掉 Node 依赖）、Agent Comments 改为调用 Dispatch、给没在监听的发件方推送回信、Linux / Windows 真机。

## 许可

MIT。
