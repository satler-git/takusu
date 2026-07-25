import { useEffect, useState } from 'react';
import { getClient } from './api/client';

export default function App() {
  const [health, setHealth] = useState<string>('connecting…');

  useEffect(() => {
    getClient()
      .then((c) => c.health())
      .then((h) => setHealth(h))
      .catch((e) => setHealth(`error: ${String(e)}`));
  }, []);

  return (
    <div className="flex h-screen flex-col bg-neutral-950 text-neutral-100">
      <header className="flex items-center gap-3 border-b border-neutral-800 px-4 py-2">
        <span className="font-semibold text-brand">takusu</span>
        <span className="text-sm text-neutral-500">web</span>
      </header>

      <div className="flex min-h-0 flex-1">
        <aside className="w-56 shrink-0 border-r border-neutral-800 p-3 text-sm text-neutral-400">
          <p>sidebar (scaffold)</p>
        </aside>

        <main className="flex flex-1 items-center justify-center text-neutral-500">
          <div className="text-center">
            <p className="text-lg">timeline / graph / habit / stats / agent</p>
            <p className="mt-2 text-sm">
              server health: <span className="text-neutral-300">{health}</span>
            </p>
          </div>
        </main>
      </div>

      <footer className="border-t border-neutral-800 px-4 py-1 text-xs text-neutral-500">
        -- NORMAL --
      </footer>
    </div>
  );
}
