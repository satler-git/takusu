const { getSentryExpoConfig } = require('@sentry/react-native/metro');
const path = require('path');

const config = getSentryExpoConfig(__dirname);

// The shared @takusu/client package is linked via a `file:` dependency
// (mobile/node_modules/@takusu/client -> ../../ts/takusu-client). Metro must
// follow the symlink and watch the real location outside the mobile root.
config.watchFolders = [
  ...(config.watchFolders ?? []),
  path.resolve(__dirname, '../ts'),
];
config.resolver.unstable_enableSymlinks = true;
config.resolver.unstable_enablePackageExports = true;

module.exports = config;
