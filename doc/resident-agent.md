# 常駐型 planner agent

## 背景

現在の Agent は、チャット画面に音声入力(STT)と読み上げ(TTS)を追加した体験になっている。
内部では planner tool、progress tool、変更提案、承認フローが動いているが、ユーザーから見ると「質問すると返答する ChatLLM」が中心であり、takusu 固有の価値である日中の実行支援まで閉じていない。

目標は、Agent を独立したチャット画面ではなく、予定を実行する間ずっと利用できる planner assistant にすることである。

```text
予定を把握する
→ 次の行動を提示する
→ 着手、進捗、完了、遅延を受け取る
→ 必要なら変更を提案する
→ 承認後に予定を更新する
```

対象プラットフォームは Android と Linux desktop の両方である。
ユーザーは日中の大半を desktop で過ごし、外出時に Android を使う。
体験は一つであり、プラットフォームごとに変わるのは surface(後述)とマイクの所有形態だけである。

チャット履歴は詳細確認や自由入力のために残すが、プロダクトの主画面にはしない。

この文書は体験の設計を定める。
実装の work item 分解は、この文書の設計が安定してから `plan/plan-agent.md` と同じ形式で別途行う。
承認まわりの既存 invariant は `plan/plan-agent.md` のものを引き継ぎ、本文書はその上に音声とイベント駆動の体験を定義する。

## 用語

「常駐」には異なる段階があるため、区別して扱う。

1. **常時アクセス可能**: アプリ内のどの主要画面からでも Agent を操作できる。
2. **継続音声セッション**: ユーザーが明示的に開始した後、終了するまで Listen → Act → Speak を繰り返す。
3. **Ambient listening**: 画面外を含め、ユーザーが有効化している間はマイクを継続利用し、呼びかけを端末内で検出する。

最終的には 3 を目指す。
ただし 1 と 2 を先に完成させ、planner の実行ループが有用であることを確認してから導入する。

本文書で使う追加の用語を定義する。

- **Agent surface**: Agent の状態表示と最小操作を提供するプラットフォーム非依存の概念。状態機械、compact panel、通知の三つで構成する。Android では resident button、Linux では system tray icon として実体化する。
- **voice 役**: 複数デバイスのうち、wake word に応答し proactive に発話する権利を持つ一台。多デバイス調停の節で定める。
- **private channel**: 発話が本人にしか聞こえない出力経路。イヤホン(有線、Bluetooth)接続中の Android と、自宅前提の desktop スピーカーを指す。
- **reactive / proactive**: ユーザーが話しかけたことへの応答が reactive、Agent 側から話しかけることが proactive。発話可否の判定はこの二つで異なる。

## Product principles

### Agent は場所ではなく能力

Agent を開くために専用チャット画面へ移動することを必須にしない。
各プラットフォームの Agent surface から、短い操作は現在の文脈のまま完結させる。

- Android: 画面右側の円形 resident button。ドラッグで位置を変更でき、キーボード、modal、承認 sheet、OS gesture 領域を避けられる。タップで compact panel、長押しで Listen。
- Linux: system tray icon。クリックで compact popover、通知はデスクトップ通知として action button 付きで出す。

surface は見た目が違っても一つの Agent session の状態を表示する。
UI surface が切り替わっても recording、turn、TTS、approval の所有者は変わらない。

resident button は暫定の入口である。
本命は ambient listening であり、button は音声を使えない場面と ambient 導入前の受け皿として残す。

### planner state を主役にする

Agent の応答は、原則として自由な assistant message ではなく、planner state と次の action を表示する。

```text
今やること
「レポートを書く」 14:00–15:00

[着手] [完了] [延期] [相談]
```

主な presentation は以下とする。

- current task / next task
- start, pause, progress, complete, delay
- schedule summary
- progress summary(完了数、進行中、実働時間、見積もり対比)
- schedule conflict / overdue alert
- planner change proposal
- focused clarification
- fallback text message

LLM に任意の UI JSON を生成させない。
tool result、schedule state、approval request などの型付きデータからクライアントが表示を決める。
音声出力も同じ presentation 型から定型のテンプレートで生成する。
「今日どこまでやった?」への応答が毎回同じ構造で返ることは、聞き流しやすさと信頼の条件である。

### 行動の閉ループを優先する

見た目を音声アシスタントらしくする前に、次の縦切りを完成させる。

```text
「レポート始める」
→ task_start
→ task が in_progress になる
→ Home / widget / Agent surface に作業中状態が反映される
→ 「30分やって半分終わった」
→ progress と見積もりを更新する
→ 「終わった」
→ task_complete
→ 次の行動または再スケジュールを提示する
```

音声への投資はこの原則に従い二層に分ける。

- **閉ループに必要な最低限**: VAD による発話終了検知(endpointing)、文単位の TTS 再生、タップによる TTS 停止。これがないと「画面を見ずに承認」が成立しないため、閉ループ側の要件として扱う。
- **会話の磨き込み**: barge-in、応答レイテンシの短縮、割り込み後の文脈維持。閉ループが回り始めてから投資する。

active work time は wall-clock time ではなく work session の start / pause から計測する。

### 介入は σ で駆動する

takusu の cost は `NormalDist(avg, sigma)` であり、予定からのズレを分単位の固定閾値ではなく σ 単位で測れる。
固定閾値では、ばらつきの大きいタスクで騒ぎすぎ、正確なタスクで鈍くなる。
介入の強度をズレの z-score に比例させる。

```text
ズレ < 1σ   : 何もしない(ノイズの範囲として扱う)
1σ 〜 2σ    : 進捗を聞きに行く(「思ったより時間がかかってる? どこまで進んだ?」)
> 2σ       : 再計画を提案する(「このペースだと後ろが崩れる。組み直す?」)
```

「進捗を聞きに行く」段階は体験上のマナーであると同時に、estimator にとって情報価値が最も高いタイミングでの観測になる。
進捗報告が入れば見積もりが更新され、σ 自体も締まる。
介入とデータ収集を同じ行為にする。

遅延検知、超過検知、event-driven の発火条件は、原則としてすべてこの物差しに従属させる。

### proactive 発話は private channel に限る

reactive な発話(ユーザーが wake word で話しかけた応答)は常に許可する。
ユーザーが招待した以上、ポケットの中でも応答してよい。

proactive な発話は private channel でのみ行う。

```text
proactive 発話の可否(Android):
  イヤホン接続中          → 発話する
  直近の音声会話の続き     → 発話する(会話コンテキスト内)
  それ以外               → 通知に降格し、スピーカーで勝手に喋らない

desktop: 自宅前提のため原則発話する(quiet hours のみ抑制)
```

センサー(加速度、近接、activity recognition)によるポケット検出や場面推定は行わない。
誤りのコストが非対称だからである。
喋るべき場面で通知に降格しても、quick actions 付きの通知が受け皿になるだけで害はない。
喋ってはいけない場面でスピーカーが喋る事故は、一度で ambient への信頼を失わせる。
確実な信号(イヤホン接続)だけで判定し、安全側に固定する。

「自宅 Wi-Fi ではスピーカー発話を許可する」のような場所プロファイルは、opt-in の拡張としてこの枠内に追加できる。

### 永続的変更は承認を維持する

音声操作や常駐化によって、既存の承認 invariant を弱めない。

- タスク、習慣、スケジュール、永続スキルへの変更は承認前に書き込まない。
- Agent の読み上げに対する曖昧な相槌を承認として扱わない。
- 認識が曖昧な対象、数値、日時は focused clarification を行う。

音声での承認は、誤認識ではなく**誤帰属**(その発話がこの approval に向けられたものか)が主リスクである。
approve / deny の判定自体は閉じた語彙の分類であり、端末内で高精度に行える。
したがって精度を一律に要求するのではなく、誤帰属したときの被害で操作を三層に分ける。

| 層 | 対象 | 確定方法 | 話者照合 |
|---|---|---|---|
| 即時確定 | start, pause(可逆で被害ゼロ) | 確認なしで実行し、結果を読み上げる | 不問 |
| 音声確定 | progress, complete, 単発の作成・変更と読み上げ可能な範囲の玉突き | 変更内容を読み上げた後の明示的な肯定(「いいよ」) | 必須 |
| 画面必須 | delete、schedule 全体の置き換え、影響が読み上げ切れない変更 | 変更内容に固有の応答か、画面での承認 | 画面のため対象外 |

即時確定層は「承認不要」ではなく、既存の permission システムでデフォルト永続許可されている権限クラスとして定義する。
これにより `plan/plan-agent.md` の承認境界と矛盾しない。

**話者照合**(speaker verification)は、登録済みの声紋 embedding と発話の類似度を端末内で判定するゲートである。
「いいよ」の肯定が登録話者の声であることを要求し、他人や TV 音声の相槌が承認になることを防ぐ。
つまり誤帰属リスクへの対策は、内容の側は readback と変更固有の応答が、声の主の側は話者照合が受け持つ。

ただし話者照合は false reject が起きる(風邪声、遠距離、口元にマイクがない)。
本人なのに弾かれる体験は他人に反応するのと同じだけ信頼を削るため、ハードゲートにせずソフト信号として使う。
照合が通らない場合は拒否ではなく画面 fallback に降格し、reactive な応答と即時確定層では照合を要求しない。

曖昧な応答、無応答、timeout の場合は画面に fallback し、surface に waiting_for_approval を灯したまま残す。

## Interaction model

### Agent surface と状態機械

surface は最低限以下の状態を持ち、全 surface で共有する。

```text
idle
listening
transcribing
thinking
waiting_for_user
waiting_for_approval
speaking
error
```

surface の操作で状態遷移を中断できる。

- listening 中のタップ: 録音を確定する。
- thinking 中のタップ: compact panel を開く。
- speaking 中のタップ: TTS を停止する。
- waiting_for_approval 中のタップ: 承認 UI を開く。
- error 中のタップ: 復旧方法を表示する。

Android の resident button はこの状態を色、アイコン、アニメーションで示す。
Linux の tray icon は同じ状態をアイコン差し替えで示す。

### Compact panel

通常の一往復は現在画面上の sheet / overlay(desktop では tray からの popover)で処理する。

- 認識した発話
- Agent が実行中の action
- 結果または次の選択肢
- 必要な承認

過去ログ、tool details、長い相談、セッション切り替えが必要なときだけ full Agent view へ遷移する。
desktop の full view は当面提供せず、必要になった時点で web か別クライアントに委ねる。

### Continuous voice session

ユーザーが surface から明示的に開始した間は、次を繰り返す。

```text
listening
→ transcribing
→ acting
→ speaking
→ listening
```

full-duplex の会話 API は使わない。
コストが高く、ベンダー依存が強すぎるためである。
代わりに、full-duplex の体感を構成する性質を分解し、turn 制のまま音声の入出力層だけを全二重化する。

| 性質 | 実現手段 |
|---|---|
| 低レイテンシ | 端末内 streaming ASR + VAD endpointing で発話終了を数百 ms で検知 |
| incremental response | 文単位の TTS チャンク再生(実装済みの TTS block streaming を使う) |
| barge-in | TTS 再生中もマイクを開き、AEC で自声を除去して VAD を回す |

barge-in の品質は端末の AEC 実装に依存する。
AEC が効かない環境では「TTS 中はタップで停止」に fallback する。

音声入力から始まった turn のみ自動読み上げを行う。
テキスト入力やバックグラウンドイベントは、private channel 原則と緊急度に応じて通知、表示、TTS を選択する。

## 多デバイス調停

desktop と Android の両方が Agent surface を持つと、同じ planner event を両方が喋る、wake word に両方が反応する、という衝突が起きる。
調停は優先度リストと生存確認で機械的に決める。

- デバイス優先度リストを設定として storage に持つ(既定: desktop > Android)。
- 各デバイスの agent service が heartbeat を打ち、生きている中で最上位のデバイスが voice 役になる。
- voice 役以外のデバイスは通知のみに降格する。通知は全デバイスに出す。
- desktop がスリープすれば heartbeat が切れ、Android が自動昇格する。voice 役の切り替えは無言で行う。
- オフライン時は最後に知っている役割を維持する。稀に二台が同時に応答することを許容し、分散合意は実装しない。個人利用の規模では、片方を止める操作の方が安い。

pending approval は現状 AgentSession 内にあり、承認できるのは提案を出したデバイスに限られる。
どの端末でも承認できるようにするかは open question とする(後述)。

## Event-driven assistance

ユーザー発話だけでなく planner event も Agent の入口にする。
介入姿勢は積極側に倒す。
黙って予定が崩れるより、早めに声をかけて小さく直す方が planner の価値が出る。
ただし発話するかどうかは private channel 原則に、発火するかどうかは σ 駆動の原則に従う。

対象イベント:

- task の開始時刻
- task の終了予定超過(σ 換算)
- deadline 違反の予測
- schedule gap
- 未完了タスクの持ち越し
- schedule 未生成
- 睡眠時間への影響

イベントは直接 LLM turn を起動するとは限らない。
決定的に生成できる通知や action はアプリ側で生成し、曖昧な調整、説明、提案が必要な場合だけ Agent を呼ぶ。

```text
「レポートの開始時刻です」
[着手] [10分後] [組み直す]
```

追加の振る舞いを定める。

- **先送りの理由を一問だけ聞く**: ユーザーがタスクを延期したとき、「なにか詰まってる?」と一度だけ聞く。理由(ブロック中、気が重い、単に時間がない)は再配置の判断材料になる。答えなければ追わない。
- **無応答は静かに降格する**: proactive な問いかけに返事がなければ、聞き直さず通知に降格する。
- **quiet hours**: 就寝時刻(sleep 設定から導出)以降は voice も通知も停止する。例外とする「緊急」の定義は open question とする。

## Ambient listening

### Target behavior

opt-in 設定を有効にしている間、Agent service がマイクを継続利用する。
マイク使用中であること、停止方法、現在の状態を常にユーザーへ示す。

常時 Listen は「すべての音声を常にクラウド LLM へ送信する」ことを意味しない。
処理は段階的なゲートになっており、後段ほど重い処理を、前段が通ったときだけ起動する。

```text
マイク音声(常時)
→ VAD: 発話区間の検出のみ。極小モデルで CPU 負荷はほぼゼロ
→ KWS(keyword spotting): 決めたフレーズが鳴ったかの二値判定のみ。数 MB のモデルで常時稼働できる
→ 話者照合: KWS を通った発話だけを声紋 embedding で判定。常時稼働しない
→ (ここで初めて) streaming ASR を起動して発話全体を文字起こし
→ (ここで初めて) LLM turn。クラウド課金はここからのみ発生
→ tool execution / proposal
→ TTS or notification
```

常時稼働するのは VAD と KWS だけであり、電池とクラウドコストの問題はこの二つの軽さに還元される。
KWS は任意の発話を文字にする能力を捨て、特定の音パターンの検出に特化しているため、streaming ASR より一桁軽い。
話者照合の使い方(承認層ごとの要否と false reject 時の降格)は音声承認の節で定めたものに従う。

呼びかけは wake word を本命とする。
task-related utterance を端末内分類で拾う wake-word-less mode は、作業セッション中(task が in_progress の間)に限定した格上げとして将来検討する余地を残すが、初期実装には含めない。

### マイクの所有と常駐の段階

「常時」はマイクを所有するプロセスの寿命で決まる。

1. **アプリ画面内のみ**: UI がマイクを持つ。現状の push-to-talk。
2. **アプリプロセス生存中**: Android では microphone foreground service、Linux では tray デーモンが持つ。画面を離れてもロック中でも聞き続ける。
3. **端末起動中**: boot 時に service を自動起動する。

ambient の単位はアプリではなく service とする。
2 と 3 の差はマイク処理の実装ではなく起動契機だけなので、boot 自動起動は独立した opt-in 設定にする。
体験としての本命は 3 であり、2 はその検証段階である。

Expo / React Native の component lifecycle に常時録音を所有させない。
platform shell(Android native service / Linux デーモン)が録音と lifecycle を所有し、Rust が VAD、KWS、ASR、Agent session を担当する。
JS ほか UI 層は状態表示とユーザー操作の購読に徹する。

### desktop を実験場にする

ambient の制約はプラットフォーム間で非対称である。
Android には foreground service、background start 制限、電池、発熱、AEC の機種差がすべてあるのに対し、Linux desktop にはどれもない。
常駐は普通のプロセス、電源は据え置き、マイクは PipeWire で安定している。

したがって ambient は「Android で頑張ってから移植」ではなく、**desktop の tray デーモンを実験場にして、ゲート設計、誤発火率、会話 UX を先に検証し、Android には確立したものを載せる**。

### Privacy and safety boundaries

- 初期状態は無効にする。
- 有効化時にマイク継続利用、処理範囲、外部送信条件を説明する。
- マイク使用中は Android の foreground notification とアプリ内表示、desktop では tray 表示を消せない形で出す。
- 通知と Agent surface の両方から即時停止できるようにする。
- wake gate より前の raw audio は永続化しない。
- rolling buffer が必要な場合はメモリ上に短時間だけ保持し、対象外発話を破棄する。
- raw audio、transcript、機密情報をログへ記録しない。
- 声紋 embedding は端末内にのみ保存し、外部送信しない。削除は設定から即時にできる。
- lock screen 中、通話中、他アプリの録音中、battery saver 中の動作を定義する。
- LLM、TTS、ネットワーク障害時も録音状態を曖昧にしない。
- 常時 Listen から planner mutation を直接確定しない(音声承認の三層に従う)。

### プラットフォーム制約

Android の ambient listening は microphone foreground service を前提とする。
永続通知、foreground service type、マイク権限、近年の background start 制限へ対応する。
OS 統合を深める段階で `VoiceInteractionService` の採用可否を検討するが、最初から必須にはしない。

Linux は systemd user service として常駐し、PipeWire からマイクを取得する。
tray、通知、popover の実装は実装計画の段階で選定する。

## Architecture

Agent の状態は surface ごとに重複して持たず、共有の Rust core に集約する。
プラットフォームごとに変わるのはマイクの所有と surface の実体だけである。

```text
[surface 層]
Android UI                      Linux desktop
├── ResidentAgentButton         ├── tray icon
├── AgentCompactPanel           ├── compact popover
├── ApprovalSheet               └── desktop notification (actions 付き)
├── FullAgentView
└── 通知 (actions 付き)
        ↕ 状態購読 / コマンド          ↕ 状態購読 / コマンド

[platform shell]
Android AgentService            Linux デーモン (systemd user service)
├── AudioRecord lifecycle       ├── PipeWire capture
├── foreground notification     ├── tray 常駐
└── audio focus / 電源状態       └── セッション / 電源状態
        ↕ PCM / 状態                   ↕ PCM / 状態

[共有 Rust core]
├── VAD / denoise / KWS / 話者照合 / ASR
├── AgentSession(状態機械、turn、approval)
├── planner / progress tools
├── event 検知(σ 駆動)
├── 多デバイス調停(heartbeat、voice 役)
└── TTS adapter
```

## Rollout

### Phase 1: planner execution loop の UI

progress の storage、API、Agent tools は実装済みである。
残りは体験側に閉じる。

- current task card と quick actions
- structured presentation の最小型(current/next task、progress summary)
- Home、widget、Agent UI の状態同期

### Phase 2: 共通層と薄い surface

surface を片方ずつ厚く作るのではなく、共有層を先に固め、両プラットフォームの surface は薄く同時に置く。

- Rust core への session state 集約(状態機械、surface protocol)
- Android: resident button の全画面化(draggable)、compact panel
- Linux: tray icon、compact popover、actions 付き通知
- planner event notification と deep link
- 多デバイス調停(優先度リスト、heartbeat、voice 役)

### Phase 3: voice loop

- 閉ループに必要な最低限: VAD endpointing、TTS 停止、modality-aware response
- 音声承認の三層
- event-driven の発話(σ 駆動、private channel 原則、先送り理由の聴取)
- 磨き込み: barge-in、レイテンシ予算、interruption / timeout / error recovery

### Phase 4: ambient listening

- desktop tray デーモンでの opt-in ambient(VAD → KWS → ASR ゲート)
- wake word の実機評価(誤発火率、日本語 KWS の成否)
- Android: microphone foreground service、永続通知と即時停止、boot 自動起動 opt-in
- privacy、電池、発熱、false positive の実機評価
- background / lock screen lifecycle
- 必要に応じて VoiceInteractionService を評価

## Non-goals

- UI へ波形を追加するだけで問題を解決したことにしない。
- chat bubble の見た目だけを変えない。
- planner lifecycle がない状態で hotword 対応を先行しない。
- raw audio を常時クラウドへ送らない。
- LLM に任意の UI や承認対象を生成させない。
- 常時 Listen をデフォルトで有効にしない。
- full-duplex の会話 API を採用しない。
- センサーによる場面推定(ポケット検出、activity recognition)を v1 で行わない。
- 多デバイス間の分散合意を実装しない。
- desktop の full Agent view / planner 全体 UI を作らない(web と CLI の再設計は別トラック)。

## Open questions

- **承認の可搬性**: pending approval を storage に持ち上げ、提案を出したデバイス以外でも承認できるようにするか。session 内で閉じる現設計との整合が必要。
- **wake word の実フレーズと日本語 KWS**: sherpa-onnx の KWS 事前学習モデルは中国語と英語が中心で、日本語フレーズがそのまま動くかは実機検証が要る。代替は wake word 自作学習(openWakeWord 系)、または desktop 限定で streaming ASR 常時稼働 + テキストマッチ。
- **ローカル TTS への移行基準**: 当面 Cartesia を使う。日本語品質と低レイテンシが NVIDIA GPU なしの CPU で成立するローカル TTS が現れたら proactive 発話から移行する。候補(VOICEVOX、sherpa-onnx VITS 系、Kokoro など)を実装計画の段階で評価する。
- **話者照合の閾値と enrollment UX**: 何発話で声紋を登録するか、類似度閾値をどこに置くか、声の経年変化や環境差にどう追随するか(再登録の契機)。モデルは sherpa-onnx の speaker embedding(WeSpeaker / 3D-Speaker 系)を第一候補として実機評価する。
- **先送り理由の保存先**: 聴取した理由を memory に保存するか、その場の再配置判断に使って捨てるか。
- **quiet hours の「緊急」定義**: 就寝後も通す例外イベントの範囲。
- **場所プロファイル**: 「自宅ではスピーカー発話を許可」のような opt-in 拡張の要否。

## Success criteria

- ユーザーが full Agent view を開かずに着手、進捗、完了、延期を行える。
- Agent の結果が Home、schedule、widget、全 surface に即時反映される。
- 一つの明示的な音声セッション内で複数 turn を継続できる。
- σ の範囲内のズレで Agent が騒がない。σ を超えたズレでは進捗確認か再計画提案が届く。
- イヤホンなしの Android がスピーカーで proactive に発話しない。
- 同じ event に対して voice で応答するデバイスが常に一台である。
- ambient listening の開始、稼働、停止が常に視認できる。
- 対象外音声が外部送信も永続化もされない。
- planner mutation はすべて三層の承認境界を維持する。
- 登録話者以外の声で音声確定層の変更が確定しない。

## 付録: canonical scenario

体験の基準として、平日の一日を台本で示す。
presentation 型、event 発火、承認層の仕様は、この台本の各場面と矛盾しないように定める。
前提: desktop(tray 常駐、ambient 有効)と Android(優先度 2 位)、wake word は仮に「たくす」。

```text
■ 07:30 起床 habit の時刻。desktop はスリープ → Android が voice 役
  イヤホン未接続のため発話せず、Android 通知:
  「おはよう。今日は7件、まず 9:00 レポート。睡眠は予定通り。」
  [今日の予定] [組み直す]

■ 08:55 desktop に着席、スリープ解除。voice 役が無言で desktop に戻る

■ 09:00 タスク開始時刻
  desktop(発話): 「9時になった。レポート、始める?」
  ユーザー: 「始める」
  agent: 「開始。11時まで。」(start は即時確定層。確認なし)
  → Home / widget / tray が in_progress 表示に変わる

■ 09:40 独り言「あーこれ違うな」→ wake word なし、KWS を通らず何も起きない
  (raw audio は破棄され、ログにも残らない)

■ 10:20 ユーザー: 「たくす、演習半分終わった」
  agent: 「15題完了だね。ペースは見積もりから 0.5σ 遅れ。このままでいこう。」
  (1σ 未満のズレでは再計画を持ちかけない)

■ 11:10 実働時間が見積もりを 1σ 超過
  desktop(発話): 「思ったより時間かかってる? いまどのへん?」
  ユーザー: 「あと5題」
  agent: 「なら残り20分くらい。11時半までに伸ばして、昼を10分ずらす。いい?」
  ユーザー: 「いいよ」(音声確定層: 読み上げ済みの複合変更を肯定で確定)
  agent: 「更新した。」

■ 11:45 次のタスクの開始が 1σ 相当遅延
  desktop(発話): 「『申請書を出す』が押してる。今から10分でやる? 午後に回す?」
  ユーザー: 「午後に回して」
  agent: 「なにか詰まってる? それとも時間だけ?」(先送り理由を一問だけ聞く)
  ユーザー: 「窓口の返事待ち」
  agent: 「了解、ブロック中として 15:00 に置き直す。返事が来たら教えて。」

■ 12:40 ユーザー: 「たくす、来週の火曜に歯医者いれて。3時」
  agent: 「来週火曜 15:00『歯医者』、移動込みで90分確保。
          『買い出し』が金曜に動く。いい?」
  ユーザー: 「うん」(音声確定層: 玉突きが読み上げ可能な範囲)
  agent: 「作成した。」

■ 14:00 agent の発話中にユーザーが「たくす、ちがう」
  → barge-in。TTS を停止して聞き直す(Phase 3 の磨き込みまではタップ停止で代替)

■ 15:00 外出。desktop の heartbeat が切れ、Android が voice 役に昇格
  イヤホン未接続のため、ポケットの中では発話せず通知のみ:
  「『申請書を出す』の時間。」 [着手] [15分後] [相談]

■ 17:30 帰宅、desktop 復帰
  ユーザー: 「たくす、今日どこまでやった?」
  agent: 「完了3件、進行中1件。レポートは60%、実働2時間10分。
          見積もりより速いペース。」(progress summary presentation)

■ 21:50 就寝 habit の 40 分前
  agent(発話): 「今日はここまでにする? 『メール返信』だけ残ってるけど、
               明日の朝いちに置いておく?」
  ユーザー: (返事なし)
  → 聞き直さず通知に降格

■ 22:30 就寝時刻以降は voice も通知も停止(緊急の定義は open question)
```
