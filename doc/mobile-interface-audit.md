# モバイルアプリ インターフェース監査（better-interface full）

- モード: `full`
- 対象: `mobile/app` の全ルート、`mobile/src/views`・`mobile/src/components` の全画面・コンポーネント
- スタック: React Native + Expo、TypeScript、`react-native-paper`、`react-native-reanimated`、`react-native-gesture-handler`、カスタムテーマ `mobile/src/theme.tsx`（light / dark / catppuccin / aura-soft-dark）
- 境界: ネイティブモジュール・Rust コア・Kotlin ファイルは未監査。実機のスクリーンリーダー・軽減モーション・RTL 表示は未検証。

## カバレッジ

| 領域 | 確認した証拠 | 結果 |
|---|---|---|
| Accessibility | 全 view / component、`accessibilityLabel`・`accessibilityRole`・`accessibilityViewIsModal`・`useReducedMotion`・`announceForAccessibility`・`hitSlop`・アイコンのみ `Pressable` パターンを検索 | 5 件 |
| Layout | 全 `.tsx`、`left`/`right`・`start`/`end`・`numberOfLines`・`ScrollView`  affordance・safe-area 処理を検索 | 1 件 |
| Writing | ユーザー向け文字列全般、`showError`・`Alert.alert`・`placeholder`・空状態・英語混入・テンプレートリテラルを検索 | 2 件 |
| Typography | 全 `.tsx`、`fontSize`・`lineHeight`・`fontWeight`・`numberOfLines`・`TextInput` サイズを検索 | 4 件 |
| Colors | `mobile/src/theme.tsx`、color extraction スクリプト、`__tests__/theme-contrast.test.ts`、`color-pairs-overrides.json`、`colors.red` / `rgba` / `#` 使用箇所を検索 | 1 件 |
| UI | 全 view / component、`borderRadius` ネスト、`Image` 外形線、`withSpring`/`withTiming`・press feedback・`CrossFadeIcon` を検索 | 2 件 |

## Findings

深刻度順。同じ深刻度内では影響範囲の広いものを先に配置しています。

| # | Severity | Domain | Location | Before | After | Why |
|---|---|---|---|---|---|---|
| 1 | HIGH | Accessibility | `mobile/src/views/HomeView.tsx:2240`<br>`mobile/src/components/ContextMenu.tsx:283`<br>`mobile/src/components/ViewChanger.tsx:70–86`<br>`mobile/src/components/NavigationButtons.tsx:44–55`<br>`mobile/src/components/FloatingVoiceButton.tsx:211`<br>`mobile/src/components/TaskCard.tsx:442, 450`<br>`mobile/src/components/TaskSearchBar.tsx:286–288`<br>`mobile/src/components/DeleteConfirmButton.tsx:57–82`<br>その他アイコンのみ `Pressable` / `PressableScale` 約 20 箇所 | `<PressableScale …><Ionicons … /></PressableScale>` で `accessibilityLabel` / `accessibilityRole` なし（Home 同期、ContextMenu ハンバーガー、ViewChanger、NavigationButtons、TaskCard skip/delete、検索クリア、削除ボタンなど） | 全アイコンのみ pressable に `accessibilityRole="button"` と `accessibilityLabel="…"` を追加。`FloatingVoiceButton` にはジェスチャーに加え「エージェントを開く / 上にスライドしてタスク追加」という tap 経路とラベルを用意 | スクリーンリーダー利用者が主要なナビゲーション・タスク操作・検索・エージェント入口を特定・操作できない |
| 2 | HIGH | Accessibility | `mobile/src/components/PressableScale.tsx:107, 114`<br>`mobile/src/components/TaskCard.tsx:288–305`<br>`mobile/src/components/WorkSessionCard.tsx:195–212`<br>`mobile/src/views/HomeView.tsx:419–430, 1541–1567`<br>`mobile/src/components/TopToast.tsx:62–78, 426–431`<br>`mobile/src/components/FloatingVoiceButton.tsx` | `scale.value = withSpring(activeScale, springConfig)`、`translateX.value = withSpring(panelOffset)`、Toast 入退場の spring、Home のナビ矢印 spring など、全アニメーションが条件なしで実行されている。`useReducedMotion()` や `AccessibilityInfo.isReduceMotionEnabled()` のチェックなし | `react-native-reanimated` の `useReducedMotion()` を使い、軽減モーション時は spring duration を 0 にするか動作を無効化 | 前庭・動作に敏感なユーザーがアプリ全体の spring / slide アニメーションを避ける方法がない |
| 3 | HIGH | Accessibility | `mobile/src/components/TopToast.tsx:435–474`<br>`mobile/src/components/UndoRedoToast.tsx:22–34` | Toast は `Reanimated.View` に `pointerEvents="auto"` があるが、`accessibilityLiveRegion`・`accessibilityRole="status"`・`AccessibilityInfo.announceForAccessibility(message)` 呼び出しがない | Toast コンテナに `accessibilityLiveRegion="polite"` / `accessibilityRole="status"` を追加するか、表示時に `announceForAccessibility` を呼ぶ | スクリーンリーダー利用者がエラー・成功・Undo/Redo のフィードバックを見落とす |
| 4 | HIGH | Typography | `mobile/src/views/TaskDetailView.tsx:308, 427`<br>`mobile/src/views/StatsView.tsx:748, 806`<br>`mobile/src/views/HabitDetailView.tsx:328`<br>`mobile/src/components/TaskCard.tsx:180`<br>`mobile/src/views/AgentView.tsx:1018`<br>`mobile/src/components/RruleBuilderModal.tsx:220`<br>`mobile/src/components/TaskSearchBar.tsx:372`<br>`mobile/src/components/SplitTaskModal.tsx:63`<br>`mobile/src/views/AgentView.tsx:1043, 1089`<br>`mobile/src/components/PermissionsEditor.tsx:340`<br>`mobile/src/components/settings/TtsProviderEditor.tsx:44`<br>`mobile/src/components/settings/LlmModelEditor.tsx:74` | `fontSize: 9` / `fontWeight: '800'` など 12px 未満のラベル、`fontSize: 14–15` またはサイズ未指定（デフォルト <16）の `TextInput` が多数 | 非入力テキストは最低 12px 以上、`TextInput` は全て `fontSize: 16` に統一 | 12px 未満のテキストは低視力ユーザーが読めない。16px 未満の入力は読みにくく、タップしにくい |
| 5 | MEDIUM | Writing | `mobile/src/api/errors.ts:168–178`<br>`mobile/src/views/SettingsView.tsx:637, 681, 705, 737, 887, 960`<br>`mobile/src/views/HomeView.tsx:569`<br>`mobile/src/views/AgentSettingsView.tsx:446, 476`<br>`mobile/src/views/SkillsSettingsView.tsx:88`<br>`mobile/src/views/HabitAddView.tsx:246`<br>`mobile/src/views/TaskDetailView.tsx:245` | `showError(e, 'エラー')`、`showError(e, 'タスク一覧の取得に失敗')`、`showError(e, 'Habitの追加に失敗')`、フォールバック Alert の `{ text: 'OK' }`、確認ダイアログのタイトルが名詞のみ（`'削除'`） | 失敗内容と次の手順をタイトルに含める（例：`'タスク一覧を取得できませんでした。接続を確認して再試行してください'`）。`OK` は `閉じる` に。確認ダイアグのタイトルは `〜を削除` など対象を明示 | 汎用タイトルと `OK` ではユーザーが原因と復旧方法を判断できない |
| 6 | MEDIUM | Accessibility | `mobile/src/components/TaskProgressSheet.tsx:145`<br>`mobile/src/components/HabitEstimateModal.tsx`<br>`mobile/src/components/RruleBuilderModal.tsx`<br>`mobile/src/components/DateTimePickerModal.tsx`<br>`mobile/src/components/EditMessageModal.tsx`<br>`mobile/src/components/SessionPermissionsModal.tsx`<br>`mobile/src/components/HabitPreviewModal.tsx`<br>`mobile/src/components/SplitTaskModal.tsx:145`<br>`mobile/src/components/ToolCallDetailModal.tsx` | `react-native` の `Modal` または bottom sheet で、内部 View に `accessibilityViewIsModal={true}` なし、開いた時の初期フォーカス・閉じた時のトリガー復帰もない | 各モーダル内部の最初のフォーカス可能 View に `accessibilityViewIsModal` を設定。開く時は見出しまたは最初の入力にフォーカス、閉じる時はトリガーに戻す | スクリーンリーダー・キーボード利用者がモーダル開閉を認識できず、背景に逸脱しやすい |
| 7 | MEDIUM | Accessibility | `mobile/src/components/NavigationButtons.tsx:44–55`（36×36）<br>`mobile/src/components/ViewChanger.tsx:34–39`（40×40）<br>`mobile/src/components/RruleBuilderModal.tsx:191–198`（40×40）<br>`mobile/src/components/HabitStepEditor.tsx:338–363`<br>`mobile/src/components/TaskProgressSheet.tsx:431–439`<br>`mobile/src/views/AgentView.tsx:2881–2903` | 44×44 を下回るターゲットが複数。また `placeholder="目標"`、`placeholder="メッセージ"`、`placeholder={q.text}` など、入力の唯一の可視ラベルが placeholder になっている箇所がある | タッチターゲットは 44×44 または `hitSlop` で拡張。`TextInput` には上に可視の `Text` ラベルを追加し、placeholder は例示に使う | 小さなターゲットは押しにくい。入力を始めると placeholder ラベルが消えて何を入力すべきか分からなくなる |
| 8 | MEDIUM | Typography | `mobile/src/components/ToolResultViews.tsx:95–126`<br>`mobile/src/components/WorkSessionCard.tsx:305`<br>`mobile/src/components/TaskProgressSheet.tsx:351–356` | `ToolResultViews` のタイトル `numberOfLines={1}`・説明 `numberOfLines={2}` に展開なし。`WorkSessionCard`・`TaskProgressSheet` のタイトルも `numberOfLines={1}`/`{2}` で、進捗シートが唯一の詳細画面 | `ellipsizeMode="tail"` を設定し、全文を見られる経路を用意（カードをタップして詳細、シートタイトルを展開など） | 長いタスク・セッションタイトルやツール結果説明が途切れて、ユーザーが全文を読めない |
| 9 | MEDIUM | Writing | `mobile/src/views/HabitAddView.tsx:264, 295`<br>`mobile/src/views/SkillsSettingsView.tsx:133`<br>`mobile/src/components/ContextMenu.tsx:176, 186`<br>`mobile/src/views/TaskDetailView.tsx:2355`<br>`mobile/src/views/SkillEditView.tsx:307, 325, 339, 355`<br>`mobile/src/views/StatsView.tsx:581–584`<br>`mobile/src/components/ToolCallDetailModal.tsx:705–707` | 日本語 UI に英語が混在（`New Habit`、`Habit name`、`(built-in)`、`reschedule`、`HABIT`、`slug`/`name`/`body` ラベル）。placeholder がラベルを繰り返す（`タスク名`、`元タスクに残す数量`）。空状態は存在しないことを述べるだけ | ラベルを日本語に統一。placeholder は例示にする（`例：買い物リスト`）。空状態は「ここは何か」「次に何をすればいいか」を示す | 言語の混在は voice と一貫性を損なう。ラベル再掲の placeholder と無力な空状態はユーザーを迷わせる |
| 10 | MEDIUM | Typography | `mobile/src/views/TaskDetailView.tsx:144–588`（`fontSize` 値 34 箇所）<br>`mobile/src/views/StatsView.tsx`<br>`mobile/src/views/HabitDetailView.tsx`<br>`mobile/src/views/AgentView.tsx`<br>`mobile/src/components/settings/*.tsx` | 9, 10, 11, 12, 13, 14, 15, 16, 18, 20, 22 など、ベタ書きの `fontSize` が散在 | `mobile/src/theme.tsx` に意味のある type scale（caption, body, input, title, headline 等）を定義し、全コンポーネントで使用 | サイズが統一されていないため保守が困難であり、上記の床下回りの根本原因になっている |
| 11 | MEDIUM | Typography | `mobile/src/components/ApprovalPanel.tsx:53` | `why: { fontSize: 13, lineHeight: 18 }` で行高比が 1.4 未満 | 複数行になる説明文に対して `lineHeight` を 20 以上（≈1.54）にする | 行間が詰まりすぎて、折り返した説明文が読みにくい |
| 12 | MEDIUM | Layout | `mobile/src/components/TopToast.tsx:488–489`<br>`mobile/src/components/TaskSearchBar.tsx:401–402`<br>`mobile/src/components/TaskCard.tsx:110–111, 122–123`<br>`mobile/src/components/WorkSessionCard.tsx:99–100, 109–110`<br>`mobile/src/components/TaskProgressSheet.tsx:201–202`<br>`mobile/src/components/SessionPermissionsModal.tsx:32–33`<br>`mobile/src/components/ToolCallDetailCommon.tsx:58–59`<br>`mobile/src/views/HomeView.tsx:2388`<br>`mobile/src/views/TaskAddView.tsx:204–205`<br>`mobile/src/components/MessageContextMenu.tsx:135` | 方向を意識すべき絶対配置やスワイプ背景に `left: 0, right: 0` または `left: x` を使用 | `start: 0, end: 0` や `insetStart`/`insetEnd` に置換。`MessageContextMenu` は RTL 時に x 座標をミラー | RTL では左右が反転せず、レイアウトが壊れる |
| 13 | MEDIUM | Colors | `mobile/src/theme.tsx` (`colors.red`)<br>`mobile/src/views/HomeView.tsx:2258–2267`<br>`mobile/src/components/WorkSessionCard.tsx:277`<br>`mobile/src/components/TaskCard.tsx:454`（他 `colors.red` 85 箇所以上）<br>`mobile/src/views/TaskDetailView.tsx:1349–1350`<br>`mobile/src/views/HabitDetailView.tsx:1191`<br>`mobile/src/notifications/channels.ts:21` | `colors.red` が abandonability「必須」、削除、実行中/pause、エラー状態の 4 つの意味で使われている。`rgba(255,255,255,0.5)` / `rgba(0,0,0,0.28)` は light/dark のみ。`notifications/channels.ts` はテーマ非依存の `BRAND_COLOR` を直接使用 | `mustDo`、`destructive`、`inProgress`、`error` 等の意味トークンに分離。`rgba` は theme token に alpha をかける形に。通知 LED 色は `notificationColorForTheme()` を使用 | 1 つの赤が複数の意味を持つ。固定 `rgba` と `BRAND_COLOR` は catppuccin / aura-soft-dark テーマを無視する |
| 14 | LOW | UI | `mobile/src/components/TaskCard.tsx:87–96, 120–125`<br>`mobile/src/components/WorkSessionCard.tsx:46–54`<br>`mobile/src/components/TaskProgressSheet.tsx:64–135`<br>`mobile/src/views/TaskDetailView.tsx:182, 354, 369` | 親と子のカードが同じ `borderRadius: 12` かつ内側に `padding: 12`。padding 14 のボタンも `borderRadius: 12` | 内側の角丸を `0` にするか、`outerRadius - padding` で計算し同心円にする | 同じ角丸がネストすると視覚的な違和感が生じる |
| 15 | LOW | UI | `mobile/src/components/WelcomeScreen.tsx:79–83` | `<Image source={WELCOME_IMAGES[theme]} style={styles.image} />` に border なし | `Image` スタイルにテーマに応じた `borderWidth: 1` の薄い border を追加 | スプラッシュ画像が背景と同化し、境界が不明確になることがある |

## 保留・採用見送り

| Location | 候補 | 採用見送りの理由 |
|---|---|---|
| `mobile/__tests__/color-pairs-overrides.json` | 20 件以上の既存コントラスト失敗を新規 finding として報告 | 既に認識・文書化され、フォローアップ注釈が付いている。再報告は重複となる |
| `mobile/src/components/TaskCard.tsx:520` | カードタイトルをインラインで展開 | 全文は詳細画面で読め、スクリーンリーダーでもフルテキストにアクセスできる。2 行省略はリスト密度を保つ意図的な選択 |
| `mobile/src/components/FloatingVoiceButton.tsx` | スライドジェスチャーを単純なタップボタンに置換 | スライドは意図的なパワーユーザーショートカット。対応はラベル付き tap 経路を追加し、ジェスチャー自体を残す |
| `mobile/src/components/CrossFadeIcon.tsx:56–64` | クロスフェードに 4px blur フィルターを追加 | React Native / Reanimated はベクターアイコンに単純な blur フィルターを提供しない。既存の opacity + scale クロスフェードがプラットフォームに適した実装 |
| `mobile/src/components/TopToast.tsx` | 情報 Toast の自動消去時間を 3000ms から延長 | エラー Toast は既に消去されない。非緊急の情報 Toast は 2–3 秒が一般的運用であり、スワイプで消せる |

## 検証

実行済みチェック（全て通過）:

| チェック | コマンド | 結果 |
|---|---|---|
| Lint | `cd mobile && npm run lint` | `Found 0 warnings and 0 errors` |
| Format | `cd mobile && npm run fmt:check` | `All matched files use the correct format` |
| Type check | `cd mobile && npx tsc --noEmit` | exit 0、エラーなし |
| コントラストテスト | `cd mobile && npx jest __tests__/theme-contrast.test.ts` | `PASS` |
| カラー抽出 | `cd mobile && npm run extract-colors:check` | `color-pairs.json is up to date (280 pairs, 9 palette pairs)` |

未検証:
- 実機での TalkBack / VoiceOver 動作
- 軽減モーション設定時の挙動
- RTL 端末でのミラー表示
- デバイス上での同心円角丸・画像外形線の視覚的確認

## Verdict

**Block**

`HIGH` の指摘が 4 件残る：アイコンのみコントロールがスクリーンリーダー非対応、アニメーションが軽減モーションを無視、Toast が読み上げられない、テキスト・入力がタイポグラフィ下限を下回る。これらは障害のあるユーザー・低視力ユーザーのタスク遂行を阻害する。これらを優先して対処し、その後 `MEDIUM`（モーダル・タッチターゲット・タイプスケール・RTL・カラー意味・文章）、`LOW`（角丸・画像外形線）を順に扱う。
