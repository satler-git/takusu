# 常駐型 planner agent

## 背景

takusu の目的は、スケジューリングをユーザーの頭から忘れさせ、より価値の高い時間を過ごせるようにすることである(`proposal.typ`)。
予定の把握、次の行動の判断、ズレたときの組み直しを takusu が引き受け、ユーザーは「今やること」を見るだけでよい状態を目指す。
本文書ではこの状態を**実行機能の外部化**と呼ぶ。

`proposal.typ` は、この目的を阻む最大の敵も名指ししている。
入力を怠ると自動構築された予定の価値が下がり、価値が下がるとさらに入力しなくなる、という**負のループ**である。
takusu の planner は現実のすべての予定が登録されていることを前提に設計されており、この前提は緩和できないし、実行機能の外部化のためには緩和すべきでもない。
知らない予定を避けてスケジュールを組むことはできず、現実を半分しか知らない計画に「今やること」を任せることもできないからである。
したがって、計画と現実の同期が切れることは品質の低下ではなく、プロダクトの死を意味する。

現在の Agent は、チャット画面に音声入力(STT)と読み上げ(TTS)を追加した体験になっている。
内部では planner tool、progress tool、変更提案、承認フローが動いているが、ユーザーから見ると「質問すると返答する ChatLLM」が中心であり、同期を維持する仕事を担えていない。

目標は、Agent を独立したチャット画面ではなく、計画と現実の同期を維持し続ける planner assistant にすることである。

対象プラットフォームは Android と Linux desktop の両方である。
ユーザーは日中の大半を desktop で過ごし、外出時に Android を使う。
体験は一つであり、プラットフォームごとに変わるのは surface(後述)とマイクの所有形態だけである。

チャット履歴は詳細確認や自由入力のために残すが、プロダクトの主画面にはしない。

この文書は体験の設計を定める。
実装の work item 分解は `plan/resident-agent.md` が担い、本改訂から再導出済みである。
承認まわりの既存 invariant は `plan/plan-agent.md` のものを引き継ぎ、本文書はその上に同期とイベント駆動の体験を定義する。

## 前提とするユーザー

本文書は、外部からの声かけが実行の助けになるユーザーを前提とする。
作者自身がそうであり、予定の時刻に agent から接触されることを歓迎し、接触がないことよりも接触が選択肢を持たないこと(後述)を苦痛と感じる。
接触自体を煩わしいと感じるユーザーへの一般化は本文書の範囲外である。

## 用語

「常駐」には異なる段階があるため、区別して扱う。

1. **常時アクセス可能**: アプリ内のどの主要画面からでも Agent を操作できる。
2. **継続音声セッション**: ユーザーが明示的に開始した後、終了するまで Listen → Act → Speak を繰り返す。
3. **Ambient listening**: 画面外を含め、ユーザーが有効化している間はマイクを継続利用し、呼びかけを端末内で検出する。

最終的には 3 を目指す。
ただし 1 と 2 を先に完成させ、同期のループが有用であることを確認してから導入する。

本文書で使う追加の用語を定義する。

- **Agent surface**: Agent の状態表示と最小操作を提供するプラットフォーム非依存の概念。状態機械、compact panel、通知の三つで構成する。Android では resident button、Linux では system tray icon として実体化する。
- **check-in**: agent からユーザーへの、現在の行動を確かめる一往復の接触。「今なにしてる?」「始める?」「どこまで進んだ?」の形をとる。
- **精算**: 既に過ぎた時間の使途を、遡って計画と記録に反映させる操作。
- **intake**: 使い始めに現実の予定を集中的に聞き取る、インタビュー形式の capture。
- **resident authority**: 複数デバイスのうち、planner event を評価して event ledger に確定する一台。microphone service の有無とは独立する。
- **SpeechCapability**: resident authority が現在 private channel で proactive speech を実行できる状態。失われた場合は event evaluation を続け、delivery を notification に降格する。
- **private channel**: 発話が本人にしか聞こえない出力経路。イヤホン(有線、Bluetooth)接続中の Android と、自宅前提の desktop スピーカーを指す。
- **reactive / proactive**: ユーザーが話しかけたことへの応答が reactive、Agent 側から話しかけることが proactive。発話可否の判定はこの二つで異なる。

## 三つのループ

同期を維持するために、agent は三つのループを回す。

```text
capture: 現実 → 計画
  新しいタスクや予定が、一言の申告から十分なデータを伴って takusu に入る

sync: ズレの検出 → 調整
  計画と現実が乖離したとき、即座に接触し、行動させるか計画を現実に追従させる

execution loop: 着手 → 進捗 → 完了
  作業中のタスクの状態を追い、所要時間分布から逸脱したときに介入する
```

三つは独立ではない。
sync の接触で「takusu が知らない活動」が見つかれば、それは capture の入力になる。
execution loop の進捗報告は estimator を更新し、総所要時間の事後分布が締まることで sync と execution loop の介入精度が上がる。
つまり、同期を維持する行為そのものが、takusu を賢くするデータ収集を兼ねる。

旧版の本文書は execution loop だけを設計していた。
しかし実際の失敗は execution loop の中(着手したタスクの超過)よりも手前で起きる。
タスクがそもそも登録されない、登録された計画と違うことをしている、という capture と sync の失敗である。
execution loop は三つのうち最後に効くループであり、最初に効くループではない。

## Product principles

### Agent は場所ではなく能力

Agent を開くために専用チャット画面へ移動することを必須にしない。
各プラットフォームの Agent surface から、短い操作は現在の文脈のまま完結させる。

- Android: 画面右側の円形 resident button。ドラッグで位置を変更でき、キーボード、modal、承認 sheet、OS gesture 領域を避けられる。タップで compact panel、長押しで Listen。
- Linux: system tray icon。クリックで compact popover、通知はデスクトップ通知として action button 付きで出す。

同じ device 上の surface は、見た目が違っても一つの AgentSession の状態を表示する。
同じ device 内で UI surface が切り替わっても recording、turn、TTS、approval の所有者は変わらない。
別 device は独立した AgentSession を持ち、user-scoped な planner state だけを共有する。

resident button は暫定の入口である。
本命は ambient listening であり、button は音声を使えない場面と ambient 導入前の受け皿として残す。

### check-in を接触の原子にする

agent からユーザーへの接触は、原則として check-in の形をとる。
同じ一つの問いが、状況によって別のループに給餌する。

| 状況 | check-in の意味 | 給餌先 |
|---|---|---|
| 未分類 gap、または計画と違う明確な気配 | 「今なにしてる?」その活動を登録するか | capture |
| 開始時刻を過ぎて着手記録がない | 「始める? ズラす?」 | sync |
| 着手済みタスクが所要時間分布の注意範囲へ入る | 「どこまで進んだ?」 | execution loop |

check-in への応答は一言で完結しなければならない。
答えを受けた agent は、既知タスクへの照合、登録候補化、ズラし提案のいずれかに一往復で落とす。
分類や登録のために長い問答を続けることは、check-in を尋問に変える。

check-in はデータ収集であると同時に、時間感覚への外部アンカーでもある。
「今 14 時で、予定ではレポートの時間」という接触は、応答がなくても現在地を思い出させる価値を持つ。

### 接触は必ず「行動」と「ズラす」を差し出す

計画とズレているとき、agent が取れる行動は二つしかない。
ユーザーを計画に引き戻すか、計画を現実に追従させるかである。
前者だけを差し出す接触は nag であり、ミュートされて同期ごと死ぬ。

したがって、ズレに対するすべての接触は「行動する」と「ズラす」の両方を等コストの選択肢として含む。

```text
「9時になった。レポート始める?」
[着手] [30分後] [午後に回す] [相談]
```

この原則の帰結として、正直の申告は常に安くなければならない。
「ゲームしてた、あと 30 分で切り上げる」という白状が一言で計画に反映されるなら、ユーザーは隠す理由を失う。
説教、評価、詰問は行わない。
agent の関心は「なぜ守らなかったか」ではなく「計画をどう現実に合わせるか」にある。

無視は常に無料である。
proactive な接触に応答がなければ通知に降格し、追わない。
「2 時間ほっといて」の一言で接触を止められる。

### planner state を主役にする

Agent の応答は、原則として自由な assistant message ではなく、planner state と次の action を表示する。

```text
今やること
「レポートを書く」 14:00–15:00

[着手] [進捗] [完了] [延期] [相談]
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

### 着手後の介入は所要時間分布で駆動する

takusu の cost は所要時間の事前分布 `NormalDist(avg, sigma)` である。
ただし、「σ のズレ」を一つの計算式で扱わない。
観測できる情報に応じて、異なる確率変数と観測を使う。

- **進捗なしの超過**: 確率変数はタスクの総所要時間 `T`、観測は `T > active_elapsed` という右打ち切り観測である。単純な `(active_elapsed - avg) / sigma` だけではなく、現時点で未完了である条件を反映した事後分布から介入強度を決める。
- **進捗ありのペース逸脱**: 確率変数は進捗から予測した総所要時間、観測は active work time と quantity の完了率である。更新後の総所要時間分布が事前分布からどれだけ遅い側へ移動したかで介入強度を決める。
- **未着手の開始遅延**: 所要時間分布の観測ではないため σ と呼ばない。開始時刻イベントと sync の check-in が受け持つ。

各イベント定義は、対象となる確率変数、観測値、事前分布または更新後分布、閾値を必ず一組で定める。
`sigma = 0` の固定予定は所要時間の逸脱判定から除外し、未着手と終了予定の規則だけを使う。
履歴のないタスクは、タスク種別から得た事前分布を使い、事前分布も作れない場合は明示した固定時間の fallback を使う。

介入強度は次の三段階に正規化する。

```text
通常範囲       : 何もしない
注意範囲       : 進捗を聞く(「思ったより時間がかかってる? どこまで進んだ?」)
再計画範囲     : 再計画を提案する(「このペースだと後ろが崩れる。組み直す?」)
```

進捗を聞くことは、体験上のマナーであると同時に estimator への観測になる。
進捗報告が入れば総所要時間の事後分布が更新され、その後の介入精度が上がる。
同じタスクと同じ分布 revision に対する閾値発火は一度だけとする。
進捗報告によって注意範囲を下回った後、再び超えた場合は、分布 revision が変わっていれば新しい発火として扱う。

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

- 永続的なタスク、習慣、スケジュール、永続スキルへの変更は承認前に書き込まない。
- work session の start / pause と短時間の snooze だけは、server-issued one-shot capability を検証した即時確定層の可逆操作として扱う。capability の検証は承認境界の外側にある無条件の API bypass ではない。
- Agent の読み上げに対する曖昧な相槌を承認として扱わない。
- 認識が曖昧な対象、数値、日時は focused clarification を行う。

音声での承認は、誤認識ではなく**誤帰属**(その発話がこの approval に向けられたものか)が主リスクである。
approve / deny の判定自体は閉じた語彙の分類であり、端末内で高精度に行える。
したがって精度を一律に要求するのではなく、誤帰属したときの被害で操作を四層に分ける。

| 層 | 対象 | 確定方法 | 話者照合 |
|---|---|---|---|
| 即時確定 | 画面、通知 action、または明示的に開始した continuous session 内の start / pause / 数十分の snooze | 確認なしで実行し、結果を提示する | continuous session では開始時に本人性を確立 |
| ambient 即時確定 | wake word から始まった start / pause | 確認なしで実行し、結果を読み上げる | 必須。失敗時は画面 fallback |
| 音声確定 | progress, complete, 単発の作成・変更と読み上げ可能な範囲の玉突き | 変更内容を読み上げた後の明示的な肯定(「いいよ」) | 必須 |
| 画面必須 | delete、schedule 全体の置き換え、影響が読み上げ切れない変更 | 変更内容に固有の応答か、画面での承認 | 画面のため対象外 |

start と pause は簡単に戻せるが、被害ゼロではない。
work session、task status、estimator の観測、current task、後続の介入時刻を変更するためである。
したがって入力経路を承認層の判定に含め、ambient では誤帰属を防ぐ。
数十分の snooze も同様に可逆で被害が小さいため即時確定層に含める。
時間帯をまたぐ延期は通常の承認対象である。
これがないと「ズラす」の一操作性(接触の原則)が承認 invariant と両立しない。

即時確定層は「承認不要」ではなく、サーバーが発行した event、device、action、期限付きの one-shot capability を検証した後に、既存の permission system でデフォルト許可される権限クラスとして定義する。
クライアントが入力経路を自己申告しても即時確定にはならない。
誤った start / pause を undo した場合は work session だけでなく estimator に渡った観測も、単調増加する revision lineage の補償観測で取り消す。
これにより `plan/plan-agent.md` の承認境界と、quick action の一操作性を両立する。

**話者照合**(speaker verification)は、登録済みの声紋 embedding と発話の類似度を端末内で判定するゲートである。
「いいよ」の肯定が登録話者の声であることを要求し、他人や TV 音声の相槌が承認になることを防ぐ。
つまり誤帰属リスクへの対策は、内容の側は readback と変更固有の応答が、声の主の側は話者照合が受け持つ。

ただし話者照合は false reject が起きる(風邪声、遠距離、口元にマイクがない)。
本人なのに弾かれる体験は他人に反応するのと同じだけ信頼を削るため、ハードゲートにせずソフト信号として使う。
照合が通らない場合は拒否ではなく画面 fallback に降格する。情報取得だけの reactive な応答と、画面や通知 action からの即時確定では照合を要求しない。

曖昧な応答、無応答、timeout の場合は画面に fallback し、surface に waiting_for_approval を灯したまま残す。

## capture の体験

### 日常の capture

タスクの登録は一言で始まる。

```text
「たくす、演習30題追加。金曜まで」
```

不足している情報(見積もり、quantity、開始可能時間)は agent が補完する。
メモリーから過去の類似タスクを参照し、根拠付きの見積もりを作り、承認画面または readback で提示する。
どうしてもデータが足りない場合だけ、focused clarification で一問聞く。
「十分なデータを伴った登録」の負担はユーザーではなく agent が持つ。

登録という行為自体の重さ(アプリを開く、フォームを埋める)を、voice の一言に置き換えることが capture の中心的な価値である。

### intake

使い始めのユーザーは、現実の予定を大量に登録しなければ takusu が機能しない、という壁に直面する。
この壁を一括入力のフォームで越えさせるのではなく、agent がインタビューする体験として設計する。

- agent 側が聞く順序を持つ。締め切りが決まっているもの、定期的に繰り返すもの、カレンダーからの import 確認、の順に想起を促す。
- ユーザーは思いつくままに喋るだけでよい。構造化、見積もり、quantity の補完は日常の capture と同じく agent が行い、まとめて承認に出す。
- 一回のセッションは 10〜15 分で中断でき、いつでも再開できる。完了を要求しない。

intake で取り切れなかった予定は、日々の sync が拾う(次節)。
ただし、不完全な計画を権威ある「今やること」として表示してはならない。
intake の目標は、まず今日の固定予定と目前の締め切りを確認し、今日について限定された信頼を作ることである。

### coverage と計画への信頼

全予定が必要という前提と、intake を中断できる体験は、計画の信頼状態を明示することで両立させる。

- **bootstrap**: 把握範囲が狭い。current task は「今やること」ではなく候補として表示し、不足している予定の intake を促す。
- **today-covered**: 今日の固定予定、締め切り、既知の割り込みを確認済みである。今日に限って current task を「今やること」として提示できる。
- **trusted**: intake と継続的な sync により、対象期間の生活を planner に預けられる。
- **stale**: 最後の確認から期間が空いた、未精算の時間が残る、または calendar sync が失敗した。current task の権威を下げ、精算を先に提示する。

信頼状態は、単に登録件数から推測しない。
どの期間を最後に確認したか、未分類 gap と未精算時間が残っているか、外部 calendar が同期済みかという観測可能な条件から決める。
ユーザーが申告していない現実を検出できない以上、`trusted` は完全性の証明ではなく、確認手順を通過した状態である。

### sync による coverage の成長

計画と現実がズレる原因は二つに分かれる。
ユーザーが計画を避けているか、takusu が知らない現実があるかである。
後者の場合、sync の精算はそのまま capture の入口になる。

```text
agent: 「今なにしてる?」
ユーザー: 「バイトの引き継ぎ資料つくってる」
agent: 「登録する?」
        [今回だけ] [毎週火曜] [自由時間] [相談]
ユーザー: 「毎週火曜」
agent: 「毎週火曜の定期タスクとして提案した。承認する?」
```

最初の check-in は一問と一回答で分類を尋問しない。
登録の承認は分類とは別の authorization step として扱う。

全予定の登録という前提は、初回の一括登録で満たすものではなく、この経路で漸進的に満たし維持するものである。

## sync の体験

### ズレの検出と接触

sync が扱うズレは三種類ある。

- **未着手**: 開始時刻を過ぎたのに着手記録がない。
- **未分類 gap**: planner が自由時間、buffer、routine として説明できない空白が続いている。
- **計画外の現実**: takusu が知らない予定や割り込みが発生した(ユーザー側からの申告で判明することが多い)。

schedule 上の空白を一種類にまとめない。

- **自由時間**: 意図した休息や余暇であり、check-in しない。
- **buffer**: タスクの不確実性に備えた余白であり、check-in しない。
- **routine**: 食事、移動、身支度など、毎回個別タスクへする必要がない既知の活動であり、必要なら開始時刻だけを案内する。
- **未分類 gap**: planner が理由を説明できない空白であり、capture の check-in 対象になる。
- **生成失敗**: task があるのに配置できなかった状態であり、check-in ではなく planner error と再計画を提示する。

接触のタイミングは早い方がよいが、不要な接触にも認知コストがある。
ズレの兆候があることに加え、未分類 gap または未着手という明確な根拠がある場合だけ接触する。
接触は「行動」と「ズラす」を差し出す(Product principles)。

接触後の振る舞いは引き下がりを基本とする。

- 応答がなければ通知に降格し、聞き直さない。
- 降格後の再接触は、次のイベント(次のタスクの開始時刻など)まで行わない。同じズレについて接触を繰り返してエスカレーションしない。
- 無応答が続いた時間帯では、その日の残りの check-in 頻度を下げる。
- 一日の proactive check-in には上限を設ける。開始時刻や deadline の通知は上限に含めず、未知活動を尋ねる check-in だけを制限する。
- 「ほっといて」の指示は期間付きの接触停止として即座に効く。

### 精算

ズレたまま時間が過ぎた後、ユーザーが戻ってきた時点で精算を行う。

```text
ユーザー: 「たくす、ごめん今までゲームしてた」
agent: 「おかえり。9時からの3時間はゲームとして記録しておくよ。
        午後で組み直すと、レポートが13時、申請書が16時、演習は明日の朝。いい?」
ユーザー: 「いいよ」
agent: 「更新した。13時からレポート。」
```

精算の設計原則:

- 白状は一言で済む。過ぎた時間の記録と、残りの計画の組み直しを agent が一度の提案にまとめる。
- 過ぎた時間をどう使ったかは記録するが、評価しない。記録は estimator と将来の計画(この時間帯は集中しにくい、など)への入力である。
- ユーザーが精算を申告しなくても、次の check-in や一日の終わりに agent 側から精算を持ちかける(「さっきの2時間はどうしておく?」)。

### 悪い日の成功条件

agent の成功は「計画がすべて実行されたこと」ではない。
**一日の終わりに、計画が現実を反映した状態に戻っていること**である。

半日がゲームで溶けても、その時間が記録され、残ったタスクが現実的な形で翌日以降に再配置されていれば、agent は仕事をしたことになる。
逆に、全タスクが未処理のまま stale な「今やること」を表示し続ける状態は、たとえ接触を一切しなかったとしても失敗である。
stale な計画は「今やること」表示の権威を殺し、実行機能を預ける先としての信頼を壊すからである。

数日間 takusu に触らなかった後の再開も、同じ原則で扱う。
溜まった持ち越しの山を一件ずつ突きつけるのではなく、一度の精算(「この3日の分はまとめて組み直すよ。生きてる締め切りはこれとこれ」)で計画を現実に戻す。

## Interaction model

### Agent surface と状態機械

surface は最低限以下の状態を持ち、同じ device の全 surface で共有する。

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

desktop と Android の両方が Agent surface を持つと、同じ planner event を両方が検知して発話する、wake word に両方が反応する、という衝突が起きる。
調停は優先度リストと生存確認で機械的に決める。

- デバイス優先度リストを設定として storage に持つ(既定: desktop > Android)。各デバイスは自分の `takusu-local` または埋め込み相当の API host を使い、planner state と event ledger は共通 backend で共有する。独立した SQLite 間の同期は行わない。
- 各デバイスの application host が evaluator heartbeat または evaluator lease を更新し、生きている中で最上位のデバイスが **resident authority** になる。desktop は host の稼働中に heartbeat を更新し、Android は次の exact alarm と grace period までの lease を予約して、alarm 実行時に lease を更新または再取得する。Android は resident role のためだけに高頻度 alarm を予約しない。
- resident authority は planner event の評価と ledger への確定を担う。microphone service の有無は authority を変えない。
- microphone service の状態と private output route は **SpeechCapability** として別に扱う。resident authority が speech capability を持たないときは、評価と通知を続け、proactive speech だけを notification に降格する。
- resident authority 以外のデバイスは、ledger から確定済み event を replay し、device-specific delivery claim に従って通知する。自分で同じ event を再評価して確定してはならない。
- desktop がスリープすれば evaluator heartbeat が切れ、Android が自動昇格する。resident authority の切り替えは無言で行う。audio heartbeat の切断は speech capability の喪失であり、authority の喪失ではない。
- オフライン時は最後に知っている役割を維持する。ただし shared backend に再接続して snapshot と heartbeat lease を検証するまで、新しい event を ledger に確定しない。partition 中の重複検知は許容し、再接続後は ledger と stable operation ID で一つに収束させる。分散合意は実装しない。

resident authority だけでは crash retry と partition 後の重複を処理できないため、event を storage の ledger に記録する。
event ID は event kind、対象 task または gap interval、canonical boundary、schedule revision、分布 revision、observation kind から決定的に作る。
ledger は immutable presentation payload、snapshot revision、delivery state、capability、device claim、mutation operation ID を保持する。
通常接続時は event ID の一意制約だけでなく、lease と snapshot revision を transaction 内で検証し、再送と再接続後の merge でも同じ mutation を重複実行しない。

状態の所有範囲は次のように分ける。

- **user-scoped**: planner state、coverage、planner event、提案内容。
- **session-scoped**: turn、会話履歴、pending approval。session は開始したデバイスに属する。
- **device-scoped**: recording、TTS、audio route、surface state。
- **ephemeral coordination**: resident authority、evaluator heartbeat/lease、audio status、event delivery claim。

同じユーザーが二台から同時に turn を開始することは許容するが、planner mutation は storage の transaction と request ID で直列化し、同じ mutation を重複適用しない。
pending approval は現状 session-scoped であり、承認できるのは提案を出したデバイスに限られる。
どの端末でも承認できるようにするかは open question とする(後述)。

## Event-driven assistance

ユーザー発話だけでなく planner event も Agent の入口にする。
介入姿勢は積極側に倒す。
黙って予定が崩れるより、早めに声をかけて小さく直す方が planner の価値が出る。
ただし発話するかどうかは private channel 原則に、接触の形は「行動とズラす」の原則に従う。
着手済みタスクに関する発火条件は、観測に対応する所要時間分布から決める。

対象イベント:

- task の開始時刻
- 開始時刻を過ぎた未着手の継続(sync の check-in)
- 未分類 gap の継続(capture / sync の check-in)
- task の所要時間分布に対する超過またはペース逸脱
- deadline 違反の予測
- 未完了タスクの持ち越し(精算の入口)
- schedule 未生成または生成失敗
- 睡眠時間への影響

planner event の評価は常駐 timer の有無に依存させない。
イベントエンジンは一貫した planner snapshot、progress snapshot、coverage、各 revision、現在時刻、event ledger view から、発火すべき event と次に評価結果が変わり得る時刻を返す。
状態や schedule が変わるたびに次の評価時刻を再計算する。
resident authority は progress、schedule の commit 後、または `next_eval_at` に評価する。
Android はその一時刻を exact alarm として予約し、receiver が full local server を起動せずに bounded Rust evaluator を呼び出し、lease と snapshot revision を検証して event を ledger に確定し、次の alarm を予約する。
マイクを使わない状態評価は microphone foreground service と分離し、service が停止しても評価と通知を続ける。

イベントは直接 LLM turn を起動するとは限らない。
決定的に生成できる通知や action はアプリ側で生成し、曖昧な調整、説明、提案が必要な場合だけ Agent を呼ぶ。

```text
「レポートの開始時刻です」
[着手] [10分後] [組み直す]
```

追加の振る舞いを定める。

- **先送りの理由を一問だけ聞く**: ユーザーがタスクを別の時間帯へ延期したとき、「なにか詰まってる?」と一度だけ聞く。理由(ブロック中、気が重い、単に時間がない)は再配置の判断材料になる。答えなければ追わない。数十分の snooze では聞かない。snooze に毎回理由を求めることは「ズラす」の選択肢を重くし、接触の原則に反するからである。
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

「常時」はマイクを所有する process の寿命と、OS が許す再開契機で決まる。

1. **アプリ画面内のみ**: UI がマイクを持つ。現状の push-to-talk。
2. **明示開始後の継続**: Android ではユーザーが visible activity、通知、または widget から開始した microphone foreground service、Linux では tray デーモンが持つ。開始後は画面を離れてもロック中でも聞き続ける。
3. **端末起動中**: 一度有効化すると端末起動中は継続する。Android では再起動または OS による service 終了後に「Listen を再開」通知を出し、その一操作で再武装する。アプリ画面を開く必要はない。

Android 14 以降では `RECORD_AUDIO` は while-in-use permission であり、background や `BOOT_COMPLETED` receiver から microphone foreground service を新規作成できない。
そのため、boot receiver は録音を開始せず、event evaluation の alarm を復元して再開通知を表示する。
通知 action の `PendingIntent` が microphone foreground service を開始する。
Ok Google を残すため、takusu は既定 assistant を要求せず、`VoiceInteractionService` も採用しない。

service の自動再生成だけに録音の回復を依存しない。
OS が既存 service を維持または再生成できる場合は継続してよいが、force-stop、再起動、復帰不能な process kill の後に保証する回復経路は通知の一操作とする。
状態 evaluator は「ambient が有効だが microphone service が不在」という状態を検出し、再開通知を維持する。

ambient の単位はアプリではなく service とする。
状態検知は microphone service から分離し、service が停止中でも exact alarm から評価と通知を続ける。
Expo / React Native の component lifecycle に常時録音を所有させない。
platform shell(Android native service / Linux デーモン)が録音と lifecycle を所有し、Rust が VAD、KWS、ASR と event evaluation を担当する。
JS ほか UI 層は状態表示とユーザー操作の購読に徹する。

### desktop を実験場にする

ambient の制約はプラットフォーム間で非対称である。
Android には foreground service、background start 制限、電池、発熱、AEC の機種差がすべてあるのに対し、Linux desktop にはどれもない。
常駐は普通のプロセス、電源は据え置き、マイクは安定している。

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
- 常時 Listen から planner mutation を直接確定しない(音声承認の四層に従う)。

## Architecture

「共有 Rust core」は、一つの process が全端末の状態を所有することを意味しない。
各 platform shell が同じ状態モデルと遷移規則を使い、user-scoped な状態だけを storage で共有する。

```text
[user-scoped / storage]
planner state / coverage / event ledger / proposal / request idempotency
                 ↕
[resident coordination]
priority / evaluator heartbeat or lease / audio status / resident authority / delivery claim
                 ↕
[device-scoped Rust core]                    [device-scoped Rust core]
Android                                      Linux desktop
├── AgentSession                             ├── AgentSession
├── recording / TTS / audio route            ├── recording / TTS / audio route
├── event evaluator + exact alarm            ├── event evaluator + daemon timer
├── SpeechCapability                          ├── SpeechCapability
└── microphone foreground service            └── tray daemon / audio owner
        ↕                                             ↕
Android UI                                   tray / notification
```

AgentSession は device 内の session-scoped な turn、会話履歴、pending approval を所有する。
planner state、coverage、planner event、提案内容は user-scoped であり、device をまたいで同じ storage 表現を読む。
recording、TTS、audio route、surface state、SpeechCapability は device-scoped であり、別端末へ引き継がない。

Android の event evaluator と microphone foreground service は独立している。
evaluator はマイクが停止していても exact alarm から bounded Rust entry point を呼び、resident authority なら event を ledger に確定する。
ambient service が稼働中で private channel を使える場合だけ発話し、それ以外では通知へ降格する。
各デバイスは自分の local API host を持てるが、planner state と event ledger は共通 backend を読む。

ライブラリやモデルの選定、crate 構成、API の形は `plan/resident-agent.md` が定める。

## Rollout

体験の優先順位は三ループの依存関係に従う。
execution loop の閉ループを最初に閉じ、次に sync の接触を通知で成立させ、voice はその上に載せる。
sync と capture の接触は通知と quick actions だけでも成立するため、voice より先に価値を検証できる。

### Phase 1: execution loop の UI と sync の最小形

progress の storage、API、Agent tools は実装済みである。

- current task card と compact panel の quick actions
- structured presentation の最小型(current/next task、progress summary)
- Home、widget、Agent UI の状態同期
- server-issued one-shot capability による開始、完了、延期
- 開始時刻イベントの通知に「行動」と「ズラす」を両方載せる(sync の最小形)

### Phase 2: 共通層と薄い surface

surface を片方ずつ厚く作るのではなく、共有層を先に固め、両プラットフォームの surface は薄く同時に置く。

- user-scoped、session-scoped、device-scoped state の境界と surface protocol
- Android: resident button の全画面化(draggable)、compact panel
- Linux: tray icon、compact popover、actions 付き通知
- planner event evaluator、event ledger、Android exact alarm、通知と deep link
- coverage の信頼状態と、bootstrap / stale 時の presentation
- 多デバイス調停(優先度リスト、evaluator/audio heartbeat、resident authority、delivery claim)

### Phase 3: voice loop と capture

- 閉ループに必要な最低限: VAD endpointing、TTS 停止、modality-aware response
- 音声承認の四層
- 一言 capture(音声からの登録、LLM 補完、まとめて承認)
- intake インタビュー(中断再開可能)
- event-driven の発話(所要時間分布、private channel 原則、先送り理由の聴取)
- 精算の会話形(「今までゲームしてた」からの一括組み直し)
- 磨き込み: barge-in、レイテンシ予算、interruption / timeout / error recovery

### Phase 4: ambient listening

- desktop tray デーモンでの opt-in ambient(VAD → KWS → ASR ゲート)
- wake word の実機評価(誤発火率、日本語 KWS の成否)
- 未知時間帯の check-in(「今なにしてる?」)の proactive 発火
- Android: ユーザー操作から開始する microphone foreground service、永続通知と即時停止
- boot 後と service 終了後の「Listen を再開」通知、通知 action による一操作の再武装
- event evaluator と microphone service の独立 lifecycle
- ambient start / pause の話者照合と、誤操作を estimator からも消す undo
- privacy、電池、発熱、false positive の実機評価
- background / lock screen lifecycle

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
- takusu を Android の既定 assistant にせず、`VoiceInteractionService` を実装しない。Ok Google と併存する通常アプリとして動作する。
- Android の再起動直後にユーザー操作なしで録音を再開しない。OS の while-in-use permission 境界を回避しない。
- desktop の full Agent view / planner 全体 UI を作らない(web と CLI の再設計は別トラック)。
- 接触ゼロを目指さない。nag の回避は接触を減らす方向ではなく、接触に選択肢を持たせる方向で行う。
- 一括入力フォームによる census 型の初期登録を要求しない。
- 過ぎた時間の使途を評価、採点、可視化して行動変容を迫らない。記録は計画の入力であって成績表ではない。

## Open questions

- **check-in の頻度統治**: 未分類 gap の check-in を何分後に発火し、一日の上限を何回にするか。無応答による頻度低下の係数は実機利用で決める。
- **精算の粒度**: 何分以上のズレから精算対象にするか。数分の遅れまで記録を求めると申告コストが原則に反する。
- **乖離の気配の検出**: 「計画と違うことをしている気配」をセンサーなしでどの信号から得るか。v1 は未着手と未分類 gap だけを使う。
- **所要時間分布の更新**: モデルの形は `plan/resident-agent.md` の v1(分単位の切断正規、正規化項を含む厳密な progress posterior、単調増加して永続化される distribution revision)で固定した。残る open は band 境界、尤度ノイズ、task-kind fallback の定数を実機利用でどこに置くかである。
- **承認の可搬性**: pending approval を storage に持ち上げ、提案を出したデバイス以外でも承認できるようにするか。session 内で閉じる現設計との整合が必要。
- **wake word の実フレーズと日本語 KWS**: 事前学習モデルは中国語と英語が中心で、日本語フレーズがそのまま動くかは実機検証が要る。代替は wake word 自作学習、または desktop 限定で streaming ASR 常時稼働 + テキストマッチ。
- **ローカル TTS への移行基準**: 当面 Cartesia を使う。日本語品質と低レイテンシが CPU で成立するローカル TTS が現れたら proactive 発話から移行する。
- **話者照合の閾値と enrollment UX**: 何発話で声紋を登録するか、類似度閾値をどこに置くか、声の経年変化や環境差にどう追随するか。
- **先送り理由の保存先**: 聴取した理由を memory に保存するか、その場の再配置判断に使って捨てるか。
- **quiet hours の「緊急」定義**: 就寝後も通す例外イベントの範囲。
- **場所プロファイル**: 「自宅ではスピーカー発話を許可」のような opt-in 拡張の要否。

## Success criteria

- 新しいタスクが一言の申告から、見積もり込みで登録される。ユーザーが quantity や見積もりを入力する必要がない。
- intake を完了しなくても takusu を使い始められ、coverage が日々の sync で成長する。
- bootstrap では current task が候補として表示され、today-covered 以降で初めて権威ある「今やること」になる。
- stale を検出したときは current task より先に精算が提示される。
- 計画とズレた日でも、一日の終わりに today-covered 以上へ戻っている。
- ズレへの接触が常に「行動」と「ズラす」の両方を差し出し、どちらも一操作または一言で完了する。
- check-in への応答が一言で完結する。無視した場合に追撃がない。
- ユーザーが full Agent view を開かずに着手、進捗、完了、延期を行える。
- Agent の結果が Home、schedule、widget、全 surface に即時反映される。
- 一つの明示的な音声セッション内で複数 turn を継続できる。
- 所要時間分布の通常範囲で Agent が騒がず、注意または再計画範囲で対応する介入が届く。
- 自由時間、buffer、routine に未知活動の check-in が出ない。
- イヤホンなしの Android がスピーカーで proactive に発話しない。
- server と通信できるデバイス群では resident authority が一台だけであり、partition 後は再接続から規定時間内に一台へ収束する。microphone service の停止は speech capability を失わせるが、event evaluation authority を奪わない。
- 同じ event に由来する planner mutation が重複実行されない。
- Android で ambient listening の開始、稼働、停止、再武装待ちが常に視認できる。再起動後は通知一操作で録音を再開できる。
- 対象外音声が外部送信も永続化もされない。
- planner mutation はすべて四層の承認境界を維持する。
- 登録話者以外の声で音声確定層の変更が確定しない。

## 付録: canonical scenario(よく回った日)

体験の基準として、平日の一日を台本で示す。
presentation 型、event 発火、承認層の仕様は、この台本の各場面と矛盾しないように定める。
前提: desktop(tray 常駐、ambient 有効)と Android(優先度 2 位)、wake word は仮に「たくす」。

```text
■ 07:30 起床 habit の時刻。desktop はスリープ → Android が resident authority
  イヤホン未接続のため発話せず、Android 通知:
  「おはよう。今日は7件、まず 9:00 レポート。睡眠は予定通り。」
  [今日の予定] [組み直す]

■ 08:55 desktop に着席、スリープ解除。resident authority が無言で desktop に戻る

■ 09:00 タスク開始時刻
  desktop(発話): 「9時になった。レポート、始める?」
  ユーザー: 「始める」
  agent: 「開始。11時まで。」(start は即時確定層。確認なし)
  → Home / widget / tray が in_progress 表示に変わる

■ 09:40 独り言「あーこれ違うな」→ wake word なし、KWS を通らず何も起きない
  (raw audio は破棄され、ログにも残らない)

■ 10:20 ユーザー: 「たくす、演習半分終わった」
  agent: 「15題完了だね。予測は見積もりの通常範囲。このままでいこう。」
  (quantity の進捗から総所要時間の事後分布を更新するが、注意範囲には入らない)

■ 11:10 未完了という右打ち切り観測により注意範囲へ入る
  desktop(発話): 「思ったより時間かかってる? いまどのへん?」
  ユーザー: 「あと5題」
  agent: 「なら残り20分くらい。11時半までに伸ばして、昼を10分ずらす。いい?」
  ユーザー: 「いいよ」(音声確定層: 読み上げ済みの複合変更を肯定で確定)
  agent: 「更新した。」

■ 11:45 次のタスクの開始時刻を15分超過(未着手の sync event。σ は使わない)
  desktop(発話): 「『申請書を出す』が押してる。今から10分でやる? 午後に回す?」
  ユーザー: 「午後に回して」
  agent: 「なにか詰まってる? それとも時間だけ?」(先送り理由を一問だけ聞く)
  ユーザー: 「窓口の返事待ち」
  agent: 「了解、ブロック中として 15:00 に置き直す。返事が来たら教えて。」

■ 12:40 ユーザー: 「たくす、来週の火曜に歯医者いれて。3時」
  agent: 「来週火曜 15:00『歯医者』、移動込みで90分確保。
          『買い出し』が金曜に動く。いい?」
  ユーザー: 「うん」(音声確定層: 玉突きが読み上げ可能な範囲)
  agent: 「作成した。」(一言 capture: 見積もりと玉突きは agent が補完)

■ 14:00 agent の発話中にユーザーが「たくす、ちがう」
  → barge-in。TTS を停止して聞き直す(Phase 3 の磨き込みまではタップ停止で代替)

■ 15:00 外出。desktop の evaluator heartbeat が切れ、Android が resident authority に昇格
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

## 付録: 悪い日の scenario

sync と精算の仕様は、この台本の各場面と矛盾しないように定める。
前提は canonical scenario と同じ。

```text
■ 09:00 タスク開始時刻
  desktop(発話): 「9時。レポート始める?」
  ユーザー: 「あとで」
  agent: 「30分後にする?」
  ユーザー: 「うん」
  → 9:30 に置き直す(「ズラす」が一言で選べる。snooze なので理由は聞かない)

■ 09:30 再接触
  desktop(発話): 「レポート、始める?」
  ユーザー: (返事なし。ゲームを始めている)
  → 通知に降格。追わない

■ 11:00 「申請書を出す」の開始時刻。レポートは未着手のまま
  直前の問いかけが無視されているため発話せず、通知のみ:
  「『申請書を出す』の時間。レポートは未着手。」
  [着手] [組み直す] [2時間ほっといて]

■ 12:10 ユーザー: 「たくす、ごめん今までゲームしてた」
  agent: 「おかえり。9時からの3時間はゲームとして記録しておくよ。
          午後で組み直すと、レポートが13時、申請書が16時、演習は明日の朝。いい?」
  ユーザー: 「いいよ」
  agent: 「更新した。13時からレポート。」
  (精算: 白状が一言で計画に反映される。説教も採点もしない)

■ 13:00 レポート開始。15:30 に総所要時間の事後分布が再計画範囲へ入り、再計画提案、承認
  (ここからは canonical scenario と同じ execution loop)

■ 16:40 未分類 gap が 40 分継続(自由時間、buffer、routine ではない。ambient 有効時のみ)
  desktop(発話): 「今なにしてる?」
  ユーザー: 「部屋の片付け」
  agent: 「30分いれておく? 申請書は17時半に下げる。」
  ユーザー: 「うん」
  (未知の活動の capture。一往復で終える)

■ 21:50 就寝前の精算
  agent(発話): 「今日は完了1件。レポートは残り3割で明日の10時、
               演習は朝いちに置いた。これで寝て大丈夫。」
  (悪い日の成功条件: 全部やれたことではなく、
   計画が現実を反映した状態で一日が終わること)
```

## 付録: intake scenario

```text
■ 初回セットアップ直後
  agent: 「予定を聞かせて。まず、締め切りが決まってるものから。思いつく順でいい。」
  ユーザー: 「レポートが金曜。奨学金の書類が来週水曜。あと再来週テスト」
  agent: 「レポートは3時間、書類は1時間で見ておく。テストの科目は?」
  ユーザー: 「線形代数」
  agent: 「勉強時間は6時間で仮置きする。あとで演習を追加したら分割するよ。
          次、毎週決まってあるものは? 授業とかバイトとか。」
  ユーザー: 「授業は月水金の午前。バイトが火曜の夜」
  agent: 「カレンダーに入ってるやつは同期しておく。
          今日はここまでにする? 続きはまた聞く。」
  → 12件をまとめて承認に出す。見積もりは全件 agent が補完済み

■ 3日後、intake で取り切れなかった予定が sync で見つかる
  agent: 「今なにしてる?」
  ユーザー: 「サークルのミーティング」
  agent: 「それ入ってなかった。毎週?」
  ユーザー: 「隔週の木曜」
  agent: 「隔週木曜で登録しておく。」
  (coverage は intake で完成させず、sync で育てる)
```
