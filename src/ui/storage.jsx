// storage.jsx — persistence wrapper over Tauri's invoke().
//
// Shape of the on-disk document (`~/.local/share/todarchy/tasks.json`):
//   { version, tasks: [...], projects: [...], contexts: [...] }
//
// The app-level store() serializes the full document on every mutation and
// relies on the Rust side to do atomic-rename + .bak rotation (see
// src-tauri/src/store.rs). Save() is debounced so a rapid sequence of edits
// (e.g. toggling cursor + completing + undoing) coalesces into one disk write.

import { invoke } from '@tauri-apps/api/core';

export async function loadStore() {
  try {
    const data = await invoke('load_tasks');
    return {
      tasks: Array.isArray(data?.tasks) ? data.tasks : [],
      projects: Array.isArray(data?.projects) ? data.projects : [],
      contexts: Array.isArray(data?.contexts) ? data.contexts : [],
    };
  } catch (e) {
    console.warn('load_tasks failed, starting empty:', e);
    return { tasks: [], projects: [], contexts: [] };
  }
}

let pending = null;
let timer = null;

export function saveStore(doc, { immediate = false, delay = 250 } = {}) {
  pending = { version: 1, ...doc };
  if (timer) clearTimeout(timer);
  const flush = async () => {
    const toSave = pending;
    pending = null;
    timer = null;
    try {
      await invoke('save_tasks', { tasks: toSave });
    } catch (e) {
      console.error('save_tasks failed:', e);
    }
  };
  if (immediate) {
    void flush();
    return;
  }
  timer = setTimeout(flush, delay);
}
