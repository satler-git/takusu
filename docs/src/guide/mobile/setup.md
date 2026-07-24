# セットアップ

## 前提条件

モバイルアプリの開発には以下が必要です。

- **Nix**：開発環境全体を提供するパッケージマネージャです。
- **Android SDK**：`nix develop` で提供される Android 開発キットです。
- Android 実機またはエミュレータ

## 開発ビルド

開発ビルドは、Expo の `run:android` を使って実機またはエミュレータにインストールします。
`TAKUSU_BUILD_VARIANT=dev` は development build を使用します。
エミュレータでは `TAKUSU_ANDROID_ABIS=x86_64` を指定してください。

```sh
cd mobile
TAKUSU_BUILD_VARIANT=dev nix develop --command bash -c \
  "npx expo run:android --device --variant debug"
```

## リリース APK ビルド

リリース APK は `nix run` でビルドします。

```sh
nix run .#build-android-apk
```

開発ビルド用とエミュレータ用は、それぞれ `.#build-android-apk-dev` と `.#build-android-apk-emulator` を使います。

## サーバー接続

モバイルアプリは `takusu-local` または **Cloudflare Worker** に接続します。
設定画面で endpoint と token を入力してください。
