// sync-commands.jsx — command-palette entries that drive the v0.2 folder
// sync feature. The user picks any folder that their OS already syncs
// across devices (iCloud Drive, Dropbox, Syncthing) and todarchy mirrors
// its Automerge doc there on every save.

import { invoke } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';

export async function getSyncFolder() {
  try {
    const folder = await invoke('get_sync_folder');
    return typeof folder === 'string' ? folder : '';
  } catch (e) {
    console.warn('get_sync_folder failed:', e);
    return '';
  }
}

export async function pickSyncFolder(flash) {
  try {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      title: 'Pick a folder your OS keeps in sync (iCloud / Dropbox / Syncthing)',
    });
    if (!selected) return; // user cancelled
    const folder = Array.isArray(selected) ? selected[0] : selected;
    await invoke('set_sync_folder', { folder });
    flash?.('sync on: ' + short(folder));
  } catch (e) {
    console.error('set_sync_folder failed:', e);
    flash?.('sync setup failed — see devtools');
  }
}

export async function clearSyncFolder(flash) {
  try {
    await invoke('clear_sync_folder');
    flash?.('sync off — local only');
  } catch (e) {
    console.error('clear_sync_folder failed:', e);
  }
}

function short(p) {
  if (!p) return '';
  if (p.length <= 40) return p;
  return '…' + p.slice(-39);
}
