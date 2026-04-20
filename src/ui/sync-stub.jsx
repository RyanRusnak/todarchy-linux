// sync-stub.jsx — local-only placeholder for the full E2EE sync module.
//
// v0.1 ships without sync. This shim satisfies the `useSync()` / `makeSyncCommand()`
// contract that app.jsx uses so the command palette still renders a "coming soon"
// entry without pulling in the (unimplemented) Rust sync commands. When the real
// sync relay ships, swap this file out for a module that talks to src-tauri/sync.rs.

import { useState } from 'react';

export function useSync() {
  const [flashMsg, setFlashMsg] = useState('');
  const flash = (msg) => {
    setFlashMsg(msg);
    setTimeout(() => setFlashMsg(''), 2200);
  };
  return {
    account: null,
    setAccount: () => {},
    dialog: null,
    setDialog: () => {},
    openSync: () => flash('sync ships in v0.2 — stay tuned'),
    flashMsg,
    flash,
  };
}

export function makeSyncCommand(_account, openSync) {
  return {
    id: 'sync',
    title: 'sync… (coming in v0.2)',
    hint: 'end-to-end encrypted sync is not yet shipped',
    keys: [],
    run: openSync,
  };
}
