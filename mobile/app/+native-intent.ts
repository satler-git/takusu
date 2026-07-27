// Redirect share intents to the iCal import handler route.
// expo-sharing uses the hostname "expo-sharing" for incoming share URLs.

export function redirectSystemPath({
  path,
}: {
  path: string;
  initial: boolean;
}): string {
  try {
    const url = new URL(path);
    if (url.hostname === 'expo-sharing') {
      return '/import-ical';
    }
    // The widget emits scheme URLs such as takusu://tasks and takusu://task/<id>.
    // Normalize them so the host becomes the first route segment.
    if (url.protocol === 'takusu:' || url.protocol === 'takusu-dev:') {
      if (url.hostname === 'tasks') {
        return '/tasks';
      }
      if (url.hostname === 'task') {
        const p = url.pathname === '/' ? '' : url.pathname;
        return `/task${p}`;
      }
      return '/';
    }
    return path;
  } catch {
    return path;
  }
}
