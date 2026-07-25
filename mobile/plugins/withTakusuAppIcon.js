const {
  withAndroidManifest,
  withMainActivity,
} = require('@expo/config-plugins');
const {
  mergeContents,
} = require('@expo/config-plugins/build/utils/generateCode');

function toArray(value) {
  if (Array.isArray(value)) return value;
  if (value == null) return [];
  return [value];
}

function applySplashTheme(contents) {
  const newSrc = '    expo.modules.takusuappicon.TakusuTheme.apply(this)';
  const tag = 'takusu-splash-theme';
  const comment = '    //';

  try {
    return mergeContents({
      src: contents,
      newSrc,
      tag,
      comment,
      anchor: /SplashScreenManager\.registerOnActivity\(this\);?/,
      offset: 0,
    }).contents;
  } catch (err) {
    if (err?.code !== 'ERR_NO_MATCH') {
      throw err;
    }
    return mergeContents({
      src: contents,
      newSrc,
      tag,
      comment,
      anchor: /super\.onCreate\([^)]*\);?/,
      offset: 0,
    }).contents;
  }
}

function withTakusuAppIcon(config) {
  config = withAndroidManifest(config, (mod) => {
    const manifest = mod.modResults;
    const application = toArray(manifest?.manifest?.application)[0];
    if (!application) {
      throw new Error('No <application> found in AndroidManifest.xml');
    }

    for (const activity of toArray(application.activity)) {
      const activityName = activity?.$?.['android:name'];
      if (!activityName || !activityName.endsWith('.MainActivity')) {
        continue;
      }

      const filters = toArray(activity['intent-filter']);
      for (const filter of filters) {
        const actions = toArray(filter.action);
        const categories = toArray(filter.category);
        const hasMain = actions.some(
          (a) => a?.$?.['android:name'] === 'android.intent.action.MAIN',
        );
        const hasLauncher = categories.some(
          (c) => c?.$?.['android:name'] === 'android.intent.category.LAUNCHER',
        );

        if (hasMain && hasLauncher) {
          // The aliases provide the launcher entry; remove the whole
          // MAIN/LAUNCHER filter from the real activity.
          filter.takusuDelete = true;
        }
      }

      const remaining = filters.filter((f) => !f.takusuDelete);
      if (remaining.length > 0) {
        activity['intent-filter'] = remaining;
      } else {
        delete activity['intent-filter'];
      }
    }

    return mod;
  });

  config = withMainActivity(config, (mod) => {
    const { modResults } = mod;
    modResults.contents = applySplashTheme(modResults.contents);
    return mod;
  });

  return config;
}

module.exports = withTakusuAppIcon;
