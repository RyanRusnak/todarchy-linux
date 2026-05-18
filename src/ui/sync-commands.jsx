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

/** Fetch the full status shape: { folder, last_synced_at, last_sync_error }.
 *  Use this once on app mount; thereafter rely on the `sync-status` event
 *  which the backend emits whenever load / save / watcher runs. */
export async function getSyncStatus() {
  try {
    const s = await invoke('get_sync_status');
    return s && typeof s === 'object' ? s : { folder: '', last_synced_at: null, last_sync_error: null };
  } catch (e) {
    console.warn('get_sync_status failed:', e);
    return { folder: '', last_synced_at: null, last_sync_error: null };
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

// ---------- Shared projects (encrypted per-project files) ----------
//
// All three commands require a sync folder; the backend rejects them
// with a friendly error otherwise. UI flow:
//   - promote: project's tasks move into shared_<id>.automerge.enc;
//     returns a todarchy:// share link to copy and send to collaborators.
//   - accept: paste a link; backend stores the key locally; once the
//     encrypted file arrives via Dropbox/iCloud/etc, tasks appear.
//   - leave: forget the key on this device. Peers keep their copies.

export async function promoteProject(projectId) {
  return invoke('share_promote', { projectId });
}

export async function acceptShareLink(url) {
  return invoke('share_accept', { url });
}

export async function leaveSharedProject(projectId) {
  return invoke('share_leave', { projectId });
}

/// Copy `text` to the clipboard. Uses navigator.clipboard when available
/// (Tauri webview supports it on Linux); falls back to a hidden textarea
/// + document.execCommand for older webviews.
export async function copyToClipboard(text) {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch { /* fall through to legacy path */ }
  try {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.focus();
    ta.select();
    const ok = document.execCommand('copy');
    document.body.removeChild(ta);
    return ok;
  } catch { return false; }
}
