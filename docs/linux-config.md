# Linux 版 takusu の設定（TOML）

この文書は、Linux 上で動作する `takusu-cli`、`takusu-agent`、`takusu-desktop`、`takusu-web`、`takusu-local` 関連の設定ファイルをまとめたリファレンスです。

## 設定ファイルの置き場

各コンポーネントは XDG Base Directory 仕様に従い、設定ファイルを `~/.config/takusu/` 以下に探します。

- `~/.config/takusu/config.toml`
  - `takusu-cli`、`takusu-web`、`takusu-desktop` が共有で読みます。
  - `takusu-local` 単体プロセスは **このファイルを読みません**。
    環境変数で設定します。
- `~/.config/takusu/agent.toml`
  - `takusu-agent`（CLI の `takusu agent`、デスクトップのエージェント、Android ネイティブ層）が読みます。

`XDG_CONFIG_HOME` が設定されている場合は、そちらを優先します。

## コンポーネントの関係

Linux 版のバイナリは、ライブラリとして `takusu-local-lib` と `takusu-agent` を共有しています。
原則として、別途 `takusu-local` サーバーを起動しておく必要はありません。

- `takusu-cli`
  - スケジュール操作では `takusu-local-lib` の `TakusuApp` を直接使います。
  - `takusu agent` 実行時は、`takusu-local` のルーターをプロセス内で起動し、その上で `takusu-agent` を動かします。
- `takusu-desktop`
  - 常駐デーモンとして、`takusu-local` と `takusu-agent` をプロセス内に埋め込んで動作します。
  - `desktop.local_url` が空の場合、起動時に内部の `takusu-local` を自動で立ち上げます。
- `takusu-local`
  - サーバーとして単独で動かすためのエントリポイントです。
  - 現状では環境変数のみで設定し、`config.toml` は読みません。

## 共有設定 `~/.config/takusu/config.toml`

以下のキーは `takusu-cli` と `takusu-web` の両方で解釈されます。
`[desktop]` テーブルは `takusu-desktop` 専用です。

```toml
# ストレージバックエンド: "sqlite" または "workers"
storage = "sqlite"

# SQLite 接続文字列
# workers 使用時は空でも構いません
db = "sqlite:./takusu.db"

# Cloudflare Worker バックエンドの URL
worker_url = ""

# Worker / ローカル API 用のトークン
workers_token = ""
root_token = ""

# SQLite JWT 署名用シークレット
jwt_secret = ""

# タイムゾーン
tz = "Asia/Tokyo"

# 睡眠時間帯（プランナーがスケジュールを入れない時間）
sleep_start = "22:00"
sleep_end = "06:00"

# takusu-web のみが読むキー
bind = "127.0.0.1:3000"

[desktop]
# テーマ: "light", "dark", "catppuccin", "aura-soft-dark"
theme = "light"

# 既存の takusu-local サーバ URL
# 空の場合、デスクトップが内部に takusu-local を起動します
local_url = ""

# ローカル API 用 Bearer トークン
# 空の場合は TAKUSU_TOKEN または TAKUSU_TOKEN_FILE から読みます
token = ""
```

### 主要な共有キーの既定値

| キー | 既定値 | 備考 |
|------|--------|------|
| `storage` | `sqlite` | `workers` にするとクラウド Worker を使います |
| `db` | `sqlite:./takusu.db` | CLI 実行時の相対パスになります |
| `worker_url` | 空 | `workers` 使用時に必要 |
| `workers_token` | 空 | `workers` 使用時に必要 |
| `root_token` | 空 | ローカル root API 用 |
| `jwt_secret` | 空 | SQLite 使用時に必要 |
| `tz` | `UTC` | タイムゾーン識別子 |

### エイリアス

`takusu config set` では以下のエイリアスが使えます。

- `worker_url` = `url`
- `workers_token` = `token`

## エージェント設定 `~/.config/takusu/agent.toml`

`AgentConfig` は `[llm]`、`[server]`、`[audio]` の 3 つのテーブルから構成されます。

```toml
[llm]
base_url = "https://api.openai.com/v1"
model = "gpt-4.1-mini"
api_key_env = "TAKUSU_LLM_API_KEY"
api_key = ""
provider = "openai_compatible"
max_history = 64
max_context_tokens = 32000
max_tool_calls = 64
request_timeout = 60

[llm.compaction]
enabled = true
reserve_tokens = 4096
keep_recent_tokens = 12000

[llm.permissions]
"task:create" = true
"schedule:generate" = true

[server]
url = "http://127.0.0.1:3000"
token = ""

[audio.stt]
backend = "sherpa"
language = "ja"
model_dir = ""
model = "sense-voice"
use_itn = true
num_threads = 2
provider = "cpu"
sample_rate = 16000

[audio.tts]
backend = "cartesia"
api_key_env = "CARTESIA_API_KEY"
api_key = ""
voice_id = "db6b0ed5-d5d3-463d-ae85-518a07d3c2b4"
language = "ja"
sample_rate = 44100
model = ""
speed = 1.0
mute = false

[audio.barge_in]
enabled = false
use_aec = true
warm_up_ms = 300
reference_delay_ms = 0
tap_to_stop = true
record_latency = true

[audio.aec]
filter_len = 1600
step_size = 0.08
delta = 0.0000000001
warm_up_frames = 16
reference_floor = 0.001

[audio.vad]
energy_threshold = 0.02

[audio.speaker]
model_id = "sherpa-speaker-campplus-zh-en"
num_threads = 1
provider = "cpu"
verify_threshold = 0.5
# voice_dir = "/home/user/.local/share/takusu/voiceprint"
```

### `[llm]`

| キー | 既定値 | 説明 |
|------|--------|------|
| `base_url` | `https://api.openai.com/v1` | OpenAI 互換エンドポイント |
| `model` | `gpt-4.1-mini` | 使用するモデル ID |
| `api_key_env` | `TAKUSU_LLM_API_KEY` | API キーを読む環境変数名 |
| `api_key` | 空 | 直接 API キーを書く場合。`api_key_env` より優先します |
| `provider` | `openai_compatible` | `openai`・`openrouter`・`custom` は同じ意味のエイリアスです |
| `max_history` | `64` | 保持する会話ターン数 |
| `max_context_tokens` | `32000` | コンテキスト上限（要約を含む） |
| `max_tool_calls` | `64` | 1 ターンあたりの最大 tool call 回数 |
| `request_timeout` | `60` | 秒単位。`request_timeout_seconds` も受け付けます |

`[llm.compaction]` は会話が長くなったときに古いターンを要約して削減します。

### `[server]`

ローカル API（`takusu-local`）への接続先です。
`token` は `takusu system gen-root-token` などで発行したものを使います。

### `[llm.permissions]`

エージェントが提案する変更を自動承認するかどうかを `target:operation` 形式で指定します。
`*` をワイルドカードに使えます。

```toml
[llm.permissions]
"*:*" = true
"task:delete" = false
```

対象（target）の例: `task`, `habit`, `skill`, `memory`, `schedule`, `comment`, `coverage`

操作（operation）の例: `create`, `update`, `delete`, `generate`, `reschedule`, `move`, `start`, `pause`, `snooze`, `progress`, `complete`, `split`, `undo`, `create_scheduled_span`, `delete_scheduled_span`, `settle`, `confirm`

### `[audio]`

#### STT

| キー | 既定値 | 選択肢 |
|------|--------|--------|
| `backend` | `sherpa` | `sherpa` |
| `model` | `sense-voice` | `sense-voice`, `funasr-nano`, `parakeet-ctc-ja`, `nemotron-ja` |
| `provider` | `cpu` | `cpu`, `cuda`, `coreml` |

#### TTS

| キー | 既定値 | 選択肢 |
|------|--------|--------|
| `backend` | `cartesia` | `cartesia`, `android`, `fish` |
| `api_key_env` | `CARTESIA_API_KEY` | 環境変数名 |
| `voice_id` | `db6b0ed5-d5d3-463d-ae85-518a07d3c2b4` | サービス依存 |
| `sample_rate` | `44100` | Hz |

#### 音声承認（Voice Approval）

音声対話では、操作の重要度に応じて 4 段階の承認レイヤーが使われます。
`AmbientImmediate` と `VoiceConfirmed` の操作は、話者確認（speaker verification）を通過する必要があります。

- `Immediate`: 画面や通知経由の安全な操作、または声紋確認済みの継続セッション
- `AmbientImmediate`: ウェイクワードによる `start` / `pause`（話者確認必須）
- `VoiceConfirmed`: 変更内容の読み上げ＋ユーザーが「はい」と応答＋話者確認
- `ScreenRequired`: 削除、スケジュール全体の変更など、画面承認が必要な操作

話者確認を有効にするには `[audio.speaker]` を設定し、あらかじめ `takusu speaker enroll` で声紋を登録しておきます。

```toml
[audio.speaker]
model_id = "sherpa-speaker-campplus-zh-en"
num_threads = 1
provider = "cpu"
verify_threshold = 0.5
voice_dir = "/home/user/.local/share/takusu/voiceprint"
```

| キー | 既定値 | 説明 |
|------|--------|------|
| `model_id` | `sherpa-speaker-campplus-zh-en` | 話者埋め込みモデル ID |
| `num_threads` | `1` | ONNX 推論スレッド数 |
| `provider` | `cpu` | `cpu`, `cuda`, `coreml` |
| `verify_threshold` | `0.5` | コサイン類似度の判定閾値 |
| `voice_dir` | 未設定 | 声紋データの保存先。未設定時は `~/.local/share/takusu/voiceprint` |

声紋の登録・確認・削除は `takusu speaker` サブコマンドで行います（近日 merge 予定）。

```bash
# 声紋を登録（複数 WAV を平均化）
takusu speaker enroll --name default sample1.wav sample2.wav

# 声紋を確認
takusu speaker verify --name default unknown.wav

# 登録済み声紋を一覧 / 削除
takusu speaker list
takusu speaker delete --name default
```

#### AEC / VAD

これらは音声対話の品質に関わる上級設定です。
通常は既定値のままで構いません。

## 環境変数による上書き

### 共有設定

以下は `takusu-cli`、`takusu-local`（単体）、`takusu-web`、`takusu-desktop` などで解釈される環境変数です。

- `TAKUSU_STORAGE`: `sqlite` または `workers`
- `TAKUSU_DB`: SQLite 接続文字列
- `TAKUSU_BIND`: サーバの bind アドレス
- `TAKUSU_WORKERS_URL` または `TAKUSU_WORKER_URL`: Worker URL
- `TAKUSU_JWT_SECRET`: JWT 署名用シークレット
- `TAKUSU_JWT_SECRET_FILE`: シークレットを読むファイルパス
- `TAKUSU_ROOT_TOKEN`: root トークン
- `TAKUSU_WORKERS_TOKEN`: Worker / ローカル用トークン
- `TAKUSU_WORKERS_TOKEN_FILE`: トークンを読むファイルパス
- `TAKUSU_TOKEN`: `takusu-desktop` が使う Bearer トークン
- `TAKUSU_TOKEN_FILE`: 同上、ファイルから読み込み
- `TAKUSU_TIMEZONE`: `takusu-cli` の `--tz` と同じ
- `TAKUSU_MODEL_CACHE_DIR`: Sherpa-ONNX / 話者埋め込みモデルのキャッシュ先。未設定時は `~/.cache/takusu/models`
- `XDG_CACHE_HOME`, `XDG_DATA_HOME`, `XDG_CONFIG_HOME`: モデルキャッシュ、声紋データ、設定ファイルの基準ディレクトリ

### エージェント設定

`TAKUSU_AGENT__<セクション>__<キー>` 形式で上書きできます。

```bash
export TAKUSU_AGENT__LLM__BASE_URL="https://api.openai.com/v1"
export TAKUSU_AGENT__LLM__MODEL="gpt-4.1-mini"
export TAKUSU_AGENT__LLM__API_KEY_ENV="OPENAI_API_KEY"
```

### デスクトップ設定

- `TAKUSU_DESKTOP_THEME`: `light`, `dark`, `catppuccin`, `aura-soft-dark`
- `TAKUSU_DESKTOP_LOCAL_URL`: 既存の `takusu-local` URL

## CLI からの設定操作

### `~/.config/takusu/config.toml`

```bash
# ファイルの中身を表示
takusu config show

# テンプレートを生成
takusu config init

# 値を設定
takusu config set storage sqlite
takusu config set db "sqlite:/home/user/.local/state/takusu/takusu.db"
takusu config set tz Asia/Tokyo
```

`set` で使えるキーは `storage`, `db`, `worker_url`（`url` でも可）, `workers_token`（`token` でも可）, `root_token`, `jwt_secret`, `tz`, `sleep_start`, `sleep_end` です。

### `~/.config/takusu/agent.toml`

```bash
# 有効な設定を表示（デフォルト含む）
takusu agent config show

# 値を設定（ドット区切り）
takusu agent config set llm.base_url "https://api.openai.com/v1"
takusu agent config set llm.model "gpt-4.1-mini"
takusu agent config set audio.stt.model sense-voice

# パーミッションの管理
takusu agent allow "task:create"
takusu agent deny "*:*"

# プロバイダーのモデル一覧を取得
takusu agent models
```

`llm.permissions` 配下は `takusu agent config set llm.permissions...` では操作できません。
必ず `takusu agent allow` / `takusu agent deny` を使ってください。

### 声紋（Speaker）

```bash
# 声紋を登録（複数 WAV を平均化）
takusu speaker enroll --name default sample1.wav sample2.wav

# 声紋を確認
takusu speaker verify --name default unknown.wav

# 登録済み声紋を一覧 / 削除
takusu speaker list
takusu speaker delete --name default
```

`--model_dir` と `--voice_dir` で、それぞれ話者埋め込みモデルと声紋データの保存先を変更できます。
未指定時は `~/.cache/takusu/models` と `~/.local/share/takusu/voiceprint` が使われます。

`--provider` には `cpu`, `cuda`, `coreml` が指定できます。

## 最小構成の例

### SQLite + OpenAI 互換プロバイダー

```toml
# ~/.config/takusu/config.toml
storage = "sqlite"
db = "sqlite:/home/user/.local/state/takusu/takusu.db"
jwt_secret = "<openssl rand -hex 32 で生成>"
tz = "Asia/Tokyo"
sleep_start = "22:00"
sleep_end = "06:00"
```

```toml
# ~/.config/takusu/agent.toml
[llm]
base_url = "https://api.openai.com/v1"
model = "gpt-4.1-mini"
provider = "openai_compatible"

[server]
url = "http://127.0.0.1:3000"
```

```bash
export TAKUSU_LLM_API_KEY="sk-..."
takusu agent models
takusu agent
```

### Cloudflare Worker バックエンド

```toml
# ~/.config/takusu/config.toml
storage = "workers"
worker_url = "https://your-worker.example.com"
workers_token = "<your-worker-token>"
```

## 秘匿情報の扱い

`~/.config/takusu/config.toml` や `~/.config/takusu/agent.toml` に API キーなどを直接書くことは可能ですが、バージョン管理に紛れ込まないよう注意してください。

推奨される方法は以下のいずれかです。

- 環境変数（`TAKUSU_LLM_API_KEY`, `CARTESIA_API_KEY` など）を使う。
- Nix/Home Manager では `services.takusu-desktop` の `jwtSecretFile` / `tokenFile` を使う。
- `takusu-desktop` では `TAKUSU_TOKEN_FILE` や `TAKUSU_JWT_SECRET_FILE` でファイルから読み込む。

`~/.config/takusu` 以下のファイルパーミッションは `600` 以上に制限することを推奨します。
