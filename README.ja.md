[English](README.md) | [한국어](README.ko.md) | **日本語** | [简体中文](README.zh-CN.md)

# bmo-cache-siren

モデルへのリクエストごとにプロンプトキャッシュのヒットと新規トークンをリアルタイムで監視し、新規トークンが急増すると
サイレンを鳴らします。独自のログを追加することなく Claude Code のトランスクリプトを追いかけ、トランスクリプトを
残さないランナーがループバック経由で報告する使用量も受け取ります。各リクエストを分類し、キャッシュミスの原因として
考えられるものを示し、1 件のリクエストや短時間に集中したリクエスト群が大きなコンテキストを再プリフィルしたときに
警告を出します。ハーネスはこれを MCP サーバー(`bmo-cache-siren mcp` と `bmo-cache-siren` スキル)として組み込み、
自分のキャッシュがどれだけ保たれているかを読み取ります。このサービス自体は他のサービスを呼び出しません。

コールドな再プリフィルは見逃しやすく、しかも高くつきます。いくつかのセッションが一斉にコールドで再開すると、1 分間で
数百万トークンがプリフィルを通過し、誰かが気づく前に 5 時間クォータのかなりの部分を使い切ってしまうことがあります。
サイレンはそれを起きたその場で見えるようにします。

## インストールと実行

中核は Cargo でビルドする Rust バイナリ 1 つで、どの OS でも動作します。

```
cargo build --release
target/release/bmo-cache-siren serve &
xdg-open "$(target/release/bmo-cache-siren page)"      # macOS: open
target/release/bmo-cache-siren status
```

デーモンは `127.0.0.1:9022` で待ち受けます。初回起動時に、すべてのデフォルト値とランダムな API トークンを含む
`config.json` をデータディレクトリに書き出します。各キーと `serve` の各フラグは `docs/configuration.md` に
まとめています。ブラウザのページがライブの表で、`page` が出力するアドレス(トークン付き)で開きます。同じバイナリの
ほかの動詞はすべて、クライアントを通じて実行中のデーモンとやり取りします。

プラットフォームごとの構成は次のとおりです。

- **macOS:** Swift のオーバーレイアプリ(`macos/build.sh` が `dist/CacheSiren.app` をビルドします)、サイレンの
  再生に `afplay`、自動起動に launchd(`deploy/launchd/`)を使います。
- **Linux:** 常に最前面に表示されるブラウザウィンドウがオーバーレイの代わりとなり、サイレンはホストにある
  `pw-play`、`paplay`、`aplay` のいずれかで再生し、systemd --user がデーモンを起動します
  (`deploy/systemd/cache-siren.service`)。残りは `docs/porting.md` で扱います。

## 種別とアラート

| 種別 | 意味 | レベル |
| --- | --- | --- |
| `warm` | 新規トークンが 20K 未満 | info |
| `growth` | キャッシュ読み取りが 0 より大きく、新規トークンが 20K 以上 | yellow、100K 以上で red |
| `start` | セッションの最初のリクエスト | info、新規 20K 以上で yellow |
| `cold` | 再プリフィル:キャッシュからほとんど、またはまったく読まず、新規 20K 以上 | yellow、100K 以上で red |
| `refusal` | リクエストが拒否された | red |
| `quota`、`error`、`compact` | クォータによる停止、API エラー、コンパクション | yellow |
| `cmd` | ローカルコマンド | info |
| `notice` | 情報表示の行 | info |
| `request` | 使用量が不明な投稿リクエスト(`usage_unavailable` と注記)、またはセッションの 2 件目以降でキャッシュから何も読まず新規 20K 未満にとどまった投稿リクエスト | info |
| `burst` | 全セッション合計で 60 秒以内の新規トークンが 500K に達した | red |

red のリクエストや burst はサイレンを鳴らします。鳴るのはセッションごとにクールダウンあたり 1 回です。ローカル
モデルが処理したリクエストは課金されないため、yellow を超えることはありません。

cold のリクエストには考えられる原因が付きます。`session_start`、`model_switch`、`effort_model`、`compact`、
`ttl_expired`(間隔がそのモードとモデルのキャッシュ寿命を超えた)、`prefix_changed`(直前のコンテキストの半分
未満しか読み戻せなかった)、`tool_search`、`image`、`unknown` のいずれかです。

しきい値は、実際のセッションの大きなサンプルから得たリクエストごとの新規トークン分布に基づいています。中央値は約
2K、p90 は約 8K、p95 は約 12K、p99 は約 170K です。20K に達するリクエストは約 3%、100K に達するものは 2% 未満
です。しきい値は設定キーなので、自分の分布に合わせて調整し直してください。

トランスクリプトを残さないランナーは、使用量を `POST /events` に投稿します。リクエストボディ、ほかのすべての
ルート、イベント、クライアント、ハーネスプラグインは `docs/api.md` に記載しています。

## 使い方

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

各動詞は `http://127.0.0.1:9022`、または `CACHE_SIREN_ADDRESS` で指定したアドレスのデーモンに、データ
ディレクトリのトークンファイルにあるトークンを付けて問い合わせます。`--json` を付けると API の応答をそのまま出力します。

オーバーレイはどこをつかんでもドラッグでき、右下の隅でサイズを変えられ、行をクリックすると詳細が開きます。ヘッダーの
`—` を押すと、次の `bmo-cache-siren restart` まで非表示になります。`local` ボタンはローカルモデルの行の表示と
非表示を切り替え、その選択はブラウザが覚えています。

Web コンソールのキャッシュサイレン画面は、クライアントと同じようにこの API を読みます。セッションごとの合計と
サイレンの回数は `GET /stats` から、スイッチとしきい値は `GET /settings` から、最新のイベントはストリームではなく
JSON のページとして `GET /events` から取得します。サービスが保持していないクォータと読み込んだファイル数は表示
しません。何を読み、何を表示するかは `docs/api.md` に記載しています。

## 運用

- `~/.claude/cache-siren/cache-siren.log` のログには INFO、WARN、ERROR の行が残り、アクセスログはオフです。
  デーモンは 10 分ごとに生存を示す行を書きます。
- デーモンが止まると、オーバーレイは 3 秒ごとに再試行し、ページは 2 秒ごとにイベントを読み直してストリームに再接続
  します。サービスを再起動し、`lsof -nP -iTCP:9022` でポートを確認してください。
- `~/.claude/cache-siren/events.jsonl` のイベントログは `retention_days` に従って整理されます。起動時に最新の
  イベントがウィンドウに復元され、番号はそのまま続きから振られます。

## 既知の制限

- トランスクリプトは応答の後に書き込まれるため、行はリクエストから 2〜5 秒後に表示されます。これはリクエストの前に
  立つプロキシではなく監視ツールなので、リクエストを止めることはできません。
- キャッシュ読み取りを 0 と報告するプロキシの背後にあるローカルモデルでは、すべてのリクエストが cold と表示されます。

## サブエージェントのファンアウトと 5 時間クォータ

サイレンが捉える最も高くつくパターンは、広範な調査中の再帰的なサブエージェントのファンアウトです。計測したある
事例では、1 つのセッションが 8 分間にサブエージェントを 35 個起動し、Web 検索を 368 回実行して約 27M トークン、
つまり 5 時間クォータのおよそ 30% を消費しました。

計測からわかったことは次のとおりです。

- 数百のセッション全体で、通常の調査は p90〜p95 でサブエージェント 3〜7 個、検索数十回でした。外れ値は
  サブエージェント 26〜40 個、検索は 100 回を大きく超えていました。なだらかな裾ではなく、10 倍の崖です。
- 高くついたセッションでは、トークンの約 89% が蓄積された検索結果、サブエージェントの最初のプリフィルが約 6%、
  メインセッションでの集約が約 6% でした。
- 368 回の検索のうち 367 回は重複していませんでした。検索は無駄ではなく実際のカバレッジだったので、検索に上限を
  設けるとカバレッジが削られます。

コストは再帰と、多数のサブエージェントが同時にプリフィルすることから生じます。制限すべきは検索ではなくこの 2 つです。
シェル環境、または `.claude/settings.json` の `env` ブロックで次のように設定します。

```
CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH=1      # no agent spawns agents: removes the multiplication
CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS=6      # damps concurrent prefill spikes, at a little speed
CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION=20    # a safety net
```

`CLAUDE_CODE_MAX_WEB_SEARCHES_PER_SESSION` はデフォルトのままにしてください。ツールを検索だけに絞った軽量な調査
サブエージェントは最初のプリフィルを約 4 分の 1 に減らしますが、深い調査では全体の節約は 5% 程度にとどまります。

## フォルダー

| フォルダー | 内容 |
| --- | --- |
| `src` | クライアントと MCP サーバーを含む `cache_siren` ライブラリと、`bmo-cache-siren` バイナリ |
| `tests` | すべてのテストと、ルートのテストが起動するプロセス内サーバー |
| `package` | `docs/openapi.json` から生成した TypeScript クライアント |
| `skills` | ハーネスプラグインの指示である `bmo-cache-siren` スキル |
| `web` | デーモンが配信するライブページとデモページ |
| `macos` | Swift のオーバーレイアプリとそのビルドスクリプト |
| `deploy` | systemd のユーザーユニットと launchd のファイル |
| `docs` | API、その OpenAPI ドキュメント、設定、移植ガイド |

## ドキュメント

- `PERSONAL-DATA.md`:どのファイルにも含めてはならない個人データ、許可される値、そしてこのルールを守る検査
  (テストベッドの `personal-data` 検査)。
- `docs/api.md`:すべての HTTP ルート、イベント、クライアント、コマンド、ハーネスプラグイン。
- `docs/openapi.json`:同じ API を、コードから生成した OpenAPI 3.1 の契約として記述したもの。
- `docs/configuration.md`:すべての設定キー、そのデフォルト値、反映されるタイミング。
- `docs/porting.md`:OS に依存しない部分、macOS 専用の部分、外部サービスに依存する部分、そして Linux での動かし方。

## クレジット

このプロジェクトは its original author による cache-sirenから派生した
ものです。ここで説明しているデーモン、オーバーレイ、分類、ファンアウトの計測は、元のプロジェクトが設計し、構築
しました。Rust のデーモンはその設計を受け継いでいます。`NOTICE` を参照してください。
