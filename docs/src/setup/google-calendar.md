# Google Calendar 連携の設定

## 1. Google Cloud Console で OAuth2 クライアントを作成

1. [Google Cloud Console](https://console.cloud.google.com/) にアクセスしてください。
2. プロジェクトを作成または選択してください。
3. 「API とサービス」→「ライブラリ」から Google Calendar API を有効化してください。
4. 「認証情報」→「OAuth クライアント ID」を作成してください。

### 共通：Web アプリケーション

CLI とモバイルアプリは、同じ **Web アプリケーション** クライアント ID を共有できます。

1. 「Web アプリケーション」を選択します。
2. 許可されたリダイレクト URI に `http://127.0.0.1:8765/callback` を追加します。
   これは CLI の `takusu sync login` が使うデフォルトのポートです。
   `--port` で変更する場合は、ここも同じポートに合わせてください。
3. クライアント ID とクライアントシークレットをメモしてください。
   この値を takusu の設定に使います。

### モバイルアプリ用：Android

モバイルアプリでは、同じプロジェクトに **Android** クライアント ID も作成し、アプリの署名を登録してください。
この Android クライアント ID は、Google Play サービスがアプリを検証するために使われます。
takusu の設定に入れる `client_id` は、上記の **Web アプリケーション** のもののままです。

1. 「Android」を作成します。
2. パッケージ名に `dev.satler.takusu`（リリース）または `dev.satler.takusu.dev`（`TAKUSU_BUILD_VARIANT=dev` の開発ビルド）を指定してください。
3. SHA-1 フィンガープリントを登録してください。
   - リリースビルドはリリースキーの SHA-1 を使います。
   - 開発ビルドは通常、`~/.android/debug.keystore` の SHA-1 を使います。

```sh
keytool -list -v -keystore ~/.android/debug.keystore -alias androiddebugkey -storepass android -keypass android | grep SHA1
```

## 2. CLI で認証

```sh
cargo run -p takusu-cli -- sync login \
  --client-id <CLIENT_ID> \
  --client-secret <CLIENT_SECRET>
```

**OAuth2**（Google アカウントへのアクセス権限を安全に委任する認可フレームワーク）のフローでブラウザが開きます。
Google 認証後、ローカルコールバックサーバーが認可コードを受け取ります。
CLI は **refresh_token**（アクセストークンを更新する長期トークン）を取得し、データベースに保存します。

## 3. モバイルアプリで認証

モバイルアプリでも、設定画面から **Google サインイン** ボタンで OAuth2 認証できます。

1. モバイルの設定画面で、Google Calendar の **Web アプリケーション** クライアント ID とクライアントシークレットを保存します。
2. 「Googleでログイン」を押すと、端末のネイティブ Google サインインが起動します。
   ライブラリはこのクライアント ID を **webClientId** として使います。
3. サインイン後に取得した server auth code が、バックエンドの `/api/sync/oauth/callback` へ送信されます。
4. バックエンドが認可コードを **refresh_token** と交換し、データベースに保存します。

このフローでは `redirect_uri` は不要です。

## 4. 既存の refresh_token を設定

ネイティブサインインが使えない場合は、CLI で取得した refresh_token を手動で設定できます。

```sh
cargo run -p takusu-cli -- sync setup --refresh-token <REFRESH_TOKEN>
```

モバイルの設定画面でも、同じく refresh_token を貼り付けて保存できます。

## 5. 同期を実行

```sh
cargo run -p takusu-cli -- sync trigger
```

`schedule generate`、`schedule move`、`schedule clear` を実行すると、自動的に同期されます。

## 注意

Google Calendar 側で削除したイベントは、takusu 側には反映されません。

`takusu sync delete-all` で Google Calendar 側の takusu イベントをすべて削除できます。
