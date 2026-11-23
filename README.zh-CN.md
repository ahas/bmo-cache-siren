[English](README.md) | [한국어](README.ko.md) | [日本語](README.ja.md) | **简体中文**

# bmo-cache-siren

逐个请求实时监视模型请求的提示缓存命中和新 token,一旦新 token 激增就拉响警笛。它不额外添加任何日志,而是跟读
Claude Code 的会话记录,同时接收不留会话记录的运行器通过回环地址上报的用量。它为每个请求分类,指出缓存未命中的
可能原因,并在单个请求或短时间内的一批请求重新预填充大段上下文时发出警报。Harness 把它作为 MCP 服务器接入
(`bmo-cache-siren mcp`,配合 `bmo-cache-siren` 技能),读取自身缓存的保持情况;它本身不调用任何其他服务。

冷启动式的重新预填充很容易被忽略,代价却很高:几个会话同时冷启动,一分钟内就可能有数百万 token 走过预填充,
在有人察觉之前就耗掉五小时配额的一大部分。警笛让这种情况在发生的当下就能被看见。

## 安装与运行

核心是一个用 Cargo 构建的 Rust 二进制文件,可在任何操作系统上运行。

在 BMO 机器上,守护进程由智能体按照索引的 INSTALL.md(`bmo install`,https://bmo.ahas.io/install.md)安装。

守护进程监听 `127.0.0.1:9022`。首次运行时,它会把包含全部默认值和一个随机 API 令牌的 `config.json` 写入数据目录;
每个配置键以及 `serve` 的每个参数都列在 `docs/configuration.md` 中。浏览器页面就是实时表格,用 `page` 打印出的
地址(其中带有令牌)打开。同一个二进制文件的其他子命令都通过客户端与正在运行的守护进程通信。

各平台的情况如下:

- **macOS:** 使用 Swift 悬浮窗应用(`macos/build.sh` 构建出 `dist/CacheSiren.app`),用 `afplay` 播放警笛,
  用 launchd 实现自启动(`deploy/launchd/`)。
- **Linux:** 用一个始终置顶的浏览器窗口代替悬浮窗,警笛通过主机上已有的 `pw-play`、`paplay` 或 `aplay` 之一
  播放,由 systemd --user 启动守护进程(`deploy/systemd/cache-siren.service`);其余内容见 `docs/porting.md`。

## 类型与警报

| 类型 | 含义 | 级别 |
| --- | --- | --- |
| `warm` | 新 token 少于 20K | info |
| `growth` | 缓存读取大于零,且新 token 达到 20K 或以上 | yellow;100K 或以上为 red |
| `start` | 会话的第一个请求 | info;新 token 达到 20K 或以上为 yellow |
| `cold` | 重新预填充:几乎或完全没有从缓存读取,新 token 达到 20K 或以上 | yellow;100K 或以上为 red |
| `refusal` | 请求被拒绝 | red |
| `quota`、`error`、`compact` | 配额停止、API 错误、上下文压缩 | yellow |
| `cmd` | 本地命令 | info |
| `notice` | 提示性的一行 | info |
| `request` | 用量未知的上报请求(标注 `usage_unavailable`),或会话首个请求之后、未从缓存读取任何内容且新 token 保持在 20K 以下的上报请求 | info |
| `burst` | 所有会话在 60 秒内的新 token 合计达到 500K | red |

red 级别的请求或 burst 会拉响警笛,每个会话在每个冷却期内只响一次。由本地模型处理的请求不产生费用,因此级别
不会超过 yellow。

cold 请求会附带可能的原因:`session_start`、`model_switch`、`effort_model`、`compact`、`ttl_expired`(间隔超过了
该模式和模型的缓存寿命)、`prefix_changed`(读回的内容不到上一次上下文的一半)、`tool_search`、`image` 或
`unknown`。

这些阈值来自大量真实会话样本中每个请求的新 token 分布:中位数约 2K,p90 约 8K,p95 约 12K,p99 约 170K。约 3% 的
请求达到 20K,不到 2% 的请求达到 100K。阈值都是配置键,请按自己的分布重新校准。

不留会话记录的运行器把用量上报到 `POST /events`;请求体、其他所有路由、事件、客户端以及 harness 插件都在
`docs/api.md` 中说明。

## 用法

```
bmo-cache-siren                   # status
bmo-cache-siren tail 30           # the last 30 events
bmo-cache-siren stats             # cache hits and new tokens by session
bmo-cache-siren mute|unmute       # quiet or arm the siren
bmo-cache-siren settings --threshold 150000
bmo-cache-siren test              # sound the siren once
bmo-cache-siren page              # the live page's address, with its token
bmo-cache-siren mcp               # the MCP server, for a harness
bmo-cache-siren restart           # launchd on macOS, systemd --user elsewhere
bmo-cache-siren log 50
```

这些子命令会携带数据目录中令牌文件里的令牌,向 `http://127.0.0.1:9022` 或 `CACHE_SIREN_ADDRESS` 指定地址上的
守护进程发出请求;加上 `--json` 则原样打印 API 的应答。

悬浮窗可以从任意位置拖动,从右下角调整大小,点击某一行会打开其详情;点击标题栏的 `—` 会将其隐藏,直到下一次
`bmo-cache-siren restart`。`local` 按钮用于显示或隐藏本地模型的行,浏览器会记住这个选择。

Web 控制台的缓存警笛页面像客户端一样读取这个 API:按会话的汇总和警笛次数来自 `GET /stats`,开关和阈值来自
`GET /settings`,最新事件则以 JSON 分页而非事件流的形式来自 `GET /events`。服务本身不保存配额和已读取文件数,
因此页面不显示这两项;它读取什么、显示什么,写在 `docs/api.md` 中。

## 运维

- `~/.claude/cache-siren/cache-siren.log` 中的日志包含 INFO、WARN 和 ERROR 行;访问日志是关闭的。守护进程每十
  分钟写一行存活记录。
- 守护进程停止后,悬浮窗每三秒重试一次,页面每两秒重新读取事件并重连事件流。请重启服务,并用
  `lsof -nP -iTCP:9022` 检查端口。
- `~/.claude/cache-siren/events.jsonl` 中的事件日志按 `retention_days` 清理;启动时最新的事件会恢复到窗口中,
  编号接着往下延续。

## 已知限制

- 会话记录在响应之后才写入,所以一行会在请求后两到五秒出现。这是一个监视器,而不是挡在请求前面的代理,
  它无法阻止请求。
- 位于代理之后、且代理上报缓存读取为零的本地模型,其每个请求都会显示为 cold。

## 子代理扇出与五小时配额

警笛能抓到的代价最高的模式,是大范围调研时递归式的子代理扇出。在一次实测中,单个会话在八分钟内启动了 35 个子
代理,执行了 368 次网页搜索,消耗约 27M token,大约是五小时配额的 30%。

测量结果表明:

- 在数百个会话中,常规调研在 p90 到 p95 处使用 3 到 7 个子代理和几十次搜索。离群值则使用 26 到 40 个子代理和远超
  一百次的搜索:这是一道十倍的断崖,而不是平缓的长尾。
- 在那次高成本会话中,累积的搜索结果约占 token 的 89%,子代理的首次预填充约占 6%,主会话的汇总约占 6%。
- 368 次搜索中有 367 次互不重复。这些搜索是实实在在的覆盖面而不是浪费,限制搜索次数就会削减覆盖面。

成本来自递归,以及大量子代理同时预填充。要限制的是这两点,而不是搜索。可以在 shell 环境中,或在
`.claude/settings.json` 的 `env` 块中设置:

```
CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH=1      # no agent spawns agents: removes the multiplication
CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS=6      # damps concurrent prefill spikes, at a little speed
CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION=20    # a safety net
```

`CLAUDE_CODE_MAX_WEB_SEARCHES_PER_SESSION` 请保持默认值。把工具限制为只能搜索的轻量调研子代理,能把首次预填充
降到约四分之一,但在深度调研中整体只能节省约 5%。

## 目录

| 目录 | 内容 |
| --- | --- |
| `src` | `cache_siren` 库(含客户端和 MCP 服务器)以及 `bmo-cache-siren` 二进制文件 |
| `tests` | 全部测试,以及路由测试所启动的进程内服务器 |
| `package` | 由 `docs/openapi.json` 生成的 TypeScript 客户端 |
| `skills` | `bmo-cache-siren` 技能,即 harness 插件的说明 |
| `web` | 守护进程提供的实时页面和演示页面 |
| `macos` | Swift 悬浮窗应用及其构建脚本 |
| `deploy` | systemd 用户单元和 launchd 文件 |
| `docs` | API、其 OpenAPI 文档、配置说明和移植指南 |

## 文档

- `PERSONAL-DATA.md`:任何文件都不得包含的个人数据、允许出现的值,以及守住这条规则的检查(测试平台的
  `personal-data` 检查)。
- `docs/api.md`:所有 HTTP 路由、事件、客户端、命令和 harness 插件。
- `docs/openapi.json`:同一 API 的 OpenAPI 3.1 契约,由代码生成。
- `docs/configuration.md`:每个配置键、其默认值以及何时生效。
- `docs/porting.md`:哪些部分与操作系统无关,哪些仅限 macOS,哪些依赖外部服务,以及如何在 Linux 上运行。
