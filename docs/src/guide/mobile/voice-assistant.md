# モバイル音声アシスタント

モバイルアプリでは、画面右下の **`Resident Agent Button`** から音声アシスタントにアクセスできます。

## Resident Agent Button の操作

「Resident Agent Button」はホーム画面に表示されるフローティングボタンです。

- **タップ**：**Agent 画面**（`/agent`）を開く
- **上スライド**：タスク追加画面（`/task/add`）を開く
- **長押し**：未実装

## Agent 画面内の音声入力

「Agent 画面」を開くと、入力欄横のマイクボタンで音声入力できます。

- **長押し**：録音開始
- **上にスライド**：ロック（手を放しても録音継続）
- **左にスライド**：キャンセル
- **ロック後のタップ**：録音停止して認識

録音はユーザーが手動で開始、終了します。
**VAD** は Voice Activity Detection（音声区間検出）の略です。
**barge-in** は、再生中に音声入力で割り込む機能です。
これらは未実装です。

## STT と TTS の設定

設定画面で音声バックエンドを選択できます。

### STT

- Sherpa-ONNX ローカル推論（デフォルト）
- 言語：`ja`、`en`、`zh`、`ko`、`auto`

### TTS

- Cartesia Sonic
- Android 内蔵 TextToSpeech

## Cartesia の設定

設定 → TTS プロバイダーで「Cartesia」を選択し、API キーを入力します。

| 項目 | 値 |
|------|-----|
| Provider | `cartesia` |
| API Key | `CARTESIA_API_KEY` |
| Voice ID | `db6b0ed5-d5d3-463d-ae85-518a07d3c2b4`（デフォルト） |
| Sample Rate | 44100 |

モデルは Cartesia 既定モデル（`sonic-3.5`）を使用します。

Cartesia には無料枠があります。
最新の料金と無料枠については [Cartesia 公式サイト](https://cartesia.ai) を確認してください。

## よく使う音声コマンド例

- 「レポートを 2 時間で始める」
- 「今日のスケジュールは？」
- 「次のタスクを完了した」
- 「明日の朝 7 時にランニングを入れて」

すべての変更は承認後に確定します。
