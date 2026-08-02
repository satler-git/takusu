# モバイルアプリ レイアウト監査

対象: `mobile/src/views`・`mobile/src/components` の主要画面・コンポーネント  
スタイリング: React Native `StyleSheet`（インラインスタイル併用）  
指針: [better-layout](../.devin/skills/better-layout/SKILL.md)

## 前提

- React Native の論理プロパティは `marginStart`/`marginEnd`、`paddingStart`/`paddingEnd`、`start`/`end`、`borderStartColor`/`borderEndColor`、`borderStartWidth`/`borderEndWidth`、`borderTopStartRadius` などで表現できます。
- React Native は `flexDirection: 'row'` を RTL で自動ミラーしますが、`marginLeft`/`marginRight` や `left`/`right` は自動ではスワップされません。

## Findings

### 1. Align to shared edges / 論理プロパティ（RTL 対応）

#### 1-a. マージンに物理値を使用

| Severity | Location | Before | After | Why |
|---|---|---|---|---|
| MEDIUM | `mobile/src/views/TaskDetailView.tsx:251`, `mobile/src/components/ApprovalPanel.tsx:87` | `marginLeft: 'auto'` | `marginStart: 'auto'` | 要素を末尾に押し出す auto マージン。`marginLeft` は LTR では右寄せになるが RTL では逆になり、親の start 方向の余白を埋める論理表現が必要 |
| MEDIUM | `mobile/src/views/TaskDetailView.tsx:193`, `mobile/src/views/HabitDetailView.tsx:164` | `marginLeft: 2` | `marginStart: 2` | トップバー内 ID 表示の先頭余白。RTL では右側が start になる |
| LOW | `mobile/src/views/TaskAddView.tsx:64`, `mobile/src/views/HabitAddView.tsx:59`, `mobile/src/views/SettingsView.tsx:167`, `mobile/src/views/LicensesView.tsx:183`, `mobile/src/views/SkillEditView.tsx:49` | `marginLeft: 8` | `marginStart: 8` | 戻るボタンとタイトルの間隔。各画面で同じパターンが繰り返されている |
| LOW | `mobile/src/views/TaskDetailView.tsx:487`, `mobile/src/views/HabitDetailView.tsx:374` | `marginLeft: 4` | `marginStart: 4` | abandonability pip 群の先頭余白 |
| MEDIUM | `mobile/src/components/ToolCallDetailModal.tsx:117`, `150` | `marginLeft: isNested ? 12 : 0` | `marginStart: isNested ? 12 : 0` | 入れ子ツールコールの視覚的インデント |
| MEDIUM | `mobile/src/components/TaskCard.tsx:737` | `marginLeft: INDENT_WIDTH` | `marginStart: INDENT_WIDTH` | 並列タスクグループ内のカードインデント |
| LOW | `mobile/src/components/SplitTaskModal.tsx:214`, `mobile/src/components/TopToast.tsx:518` | `marginLeft: 8`, `marginLeft: 12` | `marginStart: 8`, `marginStart: 12` | インライン Text/View のアイコン・テキスト間隔 |
| LOW | `mobile/src/views/AgentSettingsView.tsx:113`, `mobile/src/views/StatsView.tsx:745`, `760`, `795`, `845` | `marginRight: 10`/`marginRight: 4`/`marginRight: 2` | `marginEnd: 10`/`marginEnd: 4`/`marginEnd: 2` | 行・列内の末尾余白。`marginRight` は RTL では誤った方向に開く（StatsView:845 は `habitDot`） |

#### 1-b. パディングに物理値を使用

| Severity | Location | Before | After | Why |
|---|---|---|---|---|
| LOW | `mobile/src/components/TaskCard.tsx:115`, `mobile/src/components/WorkSessionCard.tsx:105` | `paddingLeft: 20` | `paddingStart: 20` | スワイプ完了時の背景に入る先頭パディング |
| MEDIUM | `mobile/src/components/TaskCard.tsx:241`, `mobile/src/components/WorkSessionCard.tsx:266` | `paddingLeft: CARD_BORDER_RADIUS` | `paddingStart: CARD_BORDER_RADIUS` | スワイプで出るアクションパネルの先頭ボタンがカード角のノッチを埋めるためのパディング |
| LOW | `mobile/src/components/ApprovalPanel.tsx:129` | `paddingLeft: 28` | `paddingStart: 28` | ステップ詳細の先頭インデント |
| LOW | `mobile/src/components/TaskSearchBar.tsx:384`, `mobile/src/components/DateTimePickerModal.tsx:69` | `paddingRight: 8`, `paddingRight: 4` | `paddingEnd: 8`, `paddingEnd: 20` | 横スクロール末尾のクリアランス |

#### 1-c. 絶対配置・方向指定で `left`/`right` を使用

| Severity | Location | Before | After | Why |
|---|---|---|---|---|
| MEDIUM | `mobile/src/components/TaskCard.tsx:129`, `mobile/src/components/WorkSessionCard.tsx:116` | `right: 0` | `end: 0` | カードのスワイプアクションパネルは末尾側に配置すべき。`right` は RTL でミラーされない |
| MEDIUM | `mobile/src/components/TaskCard.tsx:721` | `left: 0` | `start: 0` | 並列グループの先頭側 rail 配置 |
| MEDIUM | `mobile/src/views/TaskDetailView.tsx:260` | `left: 0` | `start: 0` | スケジュールバーの fill 開始位置。RTL では右から左へ伸びる必要がある |
| MEDIUM | `mobile/src/views/TaskDetailView.tsx:453` | `left: 0` | `start: 0` | ミニスケジュールバーの fill 開始位置 |
| MEDIUM | `mobile/src/views/TaskDetailView.tsx:1695`, `2133` | `left: '...%'` | `start: '...%'` | nowdot・ペースマーカーの位置。時間軸を RTL でもミラーするには start 基準が必要 |
| MEDIUM | `mobile/src/views/HabitDetailView.tsx:274` | `left: 13` | `start: 13` | ステップ番号を結ぶ縦線。RTL では右側（start）に来る |
| LOW | `mobile/src/views/HomeView.tsx:278`, `2357` | `right: 24`, `right: 20` | `end: 24`, `end: 20` | 開始/完了 FAB とそのヒント。RTL では左側（end）に来る |
| LOW | `mobile/src/components/NavigationButtons.tsx:30` | `right: 8` | `end: 8 + (I18nManager.isRTL ? insets.left : insets.right)` | 右側浮遊ナビ。RTL では物理的 `right` インセットを `end` に合わせて入れ替える |
| LOW | `mobile/src/components/ViewChanger.tsx:29` | `left: 8` | `start: 8 + (I18nManager.isRTL ? insets.right : insets.left)` | 左下浮遊ビュー切替。RTL では物理的 `left` インセットを `start` に合わせて入れ替える |
| LOW | `mobile/src/components/ContextMenu.tsx:61` | `left: 12` | `start: 12 + (I18nManager.isRTL ? insets.right : insets.left)` | ハンバーガーメニューの起点。安全領域 start inset を含める |
| LOW | `mobile/src/components/ComposerRecordButton.tsx:56` | `left: -44` | `start: -44` | 録音キャンセルヒントの位置 |

#### 1-d. 境界線・角丸で `left`/`right` を使用

| Severity | Location | Before | After | Why |
|---|---|---|---|---|
| MEDIUM | `mobile/src/components/WorkSessionCard.tsx:50-51`, `mobile/src/components/TaskCard.tsx:467-468` | `borderLeftWidth`/`borderLeftColor` | `borderStartWidth`/`borderStartColor` | カードの先頭側アクセントボーダー。RTL では右側に来る |
| MEDIUM | `mobile/src/components/TaskCard.tsx:658`, `729` | `borderRightColor`, `borderRightWidth` | `borderEndColor`, `borderEndWidth` | 並列グループ rail の末尾側境界線 |
| MEDIUM | `mobile/src/components/TaskCard.tsx:133-134`, `mobile/src/components/WorkSessionCard.tsx:120-121` | `borderTopRightRadius`, `borderBottomRightRadius` | `borderTopEndRadius`, `borderBottomEndRadius` | 末尾側アクションパネルの角丸 |
| MEDIUM | `mobile/src/components/TaskCard.tsx:709-712` | `borderTopLeftRadius: 6` / `borderTopRightRadius: 12` など | `borderTopStartRadius: 6` / `borderTopEndRadius: 12` など | 並列グループコンテナの非対称角丸。RTL では「先頭小・末尾大」の関係が崩れる |

### 2. Plan for growth and clipping

| Severity | Location | Before | After | Why |
|---|---|---|---|---|
| MEDIUM | `mobile/src/components/EditMessageModal.tsx:57` | `height: 120` | `minHeight: 120, maxHeight: 240` | 固定高のマルチラインテキスト入力。長文がクリップされる |
| MEDIUM | `mobile/src/components/NavigationButtons.tsx:60` | `width: 300` | `width: '100%', maxWidth: 360` | オーバーレイに左右余白がないため、320px 幅端末では 90% だとカレンダー格子が折り返す |
| LOW | `mobile/src/components/ToolCallDetailCommon.tsx:65`, `mobile/src/components/SessionPermissionsModal.tsx:40` | `height: '80%'` | `maxHeight: '80%'` | 内容が短いときも 80% 分の空白が残り、不必要な高さを取る |
| LOW | `mobile/src/views/AgentSettingsView.tsx:167-168` | `width: 48, height: 36` | `minWidth: 48, minHeight: 36` | 数値入力の固定幅。桁数が増えるとクリップする可能性 |

### 3. Breathing room between targets

| Severity | Location | Before | After | Why |
|---|---|---|---|---|
| LOW | `mobile/src/components/ViewChanger.tsx:31`, `mobile/src/components/NavigationButtons.tsx:33` | `gap: 4` | `gap: 12` | 40x40/36x36 の塗りボタン同士が 4px だと視覚的分離とタップ判定が不安定 |
| LOW | `mobile/src/components/ApprovalPanel.tsx:173`, `mobile/src/views/AgentSettingsView.tsx:145` | `actions: { gap: 8 }` | `actions: { gap: 12 }` | ボタン行の間隔が推奨 12px を下回っている |
| LOW | `mobile/src/components/HabitStepEditor.tsx:74` | `stepHeaderActions: { gap: 6 }` | `stepHeaderActions: { gap: 12 }` | ステップ編集ヘッダーの操作アイコン間隔 |
| LOW | `mobile/src/views/AgentSettingsView.tsx:99` | `row: { gap: 8 }` | `row: { gap: 12 }` | プロバイダー・モデル行の操作ボタン間隔 |

### 4. Content bleeds, controls float / 安全領域

| Severity | Location | Before | After | Why |
|---|---|---|---|---|
| MEDIUM | `mobile/src/components/ContextMenu.tsx:59-61` | `position: 'absolute', top: 60, left: 12` | `top: 56 + insets.top`、 `start: 12`（またはトリガー `onLayout` で位置決め） | 固定 `top: 60` はノッチ端末でステータスバー・トップバーと重なる/被さる。加えて `left` は RTL と安全領域を無視している |
| LOW | `mobile/src/components/NavigationButtons.tsx:30`, `mobile/src/components/ViewChanger.tsx:29` | `right: 8` / `left: 8` | `end: 8 + insets.right` / `start: 8 + insets.left` | 画面端に張り付く浮遊コントロールは安全領域を含めるべき |

### 5. Hint at hidden content

| Severity | Location | Before | After | Why |
|---|---|---|---|---|
| LOW | `mobile/src/components/DateTimePickerModal.tsx:226-229` | 水平 `ScrollView` + `showsHorizontalScrollIndicator={false}`、末尾ピークなし | コンテナ/コンテンツの `paddingEnd` で 16-32px 先を覗かせる、または `showsHorizontalScrollIndicator` を有効化 | ショートカットチップが続いていることが視覚的に分かりにくい |
| LOW | `mobile/src/views/StatsView.tsx:353-356`, `404-407` | 水平 `ScrollView` ×2 + `showsHorizontalScrollIndicator={false}`、末尾ピークなし | 同様に `paddingEnd` またはインジケータを復活 | ヒートマップ・棒グラフが続いていることが分かりにくい |

## 修正時の注意点

- **TaskCard / WorkSessionCard のスワイプジェスチャー**: `right`/`left` の論理化に加え、パンジェスチャー内の `translationX` 判定も `I18nManager.isRTL` に応じて反転させる必要があります。`startSign`（RTL では -1、LTR では 1）を `translationX` に掛けて、start/end 方向を判定し、パネル offset と背景不透明度も同様に反転させます。
- **ComposerRecordButton の cancel ジェスチャー**: `cancelHint` を `start` に配置する場合、`e.translationX` の判定と cancel hint のアニメーション `translateX` も start 方向を向ける必要があります。
- **並列グループの rail**: `borderEndWidth`/`borderEndColor` にする際、全角丸も `border*StartRadius`/`border*EndRadius` に置き換えてください。
- **数値入力 (`countInput`)**: 最低幅を確保しつつ、`paddingHorizontal` で文字が境界に接しないようにしてください。
- **安全領域 insets と `start`/`end` の組み合わせ**: `useSafeAreaInsets` の `left`/`right` は物理方向なので、論理 `start`/`end` には `I18nManager.isRTL` で入れ替えた値を使ってください。
- **ContextMenu のメニュー位置**: `start: 12` のままでは RTL でも start 側に出ますが、トリガー位置に依存する場合は `onLayout` による相対配置がより安全です。

## Verification

- `cd mobile && npx tsc --noEmit` → pass
- `cd mobile && npm run lint` → 0 warnings, 0 errors
- `cd mobile && npm run fmt:check` → all files formatted correctly
- 一時ファイルに `marginStart`/`marginEnd`/`paddingStart`/`paddingEnd`/`start`/`end`/`borderStartColor`/`borderEndColor`/`borderTopStartRadius` などを記述し `npx tsc --noEmit` → pass（論理プロパティ名の型整合性を確認）
- 実機 / エミュレータ / RTL 擬似ロケールでの表示確認は未実施（エミュレータ起動＋日本語以外ロケールでのスクリーンショット比較が推奨）
- カードスワイプの RTL 論理化は `I18nManager.isRTL` を参照して `startSign` で符号を反転し、パネル offset / 不透明度も対応

## Verdict

**Needs changes**

HIGH はなし。RTL への備え・安全領域・テキスト成長に関する MEDIUM が複数、LOW は見た目/密度の微調整です。
