// Dynamic Expo config.
//
// Stable builds use the values in app.json unchanged. To build a development
// variant that can coexist with the stable app on the same device (different
// application ID + launcher label + deep-link scheme), run the build with:
//
//   TAKUSU_BUILD_VARIANT=dev npx expo prebuild --platform android --no-install
//
// or use the Nix helper:
//
//   nix run .#build-android-apk-dev
//
// This keeps the stable package (`dev.satler.takusu`) intact for release CI,
// which never sets TAKUSU_BUILD_VARIANT.
// Clone app.json so mutations do not leak into Node's require cache.
const baseConfig = JSON.parse(JSON.stringify(require('./app.json')));
const expo = baseConfig.expo;

const sentryUrl = process.env.SENTRY_URL || 'https://sentry.io/';
const sentryProject = process.env.SENTRY_PROJECT || 'takusu';
const sentryOrg = process.env.SENTRY_ORG || 'satler-git';

const isDev = process.env.TAKUSU_BUILD_VARIANT === 'dev';

// Embed git commit/tag at build time so the settings page can show the
// exact source the APK was built from (instead of an opaque build number).
// Falls back to "unknown" when the env vars are not set (e.g. local dev).
expo.extra = {
  ...(expo.extra || {}),
  gitCommit: process.env.TAKUSU_GIT_COMMIT || 'unknown',
  gitTag: process.env.TAKUSU_GIT_TAG || 'unknown',
};

if (isDev) {
  expo.name = 'takusu dev';
  expo.slug = 'takusu-dev';
  expo.scheme = 'takusu-dev';
  if (expo.android) {
    expo.android.package = 'dev.satler.takusu.dev';
  }
}

// Plugins must be listed in the plugins array to be executed by `expo prebuild`.
// Calling them directly from app.config.js does not apply their native mods.
expo.plugins = expo.plugins || [];
expo.plugins.push(
  [
    '@sentry/react-native/expo',
    {
      url: sentryUrl,
      project: sentryProject,
      organization: sentryOrg,
      // This value is baked into android/app/build.gradle at `expo prebuild`
      // time, so SENTRY_AUTH_TOKEN must be set during prebuild for release
      // source maps to upload. Set SENTRY_DISABLE_AUTO_UPLOAD=true to skip.
      disableAutoUpload:
        !process.env.SENTRY_AUTH_TOKEN ||
        process.env.SENTRY_DISABLE_AUTO_UPLOAD === 'true',
    },
  ],
  './plugins/withTakusuAppIcon',
);

module.exports = baseConfig;
