// Format an ISO-8601 time window as a compact, user-readable string.
//
// Examples:
//   formatTimeWindow('2025-01-02T09:00:00+09:00', '2025-01-02T09:30:00+09:00')
//     → '09:00 – 09:30'
//   formatTimeWindow(undefined, '2025-01-02T09:30:00+09:00')
//     → '09:30'
//
// Invalid inputs are returned as-is so callers can debug.

export function formatTimeWindow(startAt?: string, endAt?: string): string {
  if (!startAt && !endAt) return '予定なし';
  const format = (s: string) => {
    const d = new Date(s);
    if (Number.isNaN(d.getTime())) return s;
    return d.toLocaleTimeString('ja-JP', {
      hour: '2-digit',
      minute: '2-digit',
      hour12: false,
    });
  };
  if (startAt && endAt) return `${format(startAt)} – ${format(endAt)}`;
  return startAt ? format(startAt) : endAt ? format(endAt) : '';
}
