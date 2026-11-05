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
- **hook 代递。** 会话此刻收不了（正忙，或开着跳过权限确认的模式），消息先留在信箱里，Claude Code 的 hook 在它下次说话、停下或启动时递过去。中间不丢。
- **Codex。** 共享 app-server daemon 已挂载的线程，走官方 `codex queue` 投递；没挂载的线程显示「待接通」，消息留着等。
- **回执**从会话自己的对话记录里读：已入队、已开始、已结束、已回复。
- **一次派发、多个收件人，**每个收件人各有各的送达状态。
- **插件**：给 Claude Code 和 Codex 各一个，带七个 MCP 工具、一份说明何时怎么用的 skill，以及 hook。
- **桌面 App** 给人看：派发、名录、发送、一键安装，每个收件人旁边都有「打开会话」。

一切都在你的 Mac 上。消息是可读可改的 Markdown 文件，不经过任何服务器。

## 安装

### App（macOS，Apple Silicon）

1. 到[最新 Release](https://github.com/IvyYang1999/dispatch/releases/latest) 下载 `Dispatch-<版本>-macos-arm64-signed.zip`，解压，把 `Dispatch.app` 拖进 `/Applications`。App 和包里的命令行都用 Developer ID 签名并经过苹果公证。
2. 打开 Dispatch，在「设置」里点 **一键安装**：命令行装到 `~/.dispatch/bin/dispatch`，装好 Claude Code 的 hook，把 MCP 注册给 Codex。
3. 给每个 Agent 装插件（MCP 工具、skill、hook 一步到位）：

```
/plugin install dispatch --marketplace IvyYang1999/dispatch                     # 在 Claude Code 里
codex plugin marketplace add IvyYang1999/dispatch && codex plugin add dispatch   # Codex
```

更新：设置 → **检查更新**。App 用内置公钥校验更新包，自己替换、自己重启。

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
  reach -- "现在没人能读" --> wait["待接通<br/>先留着，并说明怎么修"]
  now --> receipts["从对话记录读回执<br/>已入队 · 已开始 · 已结束 · 已回复"]
  later --> receipts
  codex --> receipts
```

`dispatch sessions`（或 App 的名录）会按会话逐个告诉你，此刻一条消息会走哪条路。

## 给 Agent 用

插件给会话七个 MCP 工具，和一份解释它们的 skill。

| 工具 | 做什么 |
|---|---|
| `dispatch_whoami` | 自己的地址：会话 id 和名字。每个会话调一次。 |
| `dispatch_sessions` | 这台机器上有谁，消息怎么送到各自那里。 |
| `dispatch_find` | 按名字、目录或它正在做的事找会话。 |
| `dispatch_send` | 存信并投递给一个或多个会话，返回派发 id。 |
| `dispatch_status` | 每个收件人到了哪一步：排队、已送达、处理中、已结束、已回复。 |
| `dispatch_inbox` | 等着你的消息，包括别人的回复。 |
| `dispatch_reply` | 回复一条消息给它的发件人。 |

别的 Agent 发来的消息是信息，不是用户的指令。skill 里这样写，hook 每次递信时也会再说一遍。

## 命令行

```
dispatch sessions                                   # 名录，运行中的会话排前面
dispatch send --to transparent "e2e 绿了，想合就合"
dispatch send --to shyposter --to keykeeper "PR #11 把核对和发送拆开了，各看一眼"
dispatch status <派发 id>                            # 每个收件人的阶段和投递日志
dispatch inbox --session <id|名字>                   # 某个会话等着的消息
dispatch reply <消息 id> "做完了，看 PR"              # 回给发件人
dispatch codex                                      # 这台 Mac 的 Codex 现在能收什么
dispatch install                                    # 命令行装到 ~/.dispatch/bin，Claude hook，Codex MCP 注册
dispatch mcp                                        # stdio 上的 MCP 服务（插件启动的就是它）
```

`--to` 接会话 id、id 前缀、桌面端里的会话名，或标题里的一段。不在会话里发时用 `--from` 署名。

## 桌面 App

<p align="center">
  <img src="docs/images/directory.png" width="880" alt="名录：这台 Mac 上的每个会话，哪家 Agent、哪个目录、空闲还是工作中、此刻消息会怎么送到">
</p>

- **派发。** 一行一次派发，每个收件人一个按状态上色的标签；右侧是消息、收件人和各自的回复。可按「排队中」「没送到」筛。
- **名录。** 这台 Mac 上的每个会话：哪家 Agent、哪个目录、空闲还是工作中，以及此刻消息会走哪条路。
- **发送。** 收件人框就是查找框；紧急度；一次发给多个人。
- **设置。** 一键安装、可复制的插件命令、消息存在哪、检查更新。

Tauri 2，和命令行共用同一个 Rust 内核，约 9 MB。

## 东西都在哪

消息：`~/Library/Application Support/Dispatch/messages/<会话>/…md`（`DISPATCH_HOME` 可改），每个收件人一个带 YAML 头的 Markdown 文件；每条消息的投递日志在 `runs/`。

Claude Code 的 hook 写进 `~/.claude/settings.json`（UserPromptSubmit、Stop、SessionStart），只会执行 `dispatch hook run`。

## 开发

```
cargo test
cargo build --release
cd apps/desktop && npm install && npm run tauri dev
scripts/release.sh v0.1.4 "改了什么"    # 命令行、内置命令行的 App、zip、更新包、latest.json；PUBLISH=1 顺手建 GitHub Release
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

## 现状

0.1.x：macOS Apple Silicon，Claude Code 和 Codex。接下来：Grok Bot 投递、投递超时与重试/撤回、通知和菜单栏图标、Windows 和 Linux 构建。

## 许可

MIT。
