[English](README.md) | **한국어** | [日本語](README.ja.md) | [简体中文](README.zh-CN.md)

# bmo-cache-siren

모델 요청마다 프롬프트 캐시 적중과 새 토큰을 실시간으로 지켜보다가, 새 토큰이 급증하면 사이렌을 울립니다.
자체 로깅을 추가하지 않고 Claude Code 트랜스크립트를 따라 읽으며, 트랜스크립트를 남기지 않는 러너가 루프백으로
보고하는 사용량도 받습니다. 각 요청을 분류하고, 캐시 미스의 유력한 원인을 짚고, 한 요청이나 짧은 시간에 몰린
요청들이 큰 컨텍스트를 다시 프리필하면 경보를 냅니다. 하네스는 이를 MCP 서버(`bmo-cache-siren mcp`, 그리고
`bmo-cache-siren` 스킬)로 붙여 자기 캐시가 얼마나 잘 유지되는지 읽습니다. 이 서비스는 다른 서비스를 호출하지
않습니다.

콜드 재프리필은 놓치기 쉽고 비쌉니다. 세션 몇 개가 한꺼번에 콜드로 다시 시작하면 1분 만에 수백만 토큰이 프리필을
거쳐 가고, 누군가 알아채기 전에 5시간 할당량의 상당 부분을 써 버릴 수 있습니다. 사이렌은 그 순간을 바로
보이게 합니다.

## 설치와 실행

핵심은 Cargo로 빌드하는 Rust 바이너리 하나이며, 어느 OS에서나 실행됩니다.

BMO 머신에서는 에이전트가 인덱스의 INSTALL.md(`bmo install`, https://bmo.ahas.io/install.md)로 데몬을 설치합니다.

데몬은 `127.0.0.1:9022`에서 대기합니다. 처음 실행하면 모든 기본값과 무작위 API 토큰을 담은 `config.json`을
데이터 디렉터리에 씁니다. 각 키와 `serve`의 각 플래그는 `docs/configuration.md`에 정리되어 있습니다. 브라우저
페이지가 실시간 표이며, `page`가 출력하는 주소(토큰이 들어 있습니다)로 엽니다. 같은 바이너리의 다른 동사는 모두
클라이언트를 통해 실행 중인 데몬과 통신합니다.

플랫폼별 구성은 다음과 같습니다.

- **macOS:** Swift 오버레이 앱(`macos/build.sh`가 `dist/CacheSiren.app`을 빌드합니다), 사이렌 재생에
  `afplay`, 자동 시작에 launchd(`deploy/launchd/`)를 씁니다.
- **Linux:** 항상 위에 떠 있는 브라우저 창이 오버레이를 대신하고, 사이렌은 호스트에 있는 `pw-play`, `paplay`,
  `aplay` 중 하나로 재생하며, systemd --user가 데몬을 시작합니다(`deploy/systemd/cache-siren.service`).
  나머지는 `docs/porting.md`에서 다룹니다.

## 종류와 경보

| 종류 | 의미 | 수준 |
| --- | --- | --- |
| `warm` | 새 토큰 20K 미만 | info |
| `growth` | 캐시 읽기가 0보다 크고 새 토큰이 20K 이상 | yellow, 100K 이상이면 red |
| `start` | 세션의 첫 요청 | info, 새 토큰 20K 이상이면 yellow |
| `cold` | 재프리필: 캐시에서 거의 또는 전혀 읽지 않았고 새 토큰 20K 이상 | yellow, 100K 이상이면 red |
| `refusal` | 요청이 거부됨 | red |
| `quota`, `error`, `compact` | 할당량 중단, API 오류, 압축 | yellow |
| `cmd` | 로컬 명령 | info |
| `notice` | 안내용 줄 | info |
| `request` | 사용량을 알 수 없는 게시된 요청(`usage_unavailable` 표시), 또는 세션의 첫 요청 이후 캐시에서 아무것도 읽지 않고 새 토큰이 20K 미만에 머문 게시된 요청 | info |
| `burst` | 모든 세션을 합쳐 60초 안의 새 토큰이 500K에 도달 | red |

red 요청이나 burst는 사이렌을 울리며, 세션마다 쿨다운당 한 번만 울립니다. 로컬 모델이 처리한 요청은 비용이
들지 않으므로 yellow를 넘지 않습니다.

cold 요청에는 유력한 원인이 붙습니다. `session_start`, `model_switch`, `effort_model`, `compact`,
`ttl_expired`(간격이 해당 모드와 모델의 캐시 수명을 넘김), `prefix_changed`(직전 컨텍스트의 절반 미만만 다시
읽음), `tool_search`, `image`, `unknown` 중 하나입니다.

임계값은 실제 세션의 큰 표본에서 얻은 요청별 새 토큰 분포에서 나왔습니다. 중앙값은 약 2K, p90은 약 8K, p95는
약 12K, p99는 약 170K입니다. 20K에 이르는 요청은 약 3%, 100K에 이르는 요청은 2% 미만입니다. 임계값은 설정
키이므로 자신의 분포에 맞게 다시 조정하십시오.

트랜스크립트를 남기지 않는 러너는 사용량을 `POST /events`로 게시합니다. 요청 본문, 나머지 모든 경로, 이벤트,
클라이언트, 하네스 플러그인은 `docs/api.md`에 있습니다.

## 사용법

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

동사들은 `http://127.0.0.1:9022`, 또는 `CACHE_SIREN_ADDRESS`가 지정한 주소의 데몬에 데이터 디렉터리의 토큰
파일에 든 토큰으로 요청합니다. `--json`을 주면 API의 응답을 그대로 출력합니다.

오버레이는 아무 곳이나 잡고 끌어 옮기고, 오른쪽 아래 모서리로 크기를 바꾸며, 행을 클릭하면 상세 내용이 열립니다.
헤더의 `—`를 누르면 다음 `bmo-cache-siren restart`까지 숨겨집니다. `local` 버튼은 로컬 모델의 행을 보이거나
빼며, 브라우저가 그 선택을 기억합니다.

웹 콘솔의 캐시 사이렌 화면은 클라이언트와 똑같이 이 API를 읽습니다. 세션별 합계와 사이렌 횟수는 `GET /stats`에서,
스위치와 임계값은 `GET /settings`에서, 최신 이벤트는 스트림이 아닌 JSON 페이지로 `GET /events`에서 가져옵니다.
서비스가 보관하지 않는 할당량과 읽은 파일 수는 보여 주지 않습니다. 무엇을 읽고 무엇을 보여 주는지는
`docs/api.md`에 적혀 있습니다.

## 운영

- `~/.claude/cache-siren/cache-siren.log`의 로그에는 INFO, WARN, ERROR 줄이 남으며, 접근 로그는 꺼져
  있습니다. 데몬은 10분마다 생존 줄을 씁니다.
- 데몬이 멈추면 오버레이는 3초마다 다시 시도하고, 페이지는 2초마다 이벤트를 다시 읽고 스트림에 다시 연결합니다.
  서비스를 다시 시작하고 `lsof -nP -iTCP:9022`로 포트를 확인하십시오.
- `~/.claude/cache-siren/events.jsonl`의 이벤트 로그는 `retention_days`에 맞춰 정리됩니다. 시작할 때 가장
  최근 이벤트가 창으로 복원되고, 번호는 이어서 매겨집니다.

## 알려진 한계

- 트랜스크립트는 응답 뒤에 기록되므로, 행은 요청 후 2~5초 뒤에 나타납니다. 이것은 요청 앞에 서는 프록시가
  아니라 감시 도구이므로 요청을 막을 수 없습니다.
- 캐시 읽기를 0으로 보고하는 프록시 뒤의 로컬 모델은 모든 요청이 cold로 표시됩니다.

## 서브에이전트 팬아웃과 5시간 할당량

사이렌이 잡아내는 가장 비싼 패턴은 폭넓은 조사 중의 재귀적 서브에이전트 팬아웃입니다. 측정된 한 사례에서는
세션 하나가 8분 동안 서브에이전트 35개를 띄우고 웹 검색을 368번 실행해 약 27M 토큰, 즉 5시간 할당량의 30%
가량을 썼습니다.

측정에서 드러난 점은 다음과 같습니다.

- 수백 개 세션에 걸쳐, 일반적인 조사는 p90~p95에서 서브에이전트 3~7개와 수십 번의 검색을 썼습니다. 이상치는
  서브에이전트 26~40개와 백 번을 훌쩍 넘는 검색을 썼습니다. 완만한 꼬리가 아니라 열 배의 절벽입니다.
- 비싼 세션에서 토큰의 약 89%는 누적된 검색 결과였고, 서브에이전트의 첫 프리필이 약 6%, 메인 세션의 취합이
  약 6%였습니다.
- 368번의 검색 중 367번이 서로 달랐습니다. 검색은 낭비가 아니라 실제 범위였으므로, 검색에 상한을 두면 범위가
  줄어듭니다.

비용은 재귀와, 많은 서브에이전트가 동시에 프리필하는 데서 나옵니다. 검색이 아니라 이 둘을 제한하십시오. 셸
환경이나 `.claude/settings.json`의 `env` 블록에서 다음과 같이 설정합니다.

```
CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH=1      # no agent spawns agents: removes the multiplication
CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS=6      # damps concurrent prefill spikes, at a little speed
CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION=20    # a safety net
```

`CLAUDE_CODE_MAX_WEB_SEARCHES_PER_SESSION`은 기본값으로 두십시오. 도구를 검색으로만 제한한 가벼운 조사
서브에이전트는 첫 프리필을 4분의 1 정도로 줄이지만, 깊은 조사에서 전체 절감은 5% 정도에 그칩니다.

## 폴더

| 폴더 | 내용 |
| --- | --- |
| `src` | 클라이언트와 MCP 서버를 포함한 `cache_siren` 라이브러리, 그리고 `bmo-cache-siren` 바이너리 |
| `tests` | 모든 테스트와, 경로 테스트가 띄우는 프로세스 내 서버 |
| `package` | `docs/openapi.json`에서 생성한 TypeScript 클라이언트 |
| `skills` | 하네스 플러그인의 지침인 `bmo-cache-siren` 스킬 |
| `web` | 데몬이 제공하는 실시간 페이지와 데모 페이지 |
| `macos` | Swift 오버레이 앱과 빌드 스크립트 |
| `deploy` | systemd 사용자 유닛과 launchd 파일 |
| `docs` | API, OpenAPI 문서, 설정, 포팅 안내 |

## 문서

- `PERSONAL-DATA.md`: 어떤 파일에도 들어가면 안 되는 개인 정보, 허용되는 값, 그리고 이 규칙을 지키는 검사(테스트베드의
  `personal-data` 검사).
- `docs/api.md`: 모든 HTTP 경로, 이벤트, 클라이언트, 명령, 하네스 플러그인.
- `docs/openapi.json`: 같은 API를 코드에서 생성한 OpenAPI 3.1 계약.
- `docs/configuration.md`: 모든 설정 키, 기본값, 적용 시점.
- `docs/porting.md`: OS와 무관한 부분, macOS 전용 부분, 외부 서비스에 의존하는 부분, 그리고 Linux에서 실행하는 방법.
