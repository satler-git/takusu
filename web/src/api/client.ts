import { TakusuClient } from '@takusu/client';

let clientPromise: Promise<TakusuClient> | null = null;

// The web UI is localhost-only and trusts the local machine, so the user never
// enters a token: we fetch /bootstrap once and use the returned root token for
// all /api calls via the shared TakusuClient.
async function bootstrap(): Promise<TakusuClient> {
  const res = await fetch('/bootstrap');
  if (!res.ok) throw new Error(`bootstrap failed: ${res.status}`);
  const body = (await res.json()) as { token: string };
  // Empty baseUrl → relative URLs, which work behind the dev proxy and when
  // served same-origin by takusu-web.
  return new TakusuClient('', body.token);
}

export function getClient(): Promise<TakusuClient> {
  clientPromise ??= bootstrap();
  return clientPromise;
}

export { ApiError } from '@takusu/client';
