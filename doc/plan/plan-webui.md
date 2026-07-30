# takusu WebUI 実装プラン

Parent: https://github.com/takusu-dev/takusu/issues/1077
Closes: https://github.com/takusu-dev/takusu/issues/1030

## Summary

takusu-local-lib を組み込んだ standalone server (`takusu-web`) と、React + Vite のフロントエンド (`web/`) で構成されるデスクトップ向け WebUI。localhost で動かして localhost でアクセスする。mobile と同じ tier1 client。モバイル対応は不要。

## 確認済みの仕様

- **ローカル専用**: 外部に serve しない。localhost で起動して localhost でアクセス
- **tier1 client**: mobile と同等の機能を持つ (agent, progress, stats 含む)
- **キーボードショートカット**: vim 風 (j/k 移動, o 新規, d 削除, etc.)
- **モバイル対応不要**: デスクトップに最適化
- **認証なし**: localhost を信頼。Bearer token 不要
- **設定**: CLI と同じ `~/.config/takusu/config.toml` を共有
- **undo/redo**: タスク CRUD + スケジュール操作 + habit CRUD、50 step (同期は含まない)
- **ブランドカラー**: #7261A3 をアクセントに
- **Google Calendar OAuth**: 設定画面から OAuth フローを完結できる
- **メインビュー**: Week view (日=列、時間=行)、Day/Week 切替
- **タスク詳細**: Overlay Inspector (選択時に右からスライドイン)
- **Command Palette**: Ctrl+K で全アクション + タスク fuzzy search
- **Agent**: Dockable panel (右寄せ floating、リサイズ可)
- **マウス**: ドラッグ&ドロップ reschedule (5分スナップ)、右クリック menu、ホバー quick actions

## mobile との対応表

| 機能 | mobile | WebUI | 備考 |
|------|--------|-------|------|
| タスク CRUD | ○ | ○ | |
| タスク status ライフサイクル | ○ | ○ | start/pause/complete |
| Progress (quantity) トラッキング | ○ | ○ | done/total, +1/-1 |
| タスク分割 (Split) | ○ | ○ | |
| actual_minutes 表示 | ○ | ○ | |
| スケジュール表示 (Timeline) | ○ | ○ | 縦軸=時間 |
| 依存グラフ (DAG) | ○ | ○ | Cytoscape.js + dagre |
| Habit CRUD + RRULE | ○ | ○ | |
| Habit steps | ○ | ○ | |
| Habit scheduled spans | ○ | ○ | |
| Stats (ヒートマップ/バー/予測) | ○ | ○ | |
| Agent チャット (SSE) | ○ | ○ | |
| Agent approval | ○ | ○ | diff 表示 |
| Agent マルチセッション | ○ | ○ | |
| Agent skills 管理 | ○ | ○ | |
| Agent 設定 (LLM/TTS) | ○ | ○ | |
| iCal import | ○ | ○ | ペースト |
| Google Calendar 同期 + OAuth | ○ | ○ | 設定から OAuth |
| undo/redo (50 step) | ○ | ○ | |
| Solver 設定 | ○ | ○ | algorithm/time/seed/warm start |
| Workload 設定 | ○ | ○ | 快適/最大作業時間 |
| Sleep 設定 | ○ | ○ | |
| Calendar overlay (月表示) | ○ | ○ | |
| Parallel task group 表示 | ○ | ○ | |
| テーマ (dark/light 等) | ○ | ○ | |
| 音声入力 (ASR) | ○ | × | mobile 専用 (on-device) |
| TTS 読み上げ | ○ | × | mobile 専用 |
| 通知 (Android) | ○ | × | mobile 専用 |
| ホーム画面ウィジェット | ○ | × | mobile 専用 |
| Haptics | ○ | × | mobile 専用 |

## Architecture

```
Browser (localhost:PORT)
├── React + Vite SPA
│   ├── 3-column layout
│   │   ├── Left: Nav + TaskList (filter/search)
│   │   ├── Center: Timeline / Graph / Habit / Stats / Agent (切替)
│   │   └── Right: Task Detail / Edit / Agent panel
│   ├── vim-style keyboard shortcuts
│   ├── Cytoscape.js + dagre (graph view)
│   ├── shadcn/ui + Tailwind CSS
│   └── WebSocket client (progress + realtime)
└── fetch / ws → localhost:PORT

takusu-web (Rust binary, single process)
├── axum server
│   ├── /api/* — REST API (takusu-local-lib の router を再利用)
│   ├── /api/agent/* — Agent SSE endpoint
│   ├── /ws — WebSocket (生成プログレス + 変更通知)
│   └── /* — 静的ファイル (rust-embed でバイナリに埋め込み)
├── takusu-local-lib (TakusuApp)
│   ├── SqliteStorage (direct sqlx)
│   └── WorkersStorage (HTTP → Cloudflare Worker)
└── config: ~/.config/takusu/config.toml
```

データフロー: Browser → fetch(localhost) → axum → TakusuApp → Storage

## UX 方針 (デスクトップ最適化)

mobile の UI をそのまま移植しない。デスクトップの武器 (横幅・キーボード・マウス精度) を活かす:

- **Week view 主軸**: 日=列、時間=行。Day/Week 切替。1日縦タイムライン (mobile 方式) だと横幅が死ぬ
- **左サイドバー = mini calendar + filter + nav**: タスクリストは置かない (Timeline と重複するため)。日付ジャンプは mini calendar
- **Overlay Inspector**: タスク詳細は選択時に右からスライドイン (Linear/Figma 方式)。常設カラムにせず Timeline を圧迫しない
- **Command Palette (Ctrl+K)**: 全アクション + タスク fuzzy search。vim 風 UX と相性抜群。レア操作はショートカット暗記より palette
- **Agent = Dockable panel**: 右寄せ floating・リサイズ可。どこからでも呼べる
- **マウス操作**: ドラッグ&ドロップで reschedule (5分スナップ、`move` API)、右クリックコンテキストメニュー、ホバーでクイックアクション (start/done)
- **生きたフィードバック**: now line が実時間で動く、in_progress タスクに progress ring、ドラッグ時に ghost + slot ハイライト、操作後に undo 付き toast

## Layout

```
┌──────────────────────────────────────────────────────────────┐
│ [takusu] [Day|Week] [‹ today ›]        [⌘K] [+ task] [sync]  │
├────────────┬─────────────────────────────────────────────────┤
│ mini cal   │  Week Timeline (Mon..Sun 列、時間行)            │
│ ┌────────┐ │  06:00                                         │
│ │ July   │ │  07:00  [task ]                                │
│ └────────┘ │  08:00  [task ] [task ]                        │
│            │        ────── now line (実時間) ──────          │
│ filters    │                                                 │
│ [x]pending │  - ブロックをドラッグ → move (5分スナップ)      │
│ [x]sched   │  - ホバー: start/done ボタン                    │
│ [ ]done    │  - 右クリック: context menu                     │
│            │                                                 │
│ habits     │                                                 │
│ nav: Graph │                                                 │
│  Stats     │                                                 │
├────────────┴─────────────────────────────────────────────────┤
│ -- NORMAL -- next: 会議 @ 14:00   unscheduled: 3   sync: ok  │
└──────────────────────────────────────────────────────────────┘

  + Inspector (タスク選択時に右から overlay スライドイン)
  + Agent dockable panel (右寄せ floating、リサイズ可)
  + Command Palette (Ctrl+K、中央 overlay)
```

- **Toolbar**: Day/Week 切替、日付ナビゲーション、Command Palette、新規タスク、sync
- **左サイドバー**: mini calendar (タスクあり日にマーク、日付選択でジャンプ)、status filter、habit リスト、view nav (Graph/Stats)
- **中央**: Week/Day Timeline (Graph/Stats view に切替可)
- **Inspector**: 選択中タスクの詳細・編集 (overlay)
- **Agent panel**: dockable floating、`a` か toolbar から toggle
- **Status bar**: vim mode、next task、unscheduled count、sync 状態

## Keyboard Shortcuts (vim 風)

### Navigation

| Key | Action |
|-----|--------|
| `j` / `k` | Timeline 上のタスクブロックを時系列移動 |
| `[` / `]` | 前日/翌日へ移動 |
| `g d` | Day view に切替 |
| `g w` | Week view に切替 |
| `g g` | 今日へジャンプ |
| `Enter` / `l` | Inspector を開く (選択タスクの詳細) |
| `h` / `Esc` | Inspector / palette を閉じる |

### Actions

| Key | Action |
|-----|--------|
| `o` | 新規タスク作成 |
| `e` | タスク編集 (Inspector を編集モードで開く) |
| `d` | タスク削除 (confirm) |
| `x` | タスク完了 (done) |
| `s` | タスクスキップ |
| `Space` | タスク開始/一時停止 (start/pause toggle) |
| `p` | Progress 記録 (quantity 入力) |
| `r` | スケジュール再生成 |
| `u` | undo |
| `Ctrl+r` | redo |

### Views & Panels

| Key | Action |
|-----|--------|
| `Ctrl+k` | Command Palette |
| `a` | Agent panel toggle |
| `g h` | Habit view に切替 |
| `g s` | Stats view に切替 |
| `g r` | Graph view に切替 |
| `1`-`5` | status filter (pending/scheduled/in_progress/completed/skipped) |
| `/` | 検索フォーカス |
| `?` | ショートカットヘルプ |

## Phase 1: 基本 UI + タスク管理 + Progress

### 目標

3 カラムレイアウト + タスク CRUD + スケジュール (Timeline) 表示 + vim ショートカット + Progress トラッキング

### 1.1 Rust: `crates/takusu-web` 作成

- `Cargo.toml`: axum, tokio, takusu-local-lib, rust-embed, tower-http
- `src/main.rs`:
  - `~/.config/takusu/config.toml` を読んで `LocalConfig` を構築
  - takusu-local-lib の router を `/api/*` に mount
  - rust-embed で `web/dist/` を `/*` に serve (SPA fallback 付き)
  - 認証ミドルウェアなし
- `src/embed.rs`: `#[derive(RustEmbed)] #[folder = "../../web/dist"]`
- takusu-local-lib の config 読み込みを CLI の `config.rs` と共通化 (または takusu-web 側で toml を読んで LocalConfig に変換)

### 1.2 Frontend: `web/` プロジェクト初期化

- `npm create vite@latest web -- --template react-ts`
- 依存: `tailwindcss`, `@tailwindcss/vite`, shadcn/ui (init), zustand
- `vite.config.ts`: proxy `/api` → `localhost:3000` (dev 時)
- ディレクトリ:
  ```
  web/
  ├── src/
  │   ├── api/          # REST API client (fetch wrapper)
  │   ├── components/   # shadcn/ui + custom
  │   ├── views/        # Timeline, TaskList, TaskDetail, Graph, Habit, Stats, Agent
  │   ├── hooks/        # useKeyboard, useTasks, useSchedule, useWebSocket
  │   ├── stores/       # zustand stores
  │   └── lib/          # utils, types
  ├── index.html
  ├── vite.config.ts
  ├── tailwind.config.ts
  └── package.json
  ```

### 1.3 API Client (`web/src/api/`)

- takusu-contracts の model.rs の型を TS にポーティング (`types.ts`)
- `client.ts`: fetch wrapper (baseURL, error handling)
- エンドポイント: tasks CRUD, schedule get/generate/reschedule/move/clear, settings, habits CRUD, progress

### 1.4 画面実装

- **App.tsx**: Toolbar + Sidebar + Main + Inspector overlay + Agent panel + Palette のシェル
- **Toolbar**: Day/Week 切替、日付ナビゲーション (‹ today ›)、⌘K ボタン、+ task、sync
- **Sidebar** (左): mini calendar (タスクあり日にマーク、日付選択でジャンプ)、status filter、habit リスト、view nav
- **WeekTimeline** (中央): 日=列・時間=行、タスクが時間ブロック、now line (実時間更新)、ドラッグ&ドロップ reschedule (5分スナップ、ghost + slot ハイライト)、ホバー quick actions (start/done)、右クリック menu、日付区切り
- **Inspector** (右 overlay): タスク詳細・編集。title, start, deadline, parallel, cost (avg/sigma), abandonability (5 段階), deps, description, status, quantity (done/total), actual_minutes。スライドイン/アウト
- **StatusBar** (下): vim mode、next task、unscheduled count、sync 状態

### 1.5 vim ショートカット + Command Palette

- `useKeyboard` hook: global keydown listener
- mode: normal / insert (編集時) / search
- normal mode で j/k/o/d/x/s/e/r/u/Space/p、`g` プレフィックスで view 切替、`[`/`]` で日移動
- Status bar に mode 表示
- **Command Palette (Ctrl+K)**: fuzzy search (タスク + アクション)、最近使ったアクション、キーボードのみで完結

### 1.6 タスク CRUD + Progress

- 作成: `o` か toolbar の + で Inspector に作成フォーム (iCal ペースト import 対応)
- 編集: `e` か右クリック → 編集
- 削除: `d` か右クリック → confirm dialog → DELETE
- 完了/スキップ: `x` / `s`、ホバーボタンからも
- Start/Pause: `Space` で in_progress ↔ scheduled トグル
- Progress 記録: `p` で quantity 入力ダイアログ (delta or cumulative, note)
- タスク分割: Inspector から split (残量タスク作成、依存リンク)
- quantity 表示: done/total、+1/-1 クイック調整
- **ドラッグ&ドロップ**: ブロックを掴んで移動 → `move` API (5分スナップ)、違反時は 409 + toast

## Phase 2: グラフ + Habit + Stats + Settings

### 2.1 Graph View (中央)

- Cytoscape.js + dagre (mobile と同じ)
- 推移的依存を全て表示、完了ノードは灰色
- 編集モード: edge 切断 / node 間追加
- 冗長依存の検出・警告・ワンタップ解消
- `g` でグラフビューに切替

### 2.2 Habit View (中央)

- habit カード一覧 + 作成/編集
- RRULE builder (daily/weekly/monthly 等)
- habit steps (ステップごとの title/time/cost/parallel)
- scheduled spans (pause/activation windows)
- cost 推定 (過去の actual_minutes から)
- 直近の生成タスクリスト表示
- `H` で habit ビューに切替

### 2.3 Stats View (中央)

- 期間選択 (today / week / month)
- today summary (完了数、作業時間、達成率)
- ヒートマップ (日次アクティビティ)
- 日次積み上げバー (status 別)
- 将来予測 (unscheduled タスクの負荷)
- habit 内訳 (habit 別の貢献度)
- `S` で stats ビューに切替

### 2.4 Settings View

- **General**: テーマ (Light/Dark/Catppuccin/Aura Soft Dark)、undo 履歴数、timezone
- **Sleep**: sleep_start / sleep_end
- **Workload**: 快適作業時間 / 最大作業時間
- **Solver**: アルゴリズム (Auto/SA/Priority)、時間予算 (ms)、seed、warm start
- **Google Calendar**:
  - 有効/無効
  - Calendar ID, Client ID, Client Secret
  - **OAuth ログイン** (ブラウザで Google OAuth フロー → redirect → token 取得)
  - 手動 refresh token 入力 (fallback)
  - 手動同期トリガー
  - 全イベント削除
- **Worker**: Cloudflare Workers URL / token
- **Info**: version, server health, license
- ショートカットヘルプ (`?`)

### 2.5 TaskDetail 拡張

- deps graph (関係あるものだけ) を右パネルにミニ表示
- habit 関連情報表示
- parallel config 編集
- 冗長依存警告

## Phase 3: Agent

### 3.1 Agent Dockable Panel

- 右寄せ floating panel、リサイズ/折りたたみ可。`a` か toolbar から toggle
- SSE ストリーミング応答
- Markdown レンダリング
- thinking 表示 (collapsible、アニメーション)
- tool call 可視化 (expandable card、args/result、error/rejected badge)
- テキスト応答
- Command Palette から quick モード (1 question → 1 answer)

### 3.2 Approval システム

- Agent が変更提案時に approval panel 表示
- before/after diff、推論フィールド、警告
- approve / reject ボタン
- habit preview (生成タスクのプレビュー)

### 3.3 マルチセッション

- 3-5 セッションをタブで切替
- セッション履歴の永続化 (localStorage)
- 新規セッション作成

### 3.4 Agent 設定

- LLM provider 管理 (name, base URL, API key)
- LLM model 管理 (provider 選択、model 選択、cost 表示)
- TTS provider 管理 (設定のみ、再生は mobile 限定)
- アクティブ LLM model 選択
- セッション履歴数

### 3.5 Skills 管理

- skill 一覧 (built-in は read-only)
- skill 作成/編集/削除 (slug, name, description, body markdown)
- ファイル import

### 3.6 Edit turn / Retry

- メッセージを編集して再実行
- メッセージのコピー/削除

## Phase 4: WebSocket + 同期 + undo/redo

### 4.1 WebSocket

- `crates/takusu-web/src/ws.rs`: axum WebSocket handler
- イベント:
  - `schedule_progress`: スケジュール生成中のプログレス (SA iteration, score)
  - `task_changed`: タスク CRUD 通知
  - `schedule_changed`: スケジュール操作通知
  - `sync_status`: Google Calendar 同期状態
- Frontend: `useWebSocket` hook、イベントに応じて state 更新
- 自動再接続 + exponential backoff

### 4.2 Google Calendar 同期

- Sync ボタン: 同期トリガー
- 同期状態を Status bar / WebSocket で表示
- スケジュール操作後の自動同期 (設定で切替)

### 4.3 undo/redo

- 操作スタック (50 step): タスク CRUD + スケジュール操作 + habit CRUD + progress
- `u` / `Ctrl+r` で undo/redo
- 同期操作は含まない
- undo/redo toast 表示

## Files to Create

### Rust

- `crates/takusu-web/Cargo.toml`
- `crates/takusu-web/src/main.rs` — server entry point
- `crates/takusu-web/src/embed.rs` — rust-embed 静的ファイル
- `crates/takusu-web/src/ws.rs` — WebSocket handler (Phase 4)

### Frontend

- `web/package.json`
- `web/vite.config.ts`
- `web/tailwind.config.ts`
- `web/tsconfig.json`
- `web/index.html`
- `web/src/main.tsx`
- `web/src/App.tsx`
- `web/src/api/client.ts` — fetch wrapper
- `web/src/api/types.ts` — TS types (from takusu-contracts model.rs)
- `web/src/hooks/useKeyboard.ts` — vim shortcuts
- `web/src/hooks/useTasks.ts` — task state
- `web/src/hooks/useSchedule.ts` — schedule state
- `web/src/hooks/useWebSocket.ts` — WS client (Phase 4)
- `web/src/hooks/useAgent.ts` — agent SSE client (Phase 3)
- `web/src/views/Timeline.tsx`
- `web/src/views/TaskList.tsx`
- `web/src/views/TaskDetail.tsx`
- `web/src/views/GraphView.tsx` (Phase 2)
- `web/src/views/HabitView.tsx` (Phase 2)
- `web/src/views/StatsView.tsx` (Phase 2)
- `web/src/views/SettingsView.tsx` (Phase 2)
- `web/src/views/AgentView.tsx` (Phase 3)
- `web/src/views/AgentSettingsView.tsx` (Phase 3)
- `web/src/views/SkillsView.tsx` (Phase 3)
- `web/src/components/StatusBar.tsx`
- `web/src/components/TaskCard.tsx`
- `web/src/components/TaskForm.tsx`
- `web/src/components/ProgressSheet.tsx`
- `web/src/components/SplitTaskModal.tsx`
- `web/src/components/CalendarOverlay.tsx`
- `web/src/components/ApprovalPanel.tsx` (Phase 3)
- `web/src/components/ToolCallCard.tsx` (Phase 3)
- `web/src/stores/` — zustand stores

## Files to Modify

- `Cargo.toml` (workspace) — `crates/takusu-web` を members に追加

## Verification

- [ ] `cargo check -p takusu-web` が通る
- [ ] `cargo nextest run --workspace` が既存テスト全て通る
- [ ] `cd web && npm run build` が通る
- [ ] `cd web && npx tsc --noEmit` が通る
- [ ] takusu-web 起動後、ブラウザで localhost にアクセスして UI が表示される
- [ ] タスク CRUD + progress (quantity, start/pause/complete, split) が動作する
- [ ] Timeline にスケジュールが時間ブロックとして表示される
- [ ] vim ショートカットが動作する
- [ ] Graph view で依存関係が dagre layout で表示される (Phase 2)
- [ ] Stats view でヒートマップ/バー/予測が表示される (Phase 2)
- [ ] Settings から Google Calendar OAuth が完了する (Phase 2)
- [ ] Agent チャットが SSE でストリーミング表示される (Phase 3)
- [ ] Agent approval (diff 表示、approve/reject) が動作する (Phase 3)
- [ ] WebSocket でスケジュール生成プログレスがリアルタイム表示される (Phase 4)
- [ ] undo/redo (50 step) が動作する (Phase 4)

## Risks/Considerations

- **rust-embed のビルド**: `web/dist/` がビルド済みである必要あり。CI では先に `npm run build`、その後 `cargo build`。開発時は `vite dev` + proxy で回避
- **takusu-local-lib の router 再利用**: takusu-local が持つ axum router を takusu-web でも使う。router の分離 (takusu-local-lib に router を移す or takusu-local から export) が必要になる可能性
- **config 共通化**: CLI の `config.rs` は takusu-cli 内にある。takusu-web も同じ TOML を読むため、config 読み込みを takusu-local-lib に移す or 重複実装
- **Cytoscape.js の bundle size**: ~500KB。code splitting で graph view のみ lazy load
- **WebSocket の再接続**: ネットワーク切断時の自動再接続 + exponential backoff
- **SPA fallback**: axum で `/*` の fallback を index.html に。`/api/*` と `/ws` は除外
- **5 min slot の Timeline 描画**: 1 日 = 288 slot。パフォーマンスに注意 (virtualization)
- **Agent SSE**: mobile は localhost の embedded server に SSE で接続。WebUI も同じ endpoint を使う。agent の tool call には approval が必要なため、SSE + REST の組み合わせ
- **Google Calendar OAuth redirect**: localhost の WebUI で OAuth redirect を受けるため、`http://localhost:PORT/api/sync/oauth/callback` 等の endpoint が必要。mobile は native Google Sign-In だが、Web はブラウザ OAuth フロー
- **Agent の LLM 設定**: API key の保存先。mobile は SecureStore だが、Web は config.toml or localStorage (localhost のみなので平文許容)
