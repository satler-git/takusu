# REST API 概要

`takusu-local` は **axum**（Rust の Web フレームワーク）で実装された REST API サーバーです。
`/api/*` エンドポイントは `Authorization: Bearer <token>` を必要とします。
`/health` は例外です。

`takusu-worker`（Cloudflare Worker）では一部エンドポイントが異なります。詳細は後述の Worker との差異を参照してください。

## 認証

```http
Authorization: Bearer <token>
```

トークンは `tokens` テーブルに SHA-256 ハッシュで保存されます。
ルートトークン `TAKUSU_ROOT_TOKEN` は、`gen-root-token` で生成した JWT です。
すべての操作が可能です。

## エンドポイント

### Task

```
GET    /api/tasks
GET    /api/tasks/:id
PUT    /api/tasks/:id
PATCH  /api/tasks/:id
DELETE /api/tasks/:id
POST   /api/tasks
GET    /api/tasks/complete
GET    /api/tasks/similar
POST   /api/tasks/import/ical
GET    /api/tasks/dependency-analysis
POST   /api/tasks/:id/work/start
POST   /api/tasks/:id/work/pause
POST   /api/tasks/:id/work/complete
POST   /api/tasks/:id/progress
GET    /api/tasks/:id/progress
POST   /api/tasks/:id/split
```

### Habit

```
GET    /api/habits
GET    /api/habits/:id
PUT    /api/habits/:id
PATCH  /api/habits/:id
DELETE /api/habits/:id
POST   /api/habits
POST   /api/habits/:id/estimate
GET    /api/habits/scheduled-spans
GET    /api/habits/steps
GET    /api/habits/:id/scheduled-spans
POST   /api/habits/:id/scheduled-spans
DELETE /api/habits/:id/scheduled-spans/:span_id
GET    /api/habits/:id/steps
PUT    /api/habits/:id/steps
GET    /api/habits/:id/steps/dependency-analysis
```

### Schedule

```
GET    /api/schedule
POST   /api/schedule/generate
POST   /api/schedule/preview
POST   /api/schedule/replace
POST   /api/schedule/reschedule
PATCH  /api/schedule/entries/:task_id
DELETE /api/schedule
```

### Settings

```
GET  /api/settings
PUT  /api/settings
PUT  /api/workers/config
GET  /api/workers/health
```

### Token

```
POST   /api/tokens
GET    /api/tokens
DELETE /api/tokens/:id
```

### Sync

```
GET  /api/sync/settings
PUT  /api/sync/settings
POST /api/sync/oauth/url
POST /api/sync/oauth/callback
POST /api/sync/trigger
POST /api/sync/delete-all
GET  /api/sync/mappings
```

### Skills

```
GET    /api/skills
POST   /api/skills
GET    /api/skills/:slug
PATCH  /api/skills/:slug
DELETE /api/skills/:slug
```

### Memory

```
POST   /api/memory
GET    /api/memory/search
GET    /api/memory/:id
PATCH  /api/memory/:id
DELETE /api/memory/:id
```

## Worker との差異

`takusu-worker` では以下のエンドポイントが追加・置き換わります。

```
POST   /api/schedule/save
POST   /api/sync/mappings
DELETE /api/sync/mappings
```

また、`takusu-local` の `/api/schedule/generate` などは worker では `/api/schedule/save` などに対応が変わることがあります。詳細は `crates/takusu-worker/src/router.rs` を参照してください。

## クライアントライブラリ

`takusu-client` クレートは、上記のエンドポイントを Rust から呼び出すためのライブラリです。

```rust
use takusu_client::Client;

let client = Client::new("http://127.0.0.1:3000", "eyJ...");
let tasks = client.list_tasks(None).await?;
```

## 詳細

各エンドポイントのリクエスト型とレスポンス型は、`takusu-storage` クレートの `model.rs` を参照してください。
