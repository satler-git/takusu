# takusu-cli コマンド体系の再設計 (re-topo)

## 目的

takusu-cli のコマンド体系を verb-first に再設計し、日常操作の打鍵数と迷いを減らす。
後方互換は考慮しない。
旧コマンドはエイリアスも残さず削除する。

対象は次の三つとする。

- コマンドツリーの再構成（統廃合、階層の浅化、命名統一）
- フラグと引数の見直し（必須フラグの位置引数化、分かりにくい引数の廃止）
- 出力と対話 UX の改善（`$EDITOR` 編集、エラーメッセージ、デフォルト動作）

TUI の再設計は方針のみ本ドキュメントで定め、詳細は実装時に詰める。

## 現状の問題

現在のコマンドは noun-verb の 2〜4 階層で構成されている。
利用頻度の高い操作ほど階層が深く、打鍵数が多い。

1. **日常操作が深い**：作業開始が `takusu task work start #5` と 4 階層になる。頻度と深さが逆転している。
2. **状態変更の経路が三つある**：`task status`、`task update --status`、`task work {start,complete}` が同じ状態遷移を別々に実装しており、どれを使うべきか決められない。
3. **`replace` (PUT) が CLI として無意味**：全フィールドをフラグで再指定する操作は対話的に使えず、`update`/`edit` と役割が重複する。
4. **`schedule reschedule --mode` が不透明**：必須フラグなのに取りうる値が help から読めない。
5. **`habit scheduled-spans` の命名が実態と乖離**：習慣の休止期間を表すのに、名前からそれが読み取れない（エイリアスに `pause` がある時点で命名の失敗を認めている）。
6. **トップレベルがノイジー**：`health`、`gen-root-token`、`license`、`mcp`、`web` などの低頻度コマンドが `task` と同列に並び、`--help` の一覧から日常操作を探しにくい。
7. **`config set` がフラグの羅列**：`agent config set <key> <value>` と形式が揃っておらず、フラグを覚えられない。
8. **`$EDITOR` 編集が使いづらい**：独自の key:value 形式で、拡張子がないため syntax highlight が効かない。加えて、raw フィールド（`avg_minutes: 30` や ISO 8601 の生文字列）をそのまま露出しており、入力形式が CLI フラグ（`30m` や `2025-06-05T14:00`）と一致しない。parse に失敗すると編集内容が失われる。

## 設計方針

タスク操作がこの CLI の主役であるため、`task` プレフィックスを廃止し、タスク操作の動詞をトップレベルに置く。
習慣、メモリ、同期などの低頻度ドメインは noun グループとして残すが、グループ内の動詞は共通の語彙（`add`、`ls`、`show`、`edit`、`rm`）に統一する。

状態変更は動詞に一本化する。
`start`/`pause`/`done`/`skip` が work session と status を同時に扱い、`status` サブコマンドと `update --status` は削除する。
任意フィールドの変更は `edit` に統合する。

## 新コマンドツリー

### タスク操作（トップレベル動詞）

```
takusu add <title> [--due <dt>] [--at <dt>] [--time 30m] [--sigma 0] [...]
takusu ls [query...] [--status s] [--from dt] [--until dt] [--limit n]
takusu show <ref>
takusu start <ref>            # work session 開始 + in_progress
takusu pause <ref>            # session 中断
takusu done <ref>             # session 終了 + completed
takusu skip <ref>             # skipped
takusu edit <ref> [--flags]   # フラグなし → $EDITOR、フラグあり → PATCH
takusu rm <ref>
takusu progress <ref> <quantity> [--note <text>]
takusu split <ref> --keep <quantity> [--title ...] [--due ...] [--dep]
takusu import <file.ics|->    # iCal 取り込み
takusu deps [--check]         # 依存の表示と冗長エッジ検出 (旧 deps-check)
```

`<ref>` は既存の display reference（`#5`、`h1#3`、UUID）をそのまま使う。
`add` の主要フラグは短く改名する（`--end-at` → `--due`、`--start-at` → `--at`、`--avg-time` → `--time`、`--sigma-time` → `--sigma`）。
`progress` の quantity は必須フラグから位置引数に変える。
`split` の `--retained-quantity` は `--keep` に改名する。

`ls` のデフォルトは actionable なタスク（pending、scheduled、in_progress）とし、完了済みを見るときだけ `--all` か `--status completed` を指定する。

### スケジュール操作

```
takusu                        # 引数なし = agenda 表示
takusu agenda [--day <date>]  # アクティブスケジュール表示 (旧 schedule get)
takusu plan [--from dt] [--until dt] [--tasks ref...] [--pin ref...] [--sleep s]
takusu move <ref> <start_at> [--force]
takusu unplan                 # アクティブスケジュール破棄 (旧 schedule clear)
```

`plan` は generate と reschedule を統合する。
範囲もタスク指定もなければ全体を生成し、`--from`/`--until`/`--tasks` のいずれかがあれば部分再計画とする。
旧 `reschedule --mode` は、この範囲指定の有無から動作が決まるため廃止する。

引数なしの `takusu` は TUI ではなく agenda 表示に変える。
一日の予定確認が最頻の操作であり、状態を持たない一覧表示のほうがシェルから呼ぶコストが低いためである。
TUI は `takusu tui` で起動する。

### 低頻度ドメイン（noun グループ）

```
takusu habit {add, ls, show, edit, rm,
              pause <ref> --from <date> --to <date> [--reason],
              pauses [ls|rm],       # 旧 scheduled-spans
              steps {ls, edit, set, check}}
takusu memory {add, ls, show, edit, rm, search <q>, similar <title>}
takusu skill {add, ls, show, edit, rm}
takusu token {add, ls, rm}
takusu sync {status, setup, login, run, mappings, purge}
takusu config {show, init, set <key> <value>, workers {set, health}}
takusu agent ["text"] {config, allow, deny}   # 詳細は「Agent UX」節
```

グループ内の動詞は共通語彙に統一する。
`create` → `add`、`list` → `ls`、`delete`/`revoke` → `rm`、`update` → `edit` に吸収、`replace` は削除。
`habit scheduled-spans add` は `habit pause` に、`sync trigger` は `sync run` に、`sync delete-all` は `sync purge` に、`sync settings` は `sync status` に改名する。

### システム系

```
takusu tui
takusu web [--bind addr]
takusu mcp
takusu system {health, gen-root-token, license, completion <shell>}
```

低頻度のユーティリティは `system` グループに退避し、トップレベルの `--help` を日常操作中心にする。

## 旧新対応表

| 旧 | 新 |
|----|----|
| `task create` | `add` |
| `task list` | `ls` |
| `task show` | `show` |
| `task edit` / `task update` | `edit` |
| `task replace` | 削除 |
| `task delete` | `rm` |
| `task status <ref> in_progress` | `start` |
| `task status <ref> completed` | `done` |
| `task status <ref> skipped` | `skip` |
| `task status <ref> pending` | `edit <ref> --status pending`（例外的経路として残す） |
| `task work start/pause/complete` | `start` / `pause` / `done` |
| `task work progress` | `progress` |
| `task work progress-show` | `show <ref>`（sessions と progress を詳細表示に統合） |
| `task work split` | `split` |
| `task import-ical` | `import` |
| `task deps-check` | `deps` |
| `schedule get` | `agenda`（引数なし `takusu` も同じ） |
| `schedule generate` / `schedule reschedule` | `plan` |
| `schedule move` | `move` |
| `schedule clear` | `unplan` |
| `habit scheduled-spans` | `habit pause` / `habit pauses` |
| `habit steps-check` | `habit steps check` |
| `token create/list/revoke` | `token add/ls/rm` |
| `sync settings/trigger/delete-all` | `sync status/run/purge` |
| `config set --key val` | `config set <key> <value>` |
| `agent run` | `agent`（bare で REPL、`agent "text"` でワンショット） |
| `agent config permissions {show,set,unset}` | `agent allow <key>` / `agent deny <key>` / `agent config show` |
| `health` / `gen-root-token` / `license` / `completion` | `system {...}` |
| 引数なし（TUI 起動） | agenda 表示 |

## `$EDITOR` 編集の再設計

独自 key:value 形式を廃止し、TOML に変える。
一時ファイルに `.toml` 拡張子を付けることで、エディタの syntax highlight と補完が効く。

値の形式は CLI フラグと揃える。
時間は `"30m"` や `"1h30m"`、日時は `"2025-06-05 14:00"`（設定タイムゾーンで解釈）を受け付ける。
raw の分数や UTC ISO 文字列を直接編集させない。

```toml
# takusu edit #5 (lines starting with # are comments)
title = "レポート提出"
due = "2025-06-05 23:59"
time = "1h30m"
status = "pending"      # pending | scheduled | in_progress | completed | skipped
depends = ["#3", "h1#5"]

[advanced]
sigma = "0"             # 0 = auto (time/5)
abandonability = 0.5
parallelizable = false
fixed = false
```

編集頻度の低いフィールド（quantity 系、parallel 系）は `[advanced]` セクションに分け、視線の移動を減らす。

parse や validation に失敗した場合は、エラーをコメントとしてファイル先頭に挿入し、同じファイルで再度エディタを開く。
編集内容を失わせない。
空ファイルを保存するか変更がなければ中断とみなす。

habit の `edit` と `steps edit` も同じ TOML 形式に揃える（steps は現在 JSON であり、コメント可否のために独自の `//` 除去をしている。TOML にすればこの処理も消せる）。

## 出力と対話 UX

- `--mode rich/simple` は環境の自動判定に変える。TTY なら rich、パイプなら simple を選び、`--plain` で強制できるようにする。
- エラーメッセージに次の行動を含める。たとえば `#5 not found` には近い display_id の候補を、workers 設定不足には該当する `config set` コマンドを添える。
- `add` の対話モード（引数なし）は継続するが、質問は title と due の 2 問に減らし、残りはデフォルト値を使う。
- `done`/`start`/`skip` の成功時は、対象タスクの 1 行サマリを出力し、何に作用したか確認できるようにする。

## Agent UX

agent はタスク操作に次ぐ高頻度の操作だが、CLI 側の実装は最小限にとどまっている。
REPL は `> ` プロンプトで `read_line` するだけであり、行編集も履歴もない。
takusu-agent には streaming API（`run_turn_stream`、`TurnEvent` の Thinking / Text / ToolCall / ToolResult / Done）が既にあるのに、CLI は blocking の `run_turn` を使っており、ターン完了まで無反応で全文を一括出力する。
承認は `y/N` の一問一答で、ユーザーへの質問 UI は ASR 訂正専用のハードコードになっている。

### コマンドの昇格と平坦化

`agent run` の `run` を外し、bare の `takusu agent` で REPL、`takusu agent "text"` でワンショットとする。
権限管理は 3 階層の `agent config permissions set <key> <value>` をやめ、`agent allow <key>` と `agent deny <key>` に平坦化する。
`--allow`/`--deny` フラグ（セッション限定）はそのまま残し、サブコマンド版は設定ファイルへの永続化と役割を分ける。
`agent config show` は、ファイルの生の内容ではなくデフォルト値込みの実効設定を表示する。

### REPL の再設計

REPL を、現代的な coding agent CLI（Claude Code や Devin CLI の対話画面）に近い見た目に作り直す。
具体的には次の要素で構成する。

- **inline レンダリング**：alternate screen ではなく通常のスクロールバックに会話を流し、画面下部に入力欄を固定する。ratatui の inline viewport か、crossterm 直書き + reedline の組み合わせを実装時に選定する（ratatui は takusu-tui で導入済み）。
- **入力欄**：複数行編集、履歴（上下キー）、Ctrl-C でターン中断。
- **streaming 表示**：`run_turn_stream` に乗り換え、Text はトークン到着ごとに描画する。Thinking は淡色の折りたたみ表示にし、進行中はスピナーを出す。
- **markdown レンダリング**：見出し、リスト、コードブロックに色とインデントを付ける。
- **tool call の可視化**：ToolCall / ToolResult を「ツール名 + 引数の要約 + 成否」の 1 行カードとして会話に挟む。
- **承認 UI**：変更一覧を diff 風（フィールドごとの before → after）に整形する。選択肢は y / n に加えて、その操作の permission をセッション許可へ昇格する「always (a)」を置く。
- **質問 UI**：ASR 専用の `UserInputProvider` 実装を、汎用の質問プロンプト（purpose を添えた自由入力）に置き換える。
- **slash コマンド**：`/help`、`/clear`、`/compact`、`/model`、`/permissions`、`/exit` を REPL 内コマンドとして提供する。

streaming と TurnEvent は takusu-agent 側に揃っているため、この再設計は CLI クレート内で完結する見込みである。
承認 UI の「always」昇格も、既存のセッション permission API（`set_session_permissions`）で表現できる。

## TUI 再設計の方針

詳細設計は別途行うが、CLI の再設計と揃える方針だけ定める。

- 起動時の初期画面を agenda（今日のスケジュール）にする。現在は Schedule タブが先頭にあるが、日付グループ全体のリストであり「今日」に焦点がない。
- キーバインドを CLI の動詞と対応づける（`a` = add、`s` = start/pause、`d` = done、`x` = skip、`e` = edit、`/` = 検索）。
- タスク作成フォームを拡張し、CLI の `add` と同じフィールド（due、time、description）を入力できるようにする。
- 習慣の作成と編集を追加する（現在は削除のみ）。
- スケジュール生成に範囲指定を追加する（現在は固定パラメータ）。
- 検索とフィルタを統合し、`ls` と同じ query 構文を使う。

## 実装計画

コマンドツリーの変更は clap の enum 定義とルーティングに閉じており、`TakusuApp` の API は変更しない。
段階ごとに独立して動く状態を保つ。

1. **フェーズ 1（コマンドツリー）**：トップレベル動詞の導入、noun グループの統廃合、旧コマンドの削除。対応表の全項目を実装する。completion も再生成対象になる。
2. **フェーズ 2（フラグと対話）**：フラグ改名（`--due` 等）、位置引数化、`--mode` 自動判定、エラーメッセージ改善、`add` 対話モードの簡素化。
3. **フェーズ 3（editor）**：TOML 形式への移行、エラー時の再編集ループ、habit と steps への適用。
4. **フェーズ 4（agent）**：`agent` の昇格と `allow`/`deny` の平坦化、streaming への乗り換え、REPL の再設計。REPL のレンダリング基盤の選定（ratatui inline か reedline か）はこのフェーズの冒頭で行う。
5. **フェーズ 5（TUI）**：上記方針に沿った再設計。着手前に画面遷移とキーバインドの詳細を別 plan として起こす。

各フェーズの完了時に `.devin/docs/clients.md` を更新する。
MCP ツールと agent の permission キー（`task:create` 等）は内部 API 名に基づいており、CLI の改名の影響を受けないが、フェーズ 1 の完了時に確認する。

## 未決事項

- `ls` の query 構文（`status:pending OR 買い物`）を維持するか、単純な部分一致に置き換えるか。
- `agenda` の表示形式（現在の schedule get のテーブルをそのまま使うか、今日に絞った時系列表示にするか）。
- `deps` の表示形式（一覧かツリーか）。
- TUI で `$EDITOR` 統合を残すか、インラインフォームに置き換えるか。
- agent の会話をプロセスを跨いで永続化するか（`takusu agent --continue` で直前の会話を再開する形など）。takusu-agent のセッションが serialize 可能かの調査から必要になる。
