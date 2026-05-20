// Main app. Hyprland-style tiling window with sidebar + task list + detail pane.

import {
  useState as aUseState,
  useEffect as aUseEffect,
  useMemo as aUseMemo,
  useRef as aUseRef,
  useCallback as aUseCb,
} from 'react';
import { Icon } from './icons.jsx';
import { Palette } from './palette.jsx';
import { useOmarchyTheme } from '../theme/useOmarchyTheme';
import { loadStore, saveStore, deleteIdsInStore } from './storage.jsx';
import {
  pickSyncFolder, clearSyncFolder, getSyncFolder, getSyncStatus,
  promoteProject, acceptShareLink, leaveSharedProject, copyToClipboard,
  setServerSync, clearServerSync, serverHealthz,
} from './sync-commands.jsx';
import { listen as tauriListen } from '@tauri-apps/api/event';
import {
  LISTS,
  CONTEXTS,
  seedTasks,
  seedProjects,
  CTX_COLOR,
  DUE_COLOR,
  nid,
  fuzzyScore,
  parseQuickAdd,
  timeAgo,
  formatDeferUntil,
  getCommentAuthor,
  setCommentAuthor,
} from './data.jsx';

// Sync status indicator for the status bar. Three visual states:
//   - local   — no sync folder configured
//   - synced  — folder configured + last sync succeeded (shows "Xm ago")
//   - error   — folder configured + last sync failed (shows reason on hover)
function SyncDot({ status, onClick, labelled = false }) {
  const folder = status?.folder || '';
  const lastSync = status?.last_synced_at ?? null;
  const error = status?.last_sync_error || null;

  let label = 'local';
  let tone = 'idle';
  let tip = 'local only — click to pick a sync folder';
  if (folder) {
    if (error) {
      label = 'sync err';
      tone = 'error';
      tip = `sync error: ${error}\nfolder: ${folder}`;
    } else if (lastSync) {
      label = `synced · ${timeAgoShort(lastSync)}`;
      tone = 'ok';
      tip = `last sync: ${new Date(lastSync).toLocaleString()}\nfolder: ${folder}`;
    } else {
      label = 'pending';
      tone = 'pending';
      tip = `sync folder set but no successful sync yet\nfolder: ${folder}`;
    }
  }

  const color = tone === 'ok' ? 'var(--success)'
    : tone === 'error' ? 'var(--danger)'
    : tone === 'pending' ? 'var(--warn)'
    : 'var(--fg-faint)';
  const filled = tone === 'ok' || tone === 'error';

  return (
    <button
      onClick={onClick}
      aria-label={label}
      title={tip}
      style={{
        display: 'inline-flex', alignItems: 'center', gap: 6,
        background: 'transparent', border: 0, cursor: 'pointer',
        padding: 0, font: 'inherit',
        color: tone === 'error' ? 'var(--danger)' : 'var(--fg-mute)',
      }}
    >
      <span style={{
        width: 6, height: 6, borderRadius: 3,
        background: filled ? color : 'transparent',
        border: filled ? '0' : `1.2px solid ${color}`,
        boxShadow: filled ? `0 0 8px ${color}` : 'none',
        flexShrink: 0,
      }} />
      {labelled && (
        <span style={{ fontSize: 11, letterSpacing: 0.3 }}>{label}</span>
      )}
    </button>
  );
}

function timeAgoShort(ts) {
  const s = Math.max(0, Math.floor((Date.now() - ts) / 1000));
  if (s < 60) return 'just now';
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  return `${d}d ago`;
}

// Date helpers for defer quick-options.
function defer9am(daysAhead) {
  const d = new Date();
  d.setDate(d.getDate() + daysAhead);
  d.setHours(9, 0, 0, 0);
  return d.getTime();
}
function deferNextWeekday(targetDow) {
  // 0=sun..6=sat. Returns next occurrence at 09:00 (today counts only if future).
  const d = new Date();
  d.setHours(9, 0, 0, 0);
  const delta = (targetDow - d.getDay() + 7) % 7 || 7;
  d.setDate(d.getDate() + delta);
  return d.getTime();
}
function combineDateTime(dateStr, timeStr) {
  if (!dateStr) return null;
  const [y, m, day] = dateStr.split("-").map(Number);
  const [hh, mm] = (timeStr || "09:00").split(":").map(Number);
  return new Date(y, (m||1)-1, day||1, hh||0, mm||0, 0, 0).getTime();
}
function toInputDate(ts) {
  const d = new Date(ts);
  const p = n => String(n).padStart(2,"0");
  return d.getFullYear()+"-"+p(d.getMonth()+1)+"-"+p(d.getDate());
}
function toInputTime(ts) {
  const d = new Date(ts);
  const p = n => String(n).padStart(2,"0");
  return p(d.getHours())+":"+p(d.getMinutes());
}

// Collapse a long absolute path down to something that fits in a palette hint.
function shortenPath(p) {
  if (!p) return '';
  if (p.length <= 40) return p;
  return '…' + p.slice(-39);
}

function App() {
  // theme: driven by the active Omarchy theme via CSS custom properties on :root.
  // useOmarchyTheme() listens to the Rust watcher and paints tokens automatically;
  // we keep the return value around so the sidebar can show the active theme name.
  const omarchyTheme = useOmarchyTheme();

  // tasks state (persisted)
  const [tasks, setTasks] = aUseState(() => {
    try {
      const raw = localStorage.getItem("gtd.tasks.v2");
      if (raw) {
        const parsed = JSON.parse(raw);
        // dedupe / repair id collisions from older builds — rebuild with fresh ids
        // while preserving parent links
        const seen = new Set();
        const needsRepair = parsed.some(t => {
          if (seen.has(t.id)) return true;
          seen.add(t.id);
          return false;
        });
        if (!needsRepair) return parsed;
        const idMap = new Map();
        const reissued = parsed.map(t => {
          const fresh = nid();
          idMap.set(t.id, fresh);
          return { ...t, id: fresh };
        });
        // first-pass parent remap uses the final idMap (captures most recent mapping
        // for a given old id — matches the "last write wins" you'd see in storage)
        return reissued.map(t => ({
          ...t,
          parent: t.parent && idMap.has(t.parent) ? idMap.get(t.parent) : null,
        }));
      }
    } catch {}
    return seedTasks;
  });
  aUseEffect(() => {
    localStorage.setItem("gtd.tasks.v2", JSON.stringify(tasks));
  }, [tasks]);

  // UI state
  const [activeList, setActiveList] = aUseState(() => {
    const v = localStorage.getItem("gtd.list.v2") || "inbox";
    return v;
  });
  aUseEffect(() => { localStorage.setItem("gtd.list.v2", activeList); }, [activeList]);

  // projects (dynamic, persisted)
  const [projects, setProjects] = aUseState(() => {
    try {
      const raw = localStorage.getItem("gtd.projects.v1");
      if (raw) { const p = JSON.parse(raw); if (Array.isArray(p) && p.length) return p; }
    } catch {}
    return seedProjects.slice();
  });
  aUseEffect(() => { localStorage.setItem("gtd.projects.v1", JSON.stringify(projects)); }, [projects]);
  const [projEditor, setProjEditor] = aUseState(false);
  const [projEditorFocus, setProjEditorFocus] = aUseState("add"); // 'add' or project id

  // filter toggles (persisted). Show done & deferred items inside whatever list is active.
  const [showDone, setShowDone] = aUseState(() => localStorage.getItem("gtd.showDone") === "1");
  aUseEffect(() => { localStorage.setItem("gtd.showDone", showDone ? "1" : "0"); }, [showDone]);
  const [showDeferred, setShowDeferred] = aUseState(() => localStorage.getItem("gtd.showDeferred") === "1");
  aUseEffect(() => { localStorage.setItem("gtd.showDeferred", showDeferred ? "1" : "0"); }, [showDeferred]);

  const [cursor, setCursor] = aUseState(0);     // index within current list
  const [quickAdd, setQuickAdd] = aUseState(false);
  const [quickAddVal, setQuickAddVal] = aUseState("");
  const [search, setSearch] = aUseState("");
  const [searchMode, setSearchMode] = aUseState(false);
  const [paletteOpen, setPaletteOpen] = aUseState(false);
  const [editingId, setEditingId] = aUseState(null);
  const [editVal, setEditVal] = aUseState("");
  const [mode, setMode] = aUseState("NORMAL");  // NORMAL | INSERT | SEARCH | CMD | DEFER
  const [toast, setToast] = aUseState(null);
  const [nowTick, setNowTick] = aUseState(Date.now());
  const [undoStack, setUndoStack] = aUseState([]);
  const [deferFor, setDeferFor] = aUseState(null); // task id being deferred
  const [ctxFilter, setCtxFilter] = aUseState(""); // active context filter
  const [contexts, setContexts] = aUseState(() => {
    try {
      const raw = localStorage.getItem("gtd.contexts.v1");
      if (raw) { const p = JSON.parse(raw); if (Array.isArray(p) && p.length) return p; }
    } catch {}
    return CONTEXTS.slice();
  });
  aUseEffect(() => { localStorage.setItem("gtd.contexts.v1", JSON.stringify(contexts)); }, [contexts]);
  const [ctxEditor, setCtxEditor] = aUseState(false);

  // sync (E2EE — stubbed for v0.1; see src/ui/sync-stub.jsx)
  // Legacy sync-stub object retained only so the existing StatusBar +
  // flash-message bindings below keep compiling. Real sync now lives in
  // ./sync-commands.jsx and the Rust side.
  const sync = { account: null, openSync: () => {}, flashMsg: '' };
  const [showDetail, setShowDetail] = aUseState(() => {
    const v = localStorage.getItem("gtd.showDetail");
    return v === null ? true : v === "1";
  });
  aUseEffect(() => { localStorage.setItem("gtd.showDetail", showDetail ? "1" : "0"); }, [showDetail]);

  // collapsed parent ids
  const [collapsed, setCollapsed] = aUseState(() => {
    try {
      const raw = localStorage.getItem("gtd.collapsed.v1");
      if (raw) { const p = JSON.parse(raw); if (Array.isArray(p)) return new Set(p); }
    } catch {}
    return new Set();
  });
  aUseEffect(() => { localStorage.setItem("gtd.collapsed.v1", JSON.stringify([...collapsed])); }, [collapsed]);
  const toggleCollapsed = (id) => setCollapsed(s => {
    const n = new Set(s); if (n.has(id)) n.delete(id); else n.add(id); return n;
  });

  // drag state
  const [dragId, setDragId] = aUseState(null);
  const [dropTarget, setDropTarget] = aUseState(null); // {id, kind: 'nest'|'before'|'after'}

  // Persistence: localStorage gives us instant first-paint; the Rust-backed
  // store at ~/.local/share/todarchy/tasks.json is the source of truth. On
  // mount we adopt whatever's on disk (if non-empty), then mirror every
  // subsequent change back out. The bootDone ref prevents the initial
  // localStorage-seeded render from clobbering on-disk state before the
  // Tauri load completes.
  const bootDone = aUseRef(false);
  aUseEffect(() => {
    (async () => {
      const loaded = await loadStore();
      if (loaded.tasks.length || loaded.projects.length) {
        setTasks(loaded.tasks);
        setProjects(loaded.projects);
      }
      if (loaded.contexts.length) setContexts(loaded.contexts);
      bootDone.current = true;
    })();
  }, []);
  aUseEffect(() => {
    if (!bootDone.current) return;
    saveStore({ tasks, projects, contexts });
  }, [tasks, projects, contexts]);

  // Sync state — the backend emits `tasks-changed` on merge and `sync-status`
  // on every load/save/watcher tick. We pull the initial snapshot once, then
  // stay subscribed for the life of the window.
  const [syncFolder, setSyncFolder] = aUseState('');
  const [syncStatus, setSyncStatus] = aUseState({
    folder: '', last_synced_at: null, last_sync_error: null,
    server_base_url: '', server_main_doc_id: '',
  });
  aUseEffect(() => {
    getSyncStatus().then((s) => {
      setSyncStatus(s);
      setSyncFolder(s.folder || '');
    });
    const unsubs = [];
    tauriListen('tasks-changed', (event) => {
      const payload = event?.payload;
      if (!payload || typeof payload !== 'object') return;
      if (Array.isArray(payload.tasks)) setTasks(payload.tasks);
      if (Array.isArray(payload.projects)) setProjects(payload.projects);
      if (Array.isArray(payload.contexts) && payload.contexts.length) {
        setContexts(payload.contexts);
      }
      getSyncFolder().then(setSyncFolder);
    }).then((fn) => unsubs.push(fn));
    tauriListen('sync-status', (event) => {
      const payload = event?.payload;
      if (!payload || typeof payload !== 'object') return;
      setSyncStatus({
        folder: payload.folder || '',
        last_synced_at: payload.last_synced_at ?? null,
        last_sync_error: payload.last_sync_error ?? null,
        server_base_url: payload.server_base_url || '',
        server_main_doc_id: payload.server_main_doc_id || '',
      });
      setSyncFolder(payload.folder || '');
    }).then((fn) => unsubs.push(fn));
    return () => { unsubs.forEach((u) => u?.()); };
  }, []);
  // Tick every 30s so the "X ago" label stays accurate without state churn.
  const [, setStatusTick] = aUseState(0);
  aUseEffect(() => {
    const t = setInterval(() => setStatusTick((n) => n + 1), 30_000);
    return () => clearInterval(t);
  }, []);

  const normalizeCtx = (s) => {
    s = (s||"").trim().toLowerCase().replace(/\s+/g, "-");
    if (!s) return "";
    if (!s.startsWith("@")) s = "@" + s;
    return s;
  };

  const addContext = (raw) => {
    const name = normalizeCtx(raw);
    if (!name || name === "@") return false;
    if (contexts.includes(name)) { flashToast(name + " exists"); return false; }
    setContexts(cs => [...cs, name]);
    flashToast("added " + name);
    return true;
  };

  const renameContext = (oldName, newRaw) => {
    const newName = normalizeCtx(newRaw);
    if (!newName || newName === oldName) return;
    if (contexts.includes(newName)) { flashToast(newName + " exists"); return; }
    setContexts(cs => cs.map(c => c === oldName ? newName : c));
    setTasks(ts => ts.map(t => t.ctx === oldName ? { ...t, ctx: newName } : t));
    if (ctxFilter === oldName) setCtxFilter(newName);
    flashToast(oldName + " → " + newName);
  };

  const deleteContext = (name) => {
    setContexts(cs => cs.filter(c => c !== name));
    setTasks(ts => ts.map(t => t.ctx === name ? { ...t, ctx: "" } : t));
    if (ctxFilter === name) setCtxFilter("");
    flashToast("removed " + name);
  };

  aUseEffect(() => {
    const t = setInterval(() => setNowTick(Date.now()), 30000);
    return () => clearInterval(t);
  }, []);

  // auto-surface: any deferred task whose time has passed clears its deferUntil
  aUseEffect(() => {
    const due = tasks.filter(t => t.deferUntil && t.deferUntil <= nowTick);
    if (due.length === 0) return;
    setTasks(ts => ts.map(t =>
      (t.deferUntil && t.deferUntil <= nowTick)
        ? { ...t, deferUntil: undefined, wasDeferred: true }
        : t
    ));
    flashToast(due.length === 1 ? "1 task is back" : due.length + " tasks are back");
  }, [nowTick]);

  const flashToast = (msg) => {
    setToast({ id: Math.random(), msg });
    setTimeout(() => setToast(t => (t && t.msg === msg ? null : t)), 1600);
  };

  const pushUndo = (snapshot) => setUndoStack(s => [...s.slice(-19), snapshot]);

  // filtered view for active list — built as a tree ordered depth-first
  const viewTasks = aUseMemo(() => {
    const inList = tasks.filter(t => t.list === activeList);

    // per-task visibility check (status + ctx + search)
    const needle = search.trim().toLowerCase();
    const visible = (t) => {
      const done = !!t.doneAt;
      const deferred = t.deferUntil && t.deferUntil > Date.now();
      if (done && !showDone) return false;
      if (deferred && !showDeferred) return false;
      if (ctxFilter && t.ctx !== ctxFilter) return false;
      if (needle) {
        const hay = t.title + " " + (t.ctx || "") + " " + (t.note || "");
        if (!hay.toLowerCase().includes(needle)) return false;
      }
      return true;
    };

    // Build children index by parent id
    const byId = new Map(inList.map(t => [t.id, t]));
    const childrenOf = new Map();
    childrenOf.set(null, []);
    inList.forEach(t => {
      const p = t.parent && byId.has(t.parent) ? t.parent : null;
      if (!childrenOf.has(p)) childrenOf.set(p, []);
      childrenOf.get(p).push(t);
    });

    const statusRank = t => t.doneAt ? 2 : (t.deferUntil && t.deferUntil > Date.now()) ? 1 : 0;
    const dueRank = d => d === "today" ? 0 : d === "tomorrow" ? 1 : d === "this week" ? 2 : 3;
    // `pos` defaults to `created`, so rows keep their default "newest first"
    // order out of the box; Shift-J/K or Shift-↑/↓ edits `pos` to move rows
    // within a group without touching the timestamp.
    const orderKey = t => (typeof t.pos === 'number' ? t.pos : t.created);
    const cmp = (a, b) =>
      (statusRank(a) - statusRank(b)) ||
      (dueRank(a.due) - dueRank(b.due)) ||
      ((b.doneAt||0) - (a.doneAt||0)) ||
      (orderKey(b) - orderKey(a));

    // DFS emit, tagging depth + hasChildren + isCollapsed
    const out = [];
    const walk = (parentId, depth) => {
      const kids = (childrenOf.get(parentId) || []).slice().sort(cmp);
      for (const t of kids) {
        const allChildren = childrenOf.get(t.id) || [];
        const hasChildren = allChildren.length > 0;
        const selfVisible = visible(t);
        const isCollapsed = collapsed.has(t.id);
        if (selfVisible) {
          out.push({ ...t, _depth: depth, _hasChildren: hasChildren, _collapsed: isCollapsed });
          if (hasChildren && !isCollapsed) walk(t.id, depth + 1);
        } else if (hasChildren) {
          // parent filtered out — promote visible descendants so they still show
          walk(t.id, depth);
        }
      }
    };
    walk(null, 0);
    return out;
  }, [tasks, activeList, search, ctxFilter, showDone, showDeferred, nowTick, collapsed]);

  aUseEffect(() => {
    setCursor(c => Math.max(0, Math.min(c, Math.max(0, viewTasks.length - 1))));
  }, [viewTasks.length, activeList]);

  const counts = aUseMemo(() => {
    // active count per list (excludes done & currently-deferred)
    const c = {};
    tasks.forEach(t => {
      if (t.doneAt) return;
      if (t.deferUntil && t.deferUntil > Date.now()) return;
      c[t.list] = (c[t.list]||0) + 1;
    });
    return c;
  }, [tasks, nowTick]);

  const currentTask = viewTasks[cursor];

  // ---------- Mutations ----------
  const addTask = (raw, list = activeList) => {
    const parsed = parseQuickAdd(raw);
    // fall back to raw text if parsing stripped everything (e.g. user typed only "@read")
    if (!parsed.title) parsed.title = (raw || "").trim();
    if (!parsed.title) { flashToast("empty — nothing added"); return; }
    pushUndo(tasks);
    const t = {
      id: nid(),
      list,
      title: parsed.title,
      ctx: parsed.ctx,
      due: parsed.due,
      note: parsed.note,
      parent: null,
      created: Date.now(),
    };
    setTasks(ts => [t, ...ts]);
    // if we added to the list we're currently viewing, switch cursor to the new row
    // (scheduled so viewTasks has recomputed first)
    if (list === activeList) {
      setTimeout(() => {
        // after viewTasks memo updates, place cursor on new task id
        setCursor(() => {
          // rely on a fresh read of DOM rows via closest match — simple: move cursor to 0
          return 0;
        });
      }, 0);
    }
    flashToast("added: " + parsed.title.slice(0, 40));
  };

  const toggleDone = (id) => {
    pushUndo(tasks);
    setTasks(ts => ts.map(t => {
      if (t.id !== id) return t;
      if (t.doneAt) return { ...t, doneAt: undefined };
      return { ...t, doneAt: Date.now(), deferUntil: undefined };
    }));
    flashToast("completed");
  };

  const moveToList = (id, target) => {
    pushUndo(tasks);
    // move the task AND all descendants (keep them together)
    const descendantIds = new Set([id]);
    let changed = true;
    while (changed) {
      changed = false;
      tasks.forEach(t => {
        if (t.parent && descendantIds.has(t.parent) && !descendantIds.has(t.id)) {
          descendantIds.add(t.id); changed = true;
        }
      });
    }
    setTasks(ts => ts.map(t => {
      if (!descendantIds.has(t.id)) return t;
      // when moving the root, clear its parent so it becomes top-level in the new list
      if (t.id === id) return { ...t, list: target, parent: null };
      return { ...t, list: target };
    }));
    const targetName = target === "inbox" ? "inbox" : (projects.find(p => p.id === target)?.name || target);
    flashToast("→ " + targetName + (descendantIds.size > 1 ? " (+" + (descendantIds.size - 1) + ")" : ""));
  };

  // defer a task until a specific timestamp (status, not list).
  const deferTask = (id, untilTs, label) => {
    if (!untilTs || untilTs <= Date.now()) { flashToast("pick a future time"); return; }
    pushUndo(tasks);
    setTasks(ts => ts.map(t => t.id === id ? { ...t, deferUntil: untilTs, doneAt: undefined } : t));
    flashToast("deferred · " + (label || formatDeferUntil(untilTs)));
  };

  const clearDefer = (id) => {
    pushUndo(tasks);
    setTasks(ts => ts.map(t => t.id === id ? { ...t, deferUntil: undefined } : t));
    flashToast("un-deferred");
  };

  const deleteTask = (id) => {
    pushUndo(tasks);
    const target = tasks.find(t => t.id === id);
    const newParent = target ? (target.parent || null) : null;
    setTasks(ts => ts
      .filter(t => t.id !== id)
      .map(t => t.parent === id ? { ...t, parent: newParent } : t)
    );
    // Explicit delete through the backend — save_tasks is upsert-only so
    // sync peers never see a silent tombstone caused by an absent id.
    deleteIdsInStore('tasks', [id]);
    flashToast("deleted");
  };

  const updateTask = (id, patch) => {
    setTasks(ts => ts.map(t => t.id === id ? { ...t, ...patch } : t));
  };

  // Append a comment to a task. Comments are an object keyed by commentId
  // so the underlying Automerge Map<id, Comment> sees per-comment inserts —
  // two devices appending concurrently both survive merge. Append-only:
  // matches the iOS app's deliberate no-edit/no-delete v1 semantics, which
  // keeps "who deleted that?" ambiguity off the table between devices.
  const addComment = (taskId, rawText) => {
    const text = (rawText || "").trim();
    if (!text) return;
    const id = nid();
    const author = getCommentAuthor();
    const createdAt = Date.now();
    const comment = { id, author, text, createdAt };
    setTasks(ts => ts.map(t => {
      if (t.id !== taskId) return t;
      const existing = (t.comments && typeof t.comments === 'object' && !Array.isArray(t.comments))
        ? t.comments : {};
      return { ...t, comments: { ...existing, [id]: comment } };
    }));
  };

  // ---------- Nesting ----------
  // Can target be made a descendant of source? (i.e. is target already a descendant of source?)
  const isDescendant = (ancestorId, candidateId, taskList = tasks) => {
    if (!candidateId || !ancestorId) return false;
    let cur = taskList.find(t => t.id === candidateId);
    const seen = new Set();
    while (cur && cur.parent && !seen.has(cur.id)) {
      seen.add(cur.id);
      if (cur.parent === ancestorId) return true;
      cur = taskList.find(t => t.id === cur.parent);
    }
    return false;
  };

  // Set parent of `childId` to `parentId` (or null for root).
  // Guards against cycles and wrong-list parents.
  const setParent = (childId, parentId) => {
    if (childId === parentId) return;
    const child = tasks.find(t => t.id === childId);
    if (!child) return;
    if (parentId) {
      const parent = tasks.find(t => t.id === parentId);
      if (!parent) return;
      if (parent.list !== child.list) return; // must be same list
      if (isDescendant(childId, parentId)) return; // no cycles
    }
    pushUndo(tasks);
    setTasks(ts => ts.map(t => t.id === childId ? { ...t, parent: parentId || null } : t));
    // ensure new parent is not collapsed so change is visible
    if (parentId) setCollapsed(s => { const n = new Set(s); n.delete(parentId); return n; });
  };

  // Indent: make the task a child of the task immediately above it in the view (same depth, same parent).
  const indentAtCursor = () => {
    const t = viewTasks[cursor]; if (!t) return;
    // find the previous sibling at same depth with same parent
    for (let i = cursor - 1; i >= 0; i--) {
      const p = viewTasks[i];
      if (p._depth < t._depth) break; // ran out of siblings
      if (p._depth === t._depth && (p.parent || null) === (t.parent || null)) {
        setParent(t.id, p.id);
        return;
      }
    }
    flashToast("no sibling above");
  };

  // Outdent: move the task out from under its current parent, becoming a sibling of its parent.
  const outdentAtCursor = () => {
    const t = viewTasks[cursor]; if (!t) return;
    if (!t.parent) { flashToast("already top level"); return; }
    const parentTask = tasks.find(x => x.id === t.parent);
    setParent(t.id, parentTask ? (parentTask.parent || null) : null);
  };

  const undo = () => {
    setUndoStack(s => {
      if (!s.length) { flashToast("nothing to undo"); return s; }
      const prev = s[s.length - 1];
      setTasks(prev);
      flashToast("undo");
      return s.slice(0, -1);
    });
  };

  // ---------- Reorder (Shift-↑/↓ / Shift-J/K) ----------
  //
  // Swap the `pos` of the task under the cursor with its sibling one row
  // away. We only allow it within the same visual group (same status, same
  // due bucket, same parent in the tree) since the top-level sort partitions
  // by those fields — crossing a partition would be a no-op visually.
  const sameSortGroup = (a, b) => {
    if (!a || !b) return false;
    const doneA = !!a.doneAt, doneB = !!b.doneAt;
    if (doneA !== doneB) return false;
    const defA = a.deferUntil && a.deferUntil > Date.now();
    const defB = b.deferUntil && b.deferUntil > Date.now();
    if (!!defA !== !!defB) return false;
    if ((a.due || "") !== (b.due || "")) return false;
    if ((a.parent || null) !== (b.parent || null)) return false;
    return true;
  };
  const moveCursorTask = (direction) => {
    const idx = cursor;
    const current = viewTasks[idx];
    if (!current) return;
    const neighborIdx = direction === "down" ? idx + 1 : idx - 1;
    const neighbor = viewTasks[neighborIdx];
    if (!neighbor) { flashToast("edge of list"); return; }
    if (!sameSortGroup(current, neighbor)) {
      flashToast("can't cross sort group — change due/project first");
      return;
    }
    const posOf = t => (typeof t.pos === "number" ? t.pos : t.created);
    const curPos = posOf(current);
    const nbrPos = posOf(neighbor);
    pushUndo(tasks);
    setTasks(ts => ts.map(t => {
      if (t.id === current.id) return { ...t, pos: nbrPos };
      if (t.id === neighbor.id) return { ...t, pos: curPos };
      return t;
    }));
    setCursor(neighborIdx);
  };

  // ---------- Project mutations ----------
  const addProject = (name = "new project", opts = {}) => {
    const p = { id: "p_" + nid(), name, icon: opts.icon || "folder", accent: opts.accent || "var(--accent)" };
    setProjects(ps => [...ps, p]);
    setActiveList(p.id);
    flashToast("project added");
    return p;
  };
  const renameProject = (id, name) => {
    const n = (name || "").trim();
    if (!n) return false;
    setProjects(ps => ps.map(p => p.id === id ? { ...p, name: n } : p));
    return true;
  };
  const updateProject = (id, patch) => {
    setProjects(ps => ps.map(p => p.id === id ? { ...p, ...patch } : p));
  };
  const deleteProject = (id, { skipConfirm = false } = {}) => {
    const p = projects.find(x => x.id === id);
    if (!p) return;
    const count = tasks.filter(t => t.list === id).length;
    if (!skipConfirm) {
      const ok = confirm(`delete project “${p.name}”?` + (count ? ` ${count} task(s) will move to inbox.` : ""));
      if (!ok) return;
    }
    pushUndo(tasks);
    setTasks(ts => ts.map(t => t.list === id ? { ...t, list: "inbox" } : t));
    setProjects(ps => ps.filter(x => x.id !== id));
    deleteIdsInStore('projects', [id]);
    if (activeList === id) setActiveList("inbox");
    flashToast("project deleted");
  };

  // ---------- Commands ----------
  const commands = aUseMemo(() => {
    const projectCommands = projects.flatMap(p => [
      { id: "go-"+p.id, title: "go to → " + p.name, keys: ["g","p"], run: () => setActiveList(p.id) },
      { id: "move-"+p.id, title: "move task → " + p.name, run: () => currentTask && moveToList(currentTask.id, p.id) },
    ]);
    return [
      { id: "go-inbox", title: "go to inbox", hint: "1", keys: ["g","i"], run: () => setActiveList("inbox") },
      ...projectCommands,
      // Real sync entries live further down ("sync: choose a folder…" etc.)
      // — the sync-stub placeholder is gone now that folder sync shipped.
      { id: "project-manage", title: "manage projects…", hint: "add / rename / delete", keys: ["g","n"], run: () => { setProjEditorFocus("add"); setProjEditor(true); } },
      { id: "project-edit-current", title: "edit current project…", run: () => {
        if (activeList === "inbox") { flashToast("inbox can't be edited"); return; }
        setProjEditorFocus(activeList); setProjEditor(true);
      } },
      { id: "toggle-done",     title: (showDone ? "hide" : "show") + " completed",  hint: "filter", keys: ["f","d"], run: () => setShowDone(v => !v) },
      { id: "toggle-deferred", title: (showDeferred ? "hide" : "show") + " deferred", hint: "filter", keys: ["f","s"], run: () => setShowDeferred(v => !v) },
      { id: "add",         title: "new task",       hint: "quick-add", keys: ["↵", "o"], run: () => openQuickAdd() },
      { id: "complete",    title: "toggle complete", hint: "current", keys: ["x", "␣"], run: () => currentTask && toggleDone(currentTask.id) },
      { id: "indent",      title: "nest under sibling above", hint: "subtask", keys: ["Tab"], run: () => indentAtCursor() },
      { id: "outdent",     title: "outdent (promote)",        hint: "subtask", keys: ["⇧Tab"], run: () => outdentAtCursor() },
      { id: "reorder-up",   title: "move task up in list",   hint: "reorder", keys: ["⇧K"], run: () => moveCursorTask("up") },
      { id: "reorder-down", title: "move task down in list", hint: "reorder", keys: ["⇧J"], run: () => moveCursorTask("down") },
      { id: "collapse",    title: "collapse / expand children", hint: "tree", keys: ["z"], run: () => currentTask && currentTask._hasChildren && toggleCollapsed(currentTask.id) },
      { id: "move-inbox",  title: "move → inbox",   keys: ["m","i"], run: () => currentTask && moveToList(currentTask.id, "inbox") },
      { id: "defer",       title: "defer task…",    hint: "pick when", keys: ["s"], run: () => currentTask && openDefer(currentTask.id) },
      { id: "defer-tomorrow", title: "defer → tomorrow 9am", keys: ["s","t"], run: () => currentTask && deferTask(currentTask.id, defer9am(1), "tomorrow 09:00") },
      { id: "defer-week",     title: "defer → next week",    keys: ["s","w"], run: () => currentTask && deferTask(currentTask.id, defer9am(7), "next week") },
      { id: "defer-weekend",  title: "defer → saturday",     run: () => currentTask && deferTask(currentTask.id, deferNextWeekday(6), "saturday 09:00") },
      { id: "defer-clear",    title: "un-defer", run: () => currentTask && clearDefer(currentTask.id) },
      { id: "delete",      title: "delete task",    keys: ["d","d"], run: () => currentTask && deleteTask(currentTask.id) },
      { id: "edit",        title: "edit task",      keys: ["e"], run: () => currentTask && beginEdit(currentTask) },
      { id: "undo",        title: "undo last",      keys: ["u"], run: () => undo() },
      { id: "search",      title: "search tasks",   keys: ["/"], run: () => openSearch() },
      { id: "due-today",   title: "set due: today",    run: () => currentTask && updateTask(currentTask.id, { due: "today" }) },
      { id: "due-tomorrow",title: "set due: tomorrow", run: () => currentTask && updateTask(currentTask.id, { due: "tomorrow" }) },
      { id: "due-week",    title: "set due: this week",run: () => currentTask && updateTask(currentTask.id, { due: "this week" }) },
      { id: "due-clear",   title: "clear due date",    run: () => currentTask && updateTask(currentTask.id, { due: "" }) },
      ...contexts.map(c => ({ id: "ctx-"+c, title: "set context " + c, run: () => currentTask && updateTask(currentTask.id, { ctx: c }) })),
      { id: "ctx-clear", title: "clear context", run: () => currentTask && updateTask(currentTask.id, { ctx: "" }) },
      { id: "ctx-manage", title: "manage contexts…", hint: "add / rename / delete", run: () => setCtxEditor(true) },
      { id: "toggle-detail", title: showDetail ? "hide detail pane" : "show detail pane", hint: "inspect", keys: ["i"], run: () => setShowDetail(v => !v) },
      { id: "theme-menu", title: "theme: pick one in Omarchy", hint: "the Rust watcher picks it up live", run: () => {
        // The Omarchy theme is the source of truth — point users at the menu
        // or `omarchy-theme-set <name>`; the watcher handles the rest.
        flashToast("open the Omarchy menu or run `omarchy-theme-set \"Tokyo Night\"`");
      } },
      { id: "sync-pick", title: syncFolder ? "sync: change folder…" : "sync: choose a folder…",
        hint: syncFolder ? shortenPath(syncFolder) : "iCloud / Dropbox / Syncthing",
        run: () => pickSyncFolder(flashToast) },
      ...(syncFolder ? [{
        id: "sync-clear",
        title: "sync: turn off (go local-only)",
        hint: shortenPath(syncFolder),
        run: () => clearSyncFolder(flashToast),
      }] : []),
      // HTTP relay (server) sync — an alternative transport that talks
      // to a self-hosted todarchy-server. Set base URL + optional main
      // doc id; share the doc id with every other device of yours so
      // they pull/push the same blob.
      { id: "sync-server-set",
        title: syncStatus.server_base_url
          ? `sync: change server… (now: ${syncStatus.server_base_url})`
          : "sync: use a server…",
        hint: "https://your.server — pushes to /doc/:id",
        run: async () => {
          const defaultUrl = syncStatus.server_base_url || "https://";
          const url = window.prompt("server base URL", defaultUrl);
          if (!url) return;
          const existing = syncStatus.server_main_doc_id || "";
          const docId = window.prompt(
            "main doc id (leave blank to mint a fresh one — share with your other devices)",
            existing,
          );
          if (docId == null) return;
          try {
            const result = await setServerSync(url, docId || undefined);
            flashToast(`server sync on — id ${result.main_doc_id}`);
            // Surface the freshly-minted doc id so the user can paste
            // it into their other devices.
            await copyToClipboard(result.main_doc_id);
            window.alert(
              `Server sync configured.\n\nBase URL: ${result.base_url}\nMain doc id: ${result.main_doc_id}\n\nThe doc id has been copied to your clipboard. Paste it on every other device you want to sync.`,
            );
          } catch (e) {
            flashToast(`server sync failed: ${e}`);
          }
        } },
      ...(syncStatus.server_base_url ? [{
        id: "sync-server-copy-id",
        title: "sync: copy main doc id",
        hint: syncStatus.server_main_doc_id,
        run: async () => {
          const ok = await copyToClipboard(syncStatus.server_main_doc_id || "");
          flashToast(ok ? "copied main doc id" : `id: ${syncStatus.server_main_doc_id}`);
        },
      }, {
        id: "sync-server-health",
        title: "sync: check server health",
        hint: syncStatus.server_base_url,
        run: async () => {
          const ok = await serverHealthz();
          flashToast(ok ? "server: reachable" : "server: unreachable");
        },
      }, {
        id: "sync-server-clear",
        title: "sync: turn off server",
        hint: "stops pushing/pulling but keeps the local doc",
        run: async () => {
          try {
            await clearServerSync();
            flashToast("server sync off");
          } catch (e) {
            flashToast(`clear failed: ${e}`);
          }
        },
      }] : []),
      { id: "sync-status",
        title: syncStatus.server_base_url
          ? `sync: server — ${syncStatus.server_base_url}`
          : syncFolder
          ? `sync: folder — ${shortenPath(syncFolder)}`
          : "sync: off (local only)",
        hint: "info",
        run: () => {
          if (syncStatus.server_base_url) {
            flashToast(`server ${syncStatus.server_base_url} (id ${syncStatus.server_main_doc_id})`);
          } else if (syncFolder) {
            flashToast(`syncing to ${syncFolder}`);
          } else {
            flashToast("no sync configured");
          }
        } },
      { id: "clear-done",  title: "clear completed (hard delete)", run: () => {
        pushUndo(tasks);
        const doneIds = tasks.filter(t => t.doneAt).map(t => t.id);
        setTasks(ts => ts.filter(t => !t.doneAt));
        if (doneIds.length) deleteIdsInStore('tasks', doneIds);
        flashToast("cleared done");
      } },
      // Per-device display name stamped on new task comments. Shares
      // the `todarchy.comment.displayName` key with the iOS app so the
      // setting is consistent if a user has both — set it once in
      // either app and freshly-posted comments adopt it.
      { id: "set-comment-author", title: `set comment author… (now: ${getCommentAuthor()})`,
        hint: "shown on new comments you post",
        run: () => {
          const next = window.prompt("comment author", getCommentAuthor());
          if (next == null) return;
          setCommentAuthor(next);
          flashToast(`comments will post as ${getCommentAuthor()}`);
        } },
      // Sharing — per-project encrypted files. Available only when
      // sync is configured (the backend rejects with a friendly error
      // otherwise, but we surface that up front so the palette is
      // honest about what's available).
      ...projects.map(p => p.isShared
        ? {
            id: "share-leave-" + p.id,
            title: `unshare locally: ${p.name}`,
            hint: "removes the key + file from this device; peers keep theirs",
            run: async () => {
              try {
                await leaveSharedProject(p.id);
                flashToast(`left ${p.name}`);
              } catch (e) {
                flashToast(`leave failed: ${e}`);
              }
            },
          }
        : {
            id: "share-promote-" + p.id,
            title: `share project: ${p.name}`,
            hint: syncFolder ? "encrypts + writes shared_<id>.automerge.enc" : "set a sync folder first",
            run: async () => {
              if (!syncFolder) { flashToast("set a sync folder first"); return; }
              try {
                const link = await promoteProject(p.id);
                const ok = await copyToClipboard(link);
                flashToast(ok ? "share link copied" : `link: ${link}`);
                window.alert(`Share link for ${p.name}:\n\n${link}\n\nKeep this safe — anyone holding it can read + write this project.`);
              } catch (e) {
                flashToast(`share failed: ${e}`);
              }
            },
          }),
      { id: "share-accept",
        title: "accept a share link…",
        hint: "paste a todarchy:// URL",
        run: async () => {
          if (!syncFolder) { flashToast("set a sync folder first"); return; }
          const url = window.prompt("paste a todarchy:// share link");
          if (!url) return;
          try {
            const projectId = await acceptShareLink(url);
            flashToast(`joined ${projectId}`);
          } catch (e) {
            flashToast(`accept failed: ${e}`);
          }
        } },
    ];
  }, [currentTask, tasks, activeList, projects, showDone, showDeferred, showDetail, contexts, sync.account, syncFolder]);

  const openQuickAdd = () => { setQuickAdd(true); setQuickAddVal(""); setMode("INSERT"); };
  const closeQuickAdd = () => { setQuickAdd(false); setQuickAddVal(""); setMode("NORMAL"); };
  const openDefer = (id) => { setDeferFor(id); setMode("DEFER"); };
  const closeDefer = () => { setDeferFor(null); setMode("NORMAL"); };
  const openSearch = () => { setSearchMode(true); setMode("SEARCH"); };
  const closeSearch = () => { setSearchMode(false); setSearch(""); setMode("NORMAL"); };
  const beginEdit = (t) => { setEditingId(t.id); setEditVal(t.title); setMode("INSERT"); };
  const commitEdit = () => {
    if (editingId) updateTask(editingId, { title: editVal.trim() || "(untitled)" });
    setEditingId(null); setEditVal(""); setMode("NORMAL");
  };
  const cancelEdit = () => { setEditingId(null); setEditVal(""); setMode("NORMAL"); };

  // ---------- Keyboard ----------
  const pendingKey = aUseRef(null);
  const pendingTimer = aUseRef(null);

  const consumeSequence = (k) => {
    // returns true if a sequence was matched (handled)
    const combo = (pendingKey.current || "") + k;
    const proj = (idx) => projects[idx];
    const seqs = {
      "gg": () => setCursor(0),
      "gi": () => setActiveList("inbox"),
      "g1": () => proj(0) && setActiveList(proj(0).id),
      "g2": () => proj(1) && setActiveList(proj(1).id),
      "g3": () => proj(2) && setActiveList(proj(2).id),
      "g4": () => proj(3) && setActiveList(proj(3).id),
      "g5": () => proj(4) && setActiveList(proj(4).id),
      "gn": () => { setProjEditorFocus("add"); setProjEditor(true); },
      "dd": () => currentTask && deleteTask(currentTask.id),
      "mi": () => currentTask && moveToList(currentTask.id, "inbox"),
      "m1": () => currentTask && proj(0) && moveToList(currentTask.id, proj(0).id),
      "m2": () => currentTask && proj(1) && moveToList(currentTask.id, proj(1).id),
      "m3": () => currentTask && proj(2) && moveToList(currentTask.id, proj(2).id),
      "m4": () => currentTask && proj(3) && moveToList(currentTask.id, proj(3).id),
      "m5": () => currentTask && proj(4) && moveToList(currentTask.id, proj(4).id),
      "fd": () => setShowDone(v => !v),
      "fs": () => setShowDeferred(v => !v),
    };
    if (seqs[combo]) { seqs[combo](); pendingKey.current = null; clearTimeout(pendingTimer.current); return true; }
    // first of a prefix?
    if (["g","d","m","f"].includes(k) && !pendingKey.current) {
      pendingKey.current = k;
      clearTimeout(pendingTimer.current);
      pendingTimer.current = setTimeout(() => { pendingKey.current = null; }, 700);
      return true;
    }
    pendingKey.current = null;
    return false;
  };

  aUseEffect(() => {
    const handler = (e) => {
      // If the key event originated inside an editable field, the field's own
      // handler is responsible for it. Without this check, pressing Enter in
      // the quick-add input would commit the new task AND then bubble here,
      // where `case "Enter"` would toggle done on the just-added row — the
      // reason every CLI-added and UI-added task was landing as `doneAt` in
      // tasks.json. The React state guards below (`if (quickAdd) return`) read
      // stale closure values by the time this event reaches us, so we can't
      // rely on them alone.
      const tag = (e.target && e.target.tagName) ? e.target.tagName.toLowerCase() : "";
      if (tag === "input" || tag === "textarea" || (e.target && e.target.isContentEditable)) {
        return;
      }

      // global: palette
      if ((e.key === "k" && (e.metaKey || e.ctrlKey))) {
        e.preventDefault(); setPaletteOpen(true); setMode("CMD"); return;
      }
      if (e.key === "?" && mode === "NORMAL") {
        e.preventDefault(); setPaletteOpen(true); setMode("CMD"); return;
      }
      // swallow when palette is open (Palette handles its own keys)
      if (paletteOpen) return;

      // insert modes handle themselves
      if (quickAdd || editingId || searchMode || deferFor || ctxEditor || projEditor) return;

      // NORMAL mode
      if (mode !== "NORMAL") return;

      // no modifiers (other than shift) for vim keys
      if (e.metaKey || e.ctrlKey || e.altKey) return;

      if (consumeSequence(e.key)) { e.preventDefault(); return; }

      switch (e.key) {
        case "j":
          e.preventDefault();
          setCursor(c => Math.min(viewTasks.length - 1, c + 1));
          break;
        case "k":
          e.preventDefault();
          setCursor(c => Math.max(0, c - 1));
          break;
        case "ArrowDown":
          e.preventDefault();
          if (e.shiftKey) moveCursorTask("down");
          else setCursor(c => Math.min(viewTasks.length - 1, c + 1));
          break;
        case "ArrowUp":
          e.preventDefault();
          if (e.shiftKey) moveCursorTask("up");
          else setCursor(c => Math.max(0, c - 1));
          break;
        // Shift-J / Shift-K reorder the current task within its sort group.
        case "J":
          e.preventDefault();
          moveCursorTask("down");
          break;
        case "K":
          e.preventDefault();
          moveCursorTask("up");
          break;
        case "G": e.preventDefault(); setCursor(Math.max(0, viewTasks.length - 1)); break;
        case "h": case "ArrowLeft": e.preventDefault(); {
          const order = ["inbox", ...projects.map(p => p.id)];
          const i = order.indexOf(activeList);
          setActiveList(order[Math.max(0, i-1)]);
          break;
        }
        case "l": case "ArrowRight": e.preventDefault(); {
          const order = ["inbox", ...projects.map(p => p.id)];
          const i = order.indexOf(activeList);
          setActiveList(order[Math.min(order.length-1, i+1)]);
          break;
        }
        case "o":
        case "O":
        case "a":
        case "Enter":
          // Enter mirrors `o` — familiar to users coming from non-vim todo
          // apps. Toggle-done still has `x` and Space.
          e.preventDefault();
          openQuickAdd();
          break;
        case "x":
        case " ":
          e.preventDefault();
          if (currentTask) toggleDone(currentTask.id);
          break;
        case "e": e.preventDefault(); if (currentTask) beginEdit(currentTask); break;
        case "u": e.preventDefault(); undo(); break;
        case "s": e.preventDefault(); if (currentTask) openDefer(currentTask.id); break;
        case "/": e.preventDefault(); openSearch(); break;
        case ":": e.preventDefault(); setPaletteOpen(true); setMode("CMD"); break;
        case "1": e.preventDefault(); projects[0] && setActiveList(projects[0].id); break;
        case "2": e.preventDefault(); projects[1] && setActiveList(projects[1].id); break;
        case "3": e.preventDefault(); projects[2] && setActiveList(projects[2].id); break;
        case "4": e.preventDefault(); projects[3] && setActiveList(projects[3].id); break;
        case "5": e.preventDefault(); projects[4] && setActiveList(projects[4].id); break;
        case "0": e.preventDefault(); setActiveList("inbox"); break;
        case "Escape": e.preventDefault(); setSearch(""); setCtxFilter(""); break;
        case "i": e.preventDefault(); setShowDetail(v => !v); break;
        case "Tab":
          e.preventDefault();
          if (e.shiftKey) outdentAtCursor(); else indentAtCursor();
          break;
        case "z": e.preventDefault(); if (currentTask && currentTask._hasChildren) toggleCollapsed(currentTask.id); break;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [mode, paletteOpen, quickAdd, editingId, searchMode, viewTasks, currentTask, activeList, tasks, projects, ctxEditor, projEditor, deferFor]);

  // ---------- Render ----------
  return (
    <div style={S.root}>
      {/* thin hyprland-style border window */}
      <div style={S.window}>
        <div style={{ ...S.winInner, gridTemplateColumns: showDetail ? "240px 1fr 320px" : "240px 1fr" }}>
          {/* Sidebar */}
          <aside style={S.sidebar}>
            <div style={S.brand}>
              <div style={S.brandDot} />
              <div>
                <div style={{ color: "var(--fg)", fontWeight: 600, letterSpacing: .5 }}>todarchy</div>
                <div style={{ color: "var(--fg-mute)", fontSize: 11 }}>~/tasks</div>
              </div>
            </div>

            <nav style={S.nav}>
              {/* inbox */}
              <button onClick={() => setActiveList("inbox")}
                style={{ ...S.navItem, ...(activeList === "inbox" ? S.navItemActive : null) }}>
                <span style={{ color: "var(--orange)", width: 16, display: "inline-flex" }}>
                  <Icon name="inbox" size={14} />
                </span>
                <span style={{ flex: 1, textAlign: "left" }}>inbox</span>
                <span style={S.projCount}>{counts["inbox"] || 0}</span>
                <span className="kbd" style={{ marginLeft: 2 }}>0</span>
              </button>
            </nav>

            <div style={S.sidebarSection}>
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
                <div style={S.sidebarLabel}>projects</div>
                <button onClick={() => { setProjEditorFocus("add"); setProjEditor(true); }} style={S.ctxIconBtn}
                  title="manage projects (g n)" aria-label="manage projects">
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z"/></svg>
                </button>
              </div>
              {projects.map((p, i) => {
                const active = activeList === p.id;
                const n = counts[p.id] || 0;
                return (
                  <button
                    key={p.id}
                    onClick={() => setActiveList(p.id)}
                    style={{ ...S.navItem, ...(active ? S.navItemActive : null) }}>
                    <span style={{ color: p.accent, width: 16, display: "inline-flex" }}>
                      <Icon name={p.icon || "folder"} size={14} />
                    </span>
                    <span style={{ flex: 1, textAlign: "left", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
                      {p.name}
                    </span>
                    {p.isShared && (
                      <span
                        title="shared project — encrypted in your sync folder"
                        style={{
                          color: "var(--cyan)", fontSize: 9, letterSpacing: 0.6,
                          textTransform: "uppercase", padding: "1px 4px",
                          border: "1px solid color-mix(in oklab, var(--cyan) 40%, transparent)",
                          borderRadius: 3, marginRight: 4,
                        }}
                      >shared</span>
                    )}
                    <span style={S.projCount}>{n}</span>
                    {i < 5 && <span className="kbd" style={{ marginLeft: 2 }}>{i+1}</span>}
                  </button>
                );
              })}
              {projects.length === 0 && (
                <div style={{ fontSize: 11, color: "var(--fg-faint)", padding: "6px 8px" }}>
                  no projects. press <span className="kbd">g</span><span className="kbd">n</span> to manage.
                </div>
              )}
            </div>

            <div style={S.sidebarSection}>
                <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
                <div style={S.sidebarLabel}>contexts</div>
                <div style={{ display: "flex", gap: 2, alignItems: "center" }}>
                  {ctxFilter && (
                    <button onClick={() => setCtxFilter("")} style={S.clearCtxBtn}>clear</button>
                  )}
                  <button onClick={() => setCtxEditor(true)} style={S.ctxIconBtn}
                    title="manage contexts" aria-label="manage contexts">
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z"/></svg>
                  </button>
                </div>
              </div>
              {contexts.map(c => {
                const n = tasks.filter(t => t.ctx === c && !t.doneAt).length;
                const active = ctxFilter === c;
                return (
                  <button key={c}
                    onClick={() => setCtxFilter(f => f === c ? "" : c)}
                    style={{ ...S.ctxRow, ...(active ? S.ctxRowActive : null) }}>
                    <span style={{ color: CTX_COLOR[c] || "var(--fg-dim)" }}>●</span>
                    <span style={{ color: active ? "var(--fg)" : "var(--fg-dim)", flex: 1, textAlign: "left" }}>{c}</span>
                    <span style={{ color: "var(--fg-mute)", fontSize: 11 }}>{n}</span>
                  </button>
                );
              })}
            </div>

            <div style={{ ...S.sidebarSection, marginTop: "auto" }}>
              <div style={S.hintRow}><span className="kbd">j</span><span className="kbd">k</span><span style={S.hintLabel}>↑ ↓ move</span></div>
              <div style={S.hintRow}><span className="kbd">x</span><span style={S.hintLabel}>complete</span></div>
              <div style={S.hintRow}><span className="kbd">o</span><span style={S.hintLabel}>new</span></div>
              <div style={S.hintRow}><span className="kbd">e</span><span style={S.hintLabel}>edit</span></div>
              <div style={S.hintRow}><span className="kbd">d</span><span className="kbd">d</span><span style={S.hintLabel}>delete</span></div>
              <div style={S.hintRow}><span className="kbd">s</span><span style={S.hintLabel}>defer</span></div>
              <div style={S.hintRow}><span className="kbd">f</span><span className="kbd">d</span><span style={S.hintLabel}>toggle done</span></div>
              <div style={S.hintRow}><span className="kbd">f</span><span className="kbd">s</span><span style={S.hintLabel}>toggle deferred</span></div>
              <div style={S.hintRow}><span className="kbd">g</span><span className="kbd">n</span><span style={S.hintLabel}>new project</span></div>
              <div style={S.hintRow}><span className="kbd">/</span><span style={S.hintLabel}>search</span></div>
              <div style={S.hintRow}><span className="kbd">:</span><span style={S.hintLabel}>commands</span></div>
              <div style={S.hintRow}><span className="kbd">i</span><span style={S.hintLabel}>inspect</span></div>
              <div style={S.hintRow}><span className="kbd">?</span><span style={S.hintLabel}>all keys</span></div>
            </div>
          </aside>

          {/* Main + Detail pane */}
          <main style={S.main}>
            <Header
              activeList={activeList}
              total={viewTasks.length}
              cursor={cursor}
              mode={mode}
              search={search}
              searchMode={searchMode}
              onSearch={setSearch}
              onCloseSearch={closeSearch}
              nowTick={nowTick}
              ctxFilter={ctxFilter}
              onClearCtx={() => setCtxFilter("")}
              projects={projects}
              showDone={showDone}
              showDeferred={showDeferred}
              onToggleDone={() => setShowDone(v => !v)}
              onToggleDeferred={() => setShowDeferred(v => !v)}
            />

            <TaskList
              list={activeList}
              listLabel={activeList === "inbox" ? "inbox" : (projects.find(p => p.id === activeList)?.name || activeList)}
              tasks={viewTasks}
              cursor={cursor}
              setCursor={setCursor}
              onToggle={toggleDone}
              editingId={editingId}
              editVal={editVal}
              setEditVal={setEditVal}
              onCommitEdit={commitEdit}
              onCancelEdit={cancelEdit}
              onAddInline={openQuickAdd}
              onToggleCollapsed={toggleCollapsed}
              dragId={dragId}
              setDragId={setDragId}
              dropTarget={dropTarget}
              setDropTarget={setDropTarget}
              onDropNest={(childId, parentId) => setParent(childId, parentId)}
              onOutdent={(id) => {
                const t = tasks.find(x => x.id === id); if (!t) return;
                const p = tasks.find(x => x.id === t.parent);
                setParent(id, p ? (p.parent || null) : null);
              }}
            />

            {/* Status bar */}
            <StatusBar mode={mode} activeList={activeList} counts={counts} nowTick={nowTick} cursor={cursor} total={viewTasks.length} projects={projects} syncStatus={syncStatus} onSyncClick={() => pickSyncFolder(flashToast)} />
          </main>

          {/* Detail pane */}
          {showDetail && (
          <aside style={S.detail}>
            {currentTask ? (
              <Detail task={currentTask} onUpdate={(patch) => updateTask(currentTask.id, patch)} onMove={(l) => moveToList(currentTask.id, l)} onDelete={() => deleteTask(currentTask.id)} onDefer={() => openDefer(currentTask.id)} onClearDefer={() => clearDefer(currentTask.id)} onAddComment={(text) => addComment(currentTask.id, text)} contexts={contexts} projects={projects} />
            ) : (
              <EmptyDetail />
            )}
          </aside>
          )}
        </div>

        {/* Quick-add bar (floating, bottom) */}
        {quickAdd && (
          <QuickAdd
            value={quickAddVal}
            setValue={setQuickAddVal}
            onCommit={(val) => { addTask(val); closeQuickAdd(); }}
            onCancel={closeQuickAdd}
          />
        )}
      </div>

      <Palette
        open={paletteOpen}
        onClose={() => { setPaletteOpen(false); setMode("NORMAL"); }}
        commands={commands}
        tasks={tasks}
        projects={projects}
        onRunCommand={(c) => { setPaletteOpen(false); setMode("NORMAL"); c.run(); }}
        onJumpTask={(t) => {
          setPaletteOpen(false); setMode("NORMAL");
          setActiveList(t.list);
          // after view recomputes, cursor lands on this task
          setTimeout(() => {
            const v = tasks.filter(x => x.list === t.list);
            const idx = v.findIndex(x => x.id === t.id);
            if (idx >= 0) setCursor(idx);
          }, 0);
        }}
      />

      {/* sync dialog intentionally omitted in v0.1 — see src/ui/sync-stub.jsx */}

      {sync.flashMsg && (
        <div style={{
          position: "fixed", bottom: 24, left: "50%", transform: "translateX(-50%)",
          padding: "10px 16px", borderRadius: 999,
          background: "var(--bg-panel)", color: "var(--fg)",
          border: "1px solid var(--border)", fontSize: 13,
          boxShadow: "0 8px 24px -8px rgba(0,0,0,0.4)", zIndex: 250,
          animation: "fadeIn .18s ease",
        }}>
          {sync.flashMsg}
        </div>
      )}

      {ctxEditor && (
        <CtxEditor
          contexts={contexts}
          tasks={tasks}
          onAdd={addContext}
          onRename={renameContext}
          onDelete={deleteContext}
          onClose={() => setCtxEditor(false)}
        />
      )}

      {projEditor && (
        <ProjEditor
          projects={projects}
          tasks={tasks}
          counts={counts}
          focus={projEditorFocus}
          onAdd={(name, opts) => { addProject(name, opts); return true; }}
          onRename={renameProject}
          onUpdate={updateProject}
          onDelete={(id) => deleteProject(id, { skipConfirm: true })}
          onClose={() => setProjEditor(false)}
        />
      )}

      {deferFor && (
        <DeferPicker
          task={tasks.find(t => t.id === deferFor)}
          onCancel={closeDefer}
          onConfirm={(ts, label) => { deferTask(deferFor, ts, label); closeDefer(); }}
        />
      )}

      {toast && <div style={S.toast} key={toast.id}>{toast.msg}</div>}
    </div>
  );
}

// ---------- Sub-components ----------

function Header({ activeList, total, cursor, mode, search, searchMode, onSearch, onCloseSearch, nowTick, ctxFilter, onClearCtx, projects, showDone, showDeferred, onToggleDone, onToggleDeferred }) {
  const project = projects.find(p => p.id === activeList);
  const accent = activeList === "inbox" ? "var(--orange)" : (project?.accent || "var(--accent)");
  const label = activeList === "inbox" ? "inbox" : (project?.name || activeList);
  const desc = activeList === "inbox" ? "capture. sort later." : "project";
  const time = new Date(nowTick);
  const hh = String(time.getHours()).padStart(2,"0");
  const mm = String(time.getMinutes()).padStart(2,"0");
  const dayName = time.toLocaleDateString(undefined, { weekday: "long" });
  const dateStr = time.toLocaleDateString(undefined, { month: "short", day: "numeric" });

  return (
    <header style={S.header}>
      <div style={{ display: "flex", alignItems: "baseline", gap: 12, minWidth: 0 }}>
        <h1 style={S.h1}>
          <span style={{ color: accent }}>▍</span>
          <span style={{ color: "var(--fg-mute)" }}>~/tasks/</span>
          <span>{label}</span>
          {ctxFilter && (
            <>
              <span style={{ color: "var(--fg-faint)", margin: "0 4px" }}>∙</span>
              <button onClick={onClearCtx} style={{
                background: "color-mix(in oklab, " + (CTX_COLOR[ctxFilter]||"var(--accent)") + " 18%, transparent)",
                color: CTX_COLOR[ctxFilter] || "var(--accent)",
                border: "1px solid color-mix(in oklab, " + (CTX_COLOR[ctxFilter]||"var(--accent)") + " 40%, transparent)",
                padding: "1px 8px", borderRadius: 4, cursor: "pointer",
                font: "inherit", fontSize: 12,
              }}>{ctxFilter} <span style={{ opacity: .6 }}>✕</span></button>
            </>
          )}
        </h1>
        <span style={S.subtle}>{desc}</span>
      </div>
      <div style={S.headerRight}>
        <button
          onClick={onToggleDeferred}
          title={(showDeferred ? "hide" : "show") + " deferred (f s)"}
          style={{ ...S.filterToggle, ...(showDeferred ? S.filterToggleActive : null) }}>
          <Icon name={showDeferred ? "eye" : "eyeOff"} size={11} />
          <span>deferred</span>
          <span className="kbd" style={{ opacity: .7 }}>f s</span>
        </button>
        <button
          onClick={onToggleDone}
          title={(showDone ? "hide" : "show") + " completed (f d)"}
          style={{ ...S.filterToggle, ...(showDone ? S.filterToggleActive : null) }}>
          <Icon name={showDone ? "eye" : "eyeOff"} size={11} />
          <span>completed</span>
          <span className="kbd" style={{ opacity: .7 }}>f d</span>
        </button>
        <span style={S.dot}>·</span>
        {searchMode ? (
          <div style={S.searchBox}>
            <span style={{ color: "var(--accent)" }}>/</span>
            <input
              autoFocus
              value={search}
              onChange={e => onSearch(e.target.value)}
              onKeyDown={e => {
                if (e.key === "Escape") onCloseSearch();
                if (e.key === "Enter")  onCloseSearch();
              }}
              placeholder="search in view…"
              style={S.searchInput}
            />
            <span className="kbd">esc</span>
          </div>
        ) : (
          <>
            <span style={S.count}>{cursor + 1} / {total || 0}</span>
            <span style={S.dot}>·</span>
            <span style={S.subtle}>{dayName}, {dateStr}</span>
            <span style={S.dot}>·</span>
            <span style={{ color: "var(--fg)", fontVariantNumeric: "tabular-nums" }}>{hh}:{mm}</span>
          </>
        )}
      </div>
    </header>
  );
}

function TaskList({ list, listLabel, tasks, cursor, setCursor, onToggle, editingId, editVal, setEditVal, onCommitEdit, onCancelEdit, onAddInline, onToggleCollapsed, dragId, setDragId, dropTarget, setDropTarget, onDropNest, onOutdent }) {
  const scrollRef = aUseRef(null);
  const activeRef = aUseRef(null);
  aUseEffect(() => {
    if (!activeRef.current || !scrollRef.current) return;
    const el = activeRef.current;
    const parent = scrollRef.current;
    const er = el.getBoundingClientRect();
    const pr = parent.getBoundingClientRect();
    if (er.top < pr.top + 8) parent.scrollBy({ top: er.top - pr.top - 8, behavior: "smooth" });
    else if (er.bottom > pr.bottom - 8) parent.scrollBy({ top: er.bottom - pr.bottom + 8, behavior: "smooth" });
  }, [cursor, tasks.length]);

  if (tasks.length === 0) {
    return (
      <div style={S.listWrap}>
        <div ref={scrollRef} style={{ ...S.listScroll, display: "grid", placeItems: "center" }}>
          <div style={{ textAlign: "center", color: "var(--fg-mute)" }}>
            <div style={{ fontSize: 40, color: "var(--fg-faint)", marginBottom: 12 }}>∅</div>
            <div style={{ marginBottom: 4 }}>no tasks in <span style={{ color: "var(--fg-dim)" }}>{listLabel || list}</span>.</div>
            <div style={{ fontSize: 12 }}>press <span className="kbd">o</span> to capture.</div>
          </div>
        </div>
      </div>
    );
  }

  const anyParents = tasks.some(t => t._hasChildren);

  return (
    <div style={S.listWrap}>
      <div ref={scrollRef} style={S.listScroll}>
        {tasks.map((t, i) => {
          const active = i === cursor;
          const done = !!t.doneAt;
          const deferred = t.deferUntil && t.deferUntil > Date.now();
          const isEditing = editingId === t.id;
          const depth = t._depth || 0;
          const hasChildren = !!t._hasChildren;
          const isCollapsed = !!t._collapsed;
          const isDragging = dragId === t.id;
          const isDropTarget = dropTarget && dropTarget.id === t.id;

          return (
            <div
              key={t.id}
              ref={active ? activeRef : null}
              draggable={!isEditing}
              onDragStart={(e) => {
                setDragId(t.id);
                e.dataTransfer.effectAllowed = "move";
                try { e.dataTransfer.setData("text/plain", t.id); } catch {}
              }}
              onDragEnd={() => { setDragId(null); setDropTarget(null); }}
              onDragOver={(e) => {
                if (!dragId || dragId === t.id) return;
                e.preventDefault();
                e.dataTransfer.dropEffect = "move";
                setDropTarget({ id: t.id, kind: "nest" });
              }}
              onDragLeave={(e) => {
                // only clear if we're truly leaving (not entering a child)
                if (e.currentTarget.contains(e.relatedTarget)) return;
                setDropTarget(dt => (dt && dt.id === t.id ? null : dt));
              }}
              onDrop={(e) => {
                e.preventDefault();
                const src = dragId;
                setDragId(null); setDropTarget(null);
                if (src && src !== t.id) onDropNest(src, t.id);
              }}
              onClick={() => setCursor(i)}
              onDoubleClick={() => onToggle(t.id)}
              style={{
                ...S.row,
                ...(active ? S.rowActive : null),
                opacity: isDragging ? .35 : (done ? .55 : 1),
                outline: isDropTarget ? "1px solid var(--accent)" : "none",
                outlineOffset: isDropTarget ? -1 : 0,
                background: isDropTarget
                  ? "color-mix(in oklab, var(--accent) 10%, transparent)"
                  : (active ? S.rowActive.background : S.row.background),
              }}>
              <div style={S.rowGutter}>
                {active && <span style={{ color: "var(--accent)" }}>▍</span>}
              </div>

              {/* indent + tree connector */}
              {depth > 0 && (
                <div style={{ display: "flex", alignItems: "center", flexShrink: 0 }}>
                  {Array.from({ length: depth - 1 }).map((_, d) => (
                    <span key={d} style={{ display: "inline-block", width: 16, borderLeft: "1px dashed var(--border)", alignSelf: "stretch", marginLeft: 7 }} />
                  ))}
                  <span style={{ display: "inline-block", width: 16, color: "var(--fg-faint)", fontSize: 11, marginLeft: 7 }}>└</span>
                </div>
              )}

              {/* collapse caret (column shown only when any row has children) */}
              {anyParents && (
                <button
                  onClick={(e) => { e.stopPropagation(); if (hasChildren) onToggleCollapsed(t.id); }}
                  tabIndex={-1}
                  style={{
                    width: 12, height: 18, padding: 0, border: "none", background: "transparent",
                    color: "var(--fg-faint)", cursor: hasChildren ? "pointer" : "default",
                    display: "inline-flex", alignItems: "center", justifyContent: "center",
                    fontSize: 10, flexShrink: 0,
                    visibility: hasChildren ? "visible" : "hidden",
                  }}
                  aria-label={isCollapsed ? "expand" : "collapse"}
                  title={isCollapsed ? "expand (z)" : "collapse (z)"}>
                  {isCollapsed ? "▸" : "▾"}
                </button>
              )}

              <button
                onClick={(e) => { e.stopPropagation(); onToggle(t.id); }}
                style={S.check}
                aria-label="toggle complete">
                <span style={{ color: done ? "var(--success)" : "var(--fg-mute)", display: "inline-flex" }}>
                  <Icon name={done ? "boxOk" : "box"} size={16} />
                </span>
              </button>

              <div style={S.rowBody}>
                {isEditing ? (
                  <input
                    autoFocus
                    value={editVal}
                    onChange={e => setEditVal(e.target.value)}
                    onKeyDown={e => {
                      if (e.key === "Enter")  { e.preventDefault(); onCommitEdit(); }
                      if (e.key === "Escape") { e.preventDefault(); onCancelEdit(); }
                    }}
                    onBlur={onCommitEdit}
                    style={S.editInput}
                  />
                ) : (
                  <div style={{ ...S.title, textDecoration: done ? "line-through" : "none" }}>
                    {t.title}
                  </div>
                )}
                {t.note && !isEditing && (
                  <div style={S.note}>└ {t.note}</div>
                )}
              </div>

              <div style={S.rowMeta}>
                {hasChildren && isCollapsed && (
                  <span style={{
                    color: "var(--fg-mute)",
                    border: "1px solid var(--border)",
                    borderRadius: 4,
                    padding: "1px 6px",
                    fontSize: 11,
                    fontVariantNumeric: "tabular-nums",
                  }}>+{tasks.filter(x => x.parent === t.id).length || ""}…</span>
                )}
                {deferred && (
                  <span style={{
                    color: "var(--cyan)",
                    border: "1px solid color-mix(in oklab, var(--cyan) 40%, transparent)",
                    borderRadius: 4,
                    padding: "1px 6px",
                    fontSize: 11,
                    display: "inline-flex", alignItems: "center", gap: 4,
                  }}><Icon name="moon" size={10} />{formatDeferUntil(t.deferUntil)}</span>
                )}
                {t.due && (
                  <span style={{
                    color: DUE_COLOR[t.due] || "var(--fg-mute)",
                    border: "1px solid color-mix(in oklab, " + (DUE_COLOR[t.due] || "var(--fg-mute)") + " 40%, transparent)",
                    borderRadius: 4,
                    padding: "1px 6px",
                    fontSize: 11,
                  }}>!{t.due}</span>
                )}
                {t.ctx && (
                  <span style={{ color: CTX_COLOR[t.ctx] || "var(--fg-mute)", fontSize: 12 }}>
                    {t.ctx}
                  </span>
                )}
                <span style={{ color: "var(--fg-faint)", fontSize: 11, minWidth: 28, textAlign: "right" }}>
                  {done && t.doneAt ? timeAgo(t.doneAt) + " ago" : timeAgo(t.created)}
                </span>
              </div>
            </div>
          );
        })}
        <div style={S.endSpacer}>
          <span style={{ color: "var(--fg-faint)" }}>—— end of {listLabel || list} ——</span>
        </div>
      </div>
    </div>
  );
}

// ThemedSelect — a drop-in replacement for <select> that renders the
// expanded menu inside the webview rather than delegating to the GTK
// ComboBox popup (which WebKitGTK paints with the system theme and
// system font, making it look out of place next to the rest of the UI).
//
// API mirrors <select>: `value`, `onChange(newValue)`, `options: [{value,label}]`.
// Keyboard: Enter/Space to open, ↑/↓ to move, Enter to commit, Esc to cancel.
function ThemedSelect({ value, onChange, options, placeholder = '—' }) {
  const [open, setOpen] = aUseState(false);
  const [hovered, setHovered] = aUseState(-1);
  const rootRef = aUseRef(null);
  const menuRef = aUseRef(null);

  const selectedIdx = options.findIndex((o) => o.value === value);
  const currentLabel =
    selectedIdx >= 0 ? options[selectedIdx].label : placeholder;

  aUseEffect(() => {
    if (!open) return undefined;
    setHovered(selectedIdx >= 0 ? selectedIdx : 0);
    const onDocMouseDown = (e) => {
      if (rootRef.current && !rootRef.current.contains(e.target)) setOpen(false);
    };
    document.addEventListener('mousedown', onDocMouseDown);
    return () => document.removeEventListener('mousedown', onDocMouseDown);
  }, [open, selectedIdx]);

  // Keep the hovered row scrolled into view.
  aUseEffect(() => {
    if (!open || !menuRef.current) return;
    const row = menuRef.current.children[hovered];
    if (row && row.scrollIntoView) {
      row.scrollIntoView({ block: 'nearest' });
    }
  }, [hovered, open]);

  const commit = (idx) => {
    const opt = options[idx];
    if (opt) onChange(opt.value);
    setOpen(false);
  };

  const onKey = (e) => {
    if (!open) {
      if (e.key === 'Enter' || e.key === ' ' || e.key === 'ArrowDown') {
        e.preventDefault();
        setOpen(true);
      }
      return;
    }
    if (e.key === 'Escape') { e.preventDefault(); setOpen(false); return; }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setHovered((h) => Math.min(options.length - 1, h + 1));
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      setHovered((h) => Math.max(0, h - 1));
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      commit(hovered);
      return;
    }
    if (e.key === 'Home') { e.preventDefault(); setHovered(0); return; }
    if (e.key === 'End')  { e.preventDefault(); setHovered(options.length - 1); return; }
  };

  return (
    <div ref={rootRef} style={{ position: 'relative' }}>
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        onKeyDown={onKey}
        aria-haspopup="listbox"
        aria-expanded={open}
        style={{ ...S.select, textAlign: 'left', cursor: 'pointer',
          display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}
      >
        <span style={{ color: selectedIdx >= 0 ? 'var(--fg)' : 'var(--fg-faint)',
          overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          {currentLabel}
        </span>
        <span style={{ color: 'var(--fg-mute)', fontSize: 10, flexShrink: 0 }}>▾</span>
      </button>

      {open && (
        <ul
          ref={menuRef}
          role="listbox"
          style={{
            position: 'absolute', top: 'calc(100% + 4px)', left: 0, right: 0,
            zIndex: 40,
            margin: 0, padding: 4, listStyle: 'none',
            background: 'var(--bg-panel)',
            color: 'var(--fg)',
            border: '1px solid var(--border-hi)',
            borderRadius: 6,
            boxShadow: 'var(--shadow)',
            maxHeight: 260, overflowY: 'auto',
            fontFamily: 'inherit', fontSize: 13,
            animation: 'fadein .12s ease-out',
          }}
        >
          {options.map((opt, i) => {
            const isActive = i === hovered;
            const isSelected = i === selectedIdx;
            return (
              <li
                key={opt.value || '__empty__'}
                role="option"
                aria-selected={isSelected}
                onMouseEnter={() => setHovered(i)}
                onMouseDown={(e) => { e.preventDefault(); commit(i); }}
                style={{
                  padding: '6px 10px',
                  borderRadius: 4,
                  cursor: 'pointer',
                  color: isSelected ? 'var(--fg)' : 'var(--fg-dim)',
                  background: isActive
                    ? 'color-mix(in oklab, var(--accent) 22%, transparent)'
                    : 'transparent',
                  outline: isActive
                    ? '1px solid color-mix(in oklab, var(--accent) 36%, transparent)'
                    : 'none',
                  display: 'flex', alignItems: 'center', gap: 8,
                }}
              >
                <span style={{ flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {opt.label}
                </span>
                {isSelected && (
                  <span style={{ color: 'var(--accent)', fontSize: 11 }}>✓</span>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

function Detail({ task, onUpdate, onMove, onDelete, onDefer, onClearDefer, onAddComment, contexts, projects }) {
  const isDone = !!task.doneAt;
  const isDeferred = task.deferUntil && task.deferUntil > Date.now();
  return (
    <div style={S.detailInner}>
      <div style={S.detailHeader}>
        <span style={S.detailKicker}>
          {isDone ? "completed" : isDeferred ? "deferred" : "selected"}
        </span>
        <span style={{ color: "var(--fg-faint)", fontSize: 10.5, fontFamily: "inherit", letterSpacing: 0.4, textTransform: "uppercase" }}>
          {task.ctx ? task.ctx : (task.list === "inbox" ? "inbox" : (projects.find(p => p.id === task.list)?.name || task.list))}
        </span>
      </div>

      <div style={{ ...S.detailTitle, textDecoration: isDone ? "line-through" : "none", opacity: isDone ? .6 : 1 }}>{task.title}</div>

      <div style={S.metaGrid}>
        <div style={S.metaKey}>project</div>
        <div style={S.metaVal}>
          <ThemedSelect
            value={task.list}
            onChange={(v) => onMove(v)}
            options={[
              { value: 'inbox', label: 'inbox' },
              ...projects.map((p) => ({ value: p.id, label: p.name })),
            ]}
          />
        </div>

        <div style={S.metaKey}>context</div>
        <div style={S.metaVal}>
          <ThemedSelect
            value={task.ctx || ''}
            onChange={(v) => onUpdate({ ctx: v })}
            options={[
              { value: '', label: '—' },
              ...contexts.map((c) => ({ value: c, label: c })),
            ]}
          />
        </div>

        <div style={S.metaKey}>due</div>
        <div style={S.metaVal}>
          <ThemedSelect
            value={task.due || ''}
            onChange={(v) => onUpdate({ due: v })}
            options={[
              { value: '', label: '—' },
              { value: 'today', label: 'today' },
              { value: 'tomorrow', label: 'tomorrow' },
              { value: 'this week', label: 'this week' },
            ]}
          />
        </div>

        <div style={S.metaKey}>created</div>
        <div style={S.metaVal}>{timeAgo(task.created)} ago</div>

        {isDone && task.doneAt && (
          <>
            <div style={S.metaKey}>done</div>
            <div style={S.metaVal}>{timeAgo(task.doneAt)} ago</div>
          </>
        )}

        {isDeferred && (
          <>
            <div style={S.metaKey}>waking</div>
            <div style={{ ...S.metaVal, color: "var(--cyan)" }}>{formatDeferUntil(task.deferUntil)}</div>
          </>
        )}
      </div>

      <button onClick={isDeferred ? onClearDefer : onDefer} style={S.deferBtn}>
        <Icon name="moon" size={12} />
        <span>{isDeferred ? "un-defer" : "defer…"}</span>
        <span className="kbd" style={{ marginLeft: "auto" }}>s</span>
      </button>

      <div style={{ marginTop: 20 }}>
        <div style={S.metaKey}>note</div>
        <textarea
          value={task.note || ""}
          onChange={e => onUpdate({ note: e.target.value })}
          placeholder="// add context, links, sub-steps…"
          style={S.textarea}
        />
      </div>

      <Comments task={task} onAddComment={onAddComment} />

      <div style={S.detailFooter}>
        <button onClick={onDelete} style={S.dangerBtn}>
          <Icon name="trash" size={12} />
          <span>delete</span>
          <span className="kbd">d</span><span className="kbd">d</span>
        </button>
      </div>
    </div>
  );
}

// Conversation entries on a task — shared with the iOS/macOS app via the
// Automerge `comments` Map<id, Comment>. Append-only by design: edits and
// deletes are intentionally unsupported in v1 so the merge semantics stay
// simple across devices. Comment object shape matches iOS exactly:
// { id, author, text, createdAt }, all stored as plain scalars.
function Comments({ task, onAddComment }) {
  const [draft, setDraft] = aUseState("");
  const ta = aUseRef(null);
  const author = getCommentAuthor();
  // Comments are an object keyed by commentId (Automerge Map). Display by
  // createdAt ASC so the conversation reads chronologically.
  const comments = aUseMemo(() => {
    const obj = task.comments;
    if (!obj || typeof obj !== "object" || Array.isArray(obj)) return [];
    return Object.values(obj)
      .filter(c => c && typeof c === "object" && typeof c.text === "string")
      .sort((a, b) => (a.createdAt || 0) - (b.createdAt || 0));
  }, [task.comments]);

  const canPost = draft.trim().length > 0;
  const post = () => {
    if (!canPost || !onAddComment) return;
    onAddComment(draft);
    setDraft("");
    if (ta.current) ta.current.focus();
  };

  return (
    <div style={S.comments}>
      <div style={S.commentsHeader}>
        <span style={S.metaKey}>comments</span>
        {comments.length > 0 && (
          <span style={{ color: "var(--fg-faint)", fontSize: 11 }}>
            ({comments.length})
          </span>
        )}
      </div>

      {comments.length === 0 ? (
        <div style={S.commentsEmpty}>no comments yet</div>
      ) : (
        <div style={S.commentList}>
          {comments.map(c => (
            <div key={c.id} style={S.commentItem}>
              <div style={S.commentMeta}>
                <span style={S.commentAuthor}>{c.author || "anon"}</span>
                <span style={S.commentTime}>
                  {c.createdAt ? timeAgo(c.createdAt) + " ago" : ""}
                </span>
              </div>
              <div style={S.commentText}>{c.text}</div>
            </div>
          ))}
        </div>
      )}

      <textarea
        ref={ta}
        value={draft}
        onChange={e => setDraft(e.target.value)}
        onKeyDown={e => {
          // ⌘↵ / Ctrl+↵ → post. Mirrors iOS's keyboard shortcut so the
          // muscle memory carries across devices.
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            post();
          }
        }}
        placeholder={`add a comment as ${author}…`}
        style={S.commentInput}
        rows={2}
      />
      <div style={S.commentActions}>
        <span style={{ color: "var(--fg-faint)", fontSize: 11 }}>
          <span className="kbd">⌘</span><span className="kbd">↵</span> post
        </span>
        <button onClick={post} disabled={!canPost} style={{
          ...S.commentPostBtn,
          opacity: canPost ? 1 : 0.5,
          cursor: canPost ? "pointer" : "default",
        }}>
          post
        </button>
      </div>
    </div>
  );
}

function EmptyDetail() {
  return (
    <div style={{ ...S.detailInner, display: "grid", placeItems: "center", color: "var(--fg-mute)", textAlign: "center" }}>
      <div>
        <div style={{ fontSize: 36, color: "var(--fg-faint)", marginBottom: 8 }}>∅</div>
        <div>no task selected.</div>
        <div style={{ fontSize: 12, marginTop: 6 }}>j/k to move · o to capture</div>
      </div>
    </div>
  );
}

function QuickAdd({ value, setValue, onCommit, onCancel }) {
  const previewRef = aUseRef(null);
  const inputRef = aUseRef(null);
  aUseEffect(() => {
    // focus without scrolling the document (prevents viewport jump on small screens)
    if (inputRef.current) {
      try { inputRef.current.focus({ preventScroll: true }); }
      catch { inputRef.current.focus(); }
    }
  }, []);
  const parsed = parseQuickAdd(value);
  return (
    <div style={S.quickAddWrap}>
      <div style={S.quickAdd}>
        <span style={{ color: "var(--accent)" }}>+</span>
        <input
          ref={inputRef}
          value={value}
          onChange={e => setValue(e.target.value)}
          onKeyDown={e => {
            if (e.key === "Enter")  { e.preventDefault(); onCommit(value); }
            if (e.key === "Escape") { e.preventDefault(); onCancel(); }
          }}
          placeholder="new task…   try:  call dad @phone !today / ask about trip"
          style={S.quickAddInput}
          spellCheck={false}
        />
        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
          {parsed.ctx && <span style={{ color: CTX_COLOR[parsed.ctx] || "var(--fg-mute)", fontSize: 12 }}>{parsed.ctx}</span>}
          {parsed.due && <span style={{ color: DUE_COLOR[parsed.due] || "var(--fg-mute)", fontSize: 12 }}>!{parsed.due}</span>}
          <span style={{ color: "var(--fg-faint)" }}>|</span>
          <span className="kbd">↵</span>
          <span style={{ color: "var(--fg-mute)", fontSize: 11 }}>add</span>
          <span className="kbd">esc</span>
        </div>
      </div>
      <div style={S.quickAddHint}>
        <span><span style={{ color: "var(--accent)" }}>@ctx</span> · context</span>
        <span><span style={{ color: "var(--accent)" }}>!today</span> · due</span>
        <span><span style={{ color: "var(--accent)" }}>/note</span> · inline note</span>
      </div>
    </div>
  );
}

function StatusBar({ mode, activeList, counts, nowTick, cursor, total, projects, syncStatus, onSyncClick }) {
  const modeColor = {
    NORMAL: "var(--accent)",
    INSERT: "var(--success)",
    SEARCH: "var(--warn)",
    CMD:    "var(--accent-2)",
  }[mode] || "var(--fg-mute)";
  const activeName = activeList === "inbox" ? "inbox" : (projects.find(p => p.id === activeList)?.name || activeList);
  const totalActive = Object.values(counts).reduce((a,b) => a+b, 0);
  return (
    <div style={S.statusBar}>
      <span style={{ ...S.statusMode, background: modeColor }}>{mode}</span>
      <span style={S.statusSeg}>~/tasks/<span style={{ color: "var(--fg)" }}>{activeName}</span></span>
      <span style={S.statusSeg}>inbox <b>{counts.inbox||0}</b></span>
      <span style={S.statusSeg}>projects <b>{projects.length}</b></span>
      <span style={S.statusSeg}>active <b>{totalActive}</b></span>
      <span style={{ flex: 1 }} />
      <span style={S.statusSeg}>{cursor+1}:{total||0}</span>
      <span style={S.statusSeg}>utf-8</span>
      <span style={S.statusSeg}>
        <SyncDot status={syncStatus} onClick={onSyncClick} labelled />
      </span>
      <span style={S.statusSeg}>todarchy</span>
    </div>
  );
}

// ---------- Styles ----------
const S = {
  root: {
    width: "100%", height: "100%",
    minWidth: 0, minHeight: 0,
    display: "grid",
    gridTemplateColumns: "minmax(0, 1fr)",
    gridTemplateRows: "minmax(0, 1fr)",
    placeItems: "stretch",
    overflow: "hidden",
  },
  window: {
    position: "relative",
    width: "100%", height: "100%",
    minWidth: 0, minHeight: 0,
    maxWidth: 1480,
    justifySelf: "center",
    alignSelf: "stretch",
    background: "var(--bg)",
    borderRadius: 12,
    overflow: "hidden",
    boxShadow: "var(--shadow)",
    // hyprland-style thin gradient border via outline
    outline: "2px solid transparent",
    backgroundImage: "linear-gradient(var(--bg), var(--bg)), linear-gradient(135deg, var(--accent), var(--accent-2))",
    backgroundOrigin: "border-box",
    backgroundClip: "padding-box, border-box",
    border: "2px solid transparent",
  },
  winInner: {
    width: "100%", height: "100%",
    minWidth: 0, minHeight: 0,
    display: "grid",
    gridTemplateColumns: "240px minmax(0, 1fr) 320px",
    gap: 1,
    background: "var(--border)",
    overflow: "hidden",
  },
  sidebar: {
    background: "var(--bg-elev)",
    padding: "18px 14px",
    display: "flex", flexDirection: "column", gap: 18,
    overflow: "auto",
    minHeight: 0,
  },
  brand: {
    display: "flex", alignItems: "center", gap: 10,
    padding: "4px 4px 12px",
    borderBottom: "1px dashed var(--border)",
  },
  brandDot: {
    width: 10, height: 10, borderRadius: 2,
    background: "linear-gradient(135deg, var(--accent), var(--accent-2))",
    boxShadow: "0 0 12px color-mix(in oklab, var(--accent) 50%, transparent)",
  },
  nav: { display: "flex", flexDirection: "column", gap: 2 },
  navItem: {
    display: "flex", alignItems: "center", gap: 10,
    padding: "8px 10px",
    background: "transparent",
    border: "1px solid transparent",
    borderRadius: 6,
    color: "var(--fg-dim)",
    cursor: "pointer",
    font: "inherit",
    textAlign: "left",
  },
  navItemActive: {
    background: "color-mix(in oklab, var(--accent) 14%, transparent)",
    borderColor: "color-mix(in oklab, var(--accent) 35%, transparent)",
    color: "var(--fg)",
  },
  sidebarSection: { display: "flex", flexDirection: "column", gap: 4 },
  sidebarLabel: {
    color: "var(--fg-mute)", fontSize: 11, letterSpacing: 1,
    textTransform: "uppercase",
    marginBottom: 4,
  },
  ctxRow: {
    display: "flex", alignItems: "center", gap: 8,
    padding: "4px 6px",
    background: "transparent",
    border: "1px solid transparent",
    borderRadius: 4,
    cursor: "pointer",
    color: "inherit", font: "inherit", fontSize: 12.5,
    width: "100%",
  },
  ctxRowActive: {
    background: "color-mix(in oklab, var(--accent) 14%, transparent)",
    borderColor: "color-mix(in oklab, var(--accent) 35%, transparent)",
  },
  clearCtxBtn: {
    background: "transparent", border: 0, cursor: "pointer",
    color: "var(--fg-mute)", fontSize: 10, letterSpacing: 1,
    textTransform: "uppercase", padding: "0 4px",
    font: "inherit",
  },
  ctxIconBtn: {
    display: "inline-grid", placeItems: "center",
    width: 22, height: 20, padding: 0,
    background: "transparent", border: 0, cursor: "pointer",
    color: "var(--fg-mute)", borderRadius: 4,
  },
  projInput: {
    flex: 1, background: "var(--bg)",
    border: "1px solid var(--accent)", outline: 0,
    color: "var(--fg)", font: "inherit", fontSize: 13,
    padding: "1px 6px", borderRadius: 3,
    minWidth: 0,
  },
  projDelBtn: {
    position: "absolute", top: "50%", transform: "translateY(-50%)", right: 6,
    display: "inline-grid", placeItems: "center",
    width: 20, height: 20, padding: 0,
    background: "var(--bg-soft)", border: 0, cursor: "pointer",
    color: "var(--fg-mute)", borderRadius: 4,
  },
  filterToggle: {
    display: "inline-flex", alignItems: "center", gap: 6,
    padding: "3px 8px",
    background: "transparent",
    border: "1px solid var(--border)",
    borderRadius: 4,
    color: "var(--fg-mute)",
    cursor: "pointer",
    font: "inherit", fontSize: 11,
    letterSpacing: .3,
  },
  filterToggleActive: {
    background: "color-mix(in oklab, var(--accent) 14%, transparent)",
    color: "var(--fg)",
    borderColor: "color-mix(in oklab, var(--accent) 35%, transparent)",
  },
  ctxEditName: {
    background: "transparent", border: 0, cursor: "text",
    color: "var(--fg-dim)", font: "inherit", fontSize: 12.5,
    textAlign: "left", padding: 0,
  },
  ctxDelBtn: {
    background: "transparent", border: 0, cursor: "pointer",
    color: "var(--fg-mute)", fontSize: 11, padding: "0 4px",
  },
  ctxAddWrap: {
    display: "flex", alignItems: "center", gap: 6,
    marginTop: 4, padding: "4px 6px",
    borderTop: "1px dashed var(--border)",
  },
  ctxAddInput: {
    flex: 1, background: "var(--bg-soft)",
    border: "1px solid var(--border)", borderRadius: 4,
    color: "var(--fg)", padding: "3px 6px",
    font: "inherit", fontSize: 12, outline: 0,
  },
  hintRow: {
    display: "flex", alignItems: "center", gap: 6,
    padding: "3px 2px",
    fontSize: 11,
  },
  hintLabel: { color: "var(--fg-mute)", marginLeft: 6 },

  main: {
    background: "var(--bg)",
    display: "grid",
    gridTemplateRows: "auto minmax(0, 1fr) auto",
    minWidth: 0,
    minHeight: 0,
    overflow: "hidden",
  },
  header: {
    display: "flex", alignItems: "center", justifyContent: "space-between",
    gap: 12, flexWrap: "wrap",
    padding: "16px 22px",
    borderBottom: "1px solid var(--border)",
    minWidth: 0,
  },
  h1: {
    margin: 0,
    minWidth: 0,
    overflow: "hidden", textOverflow: "ellipsis",
    font: "500 16px/1.2 'JetBrains Mono', monospace",
    color: "var(--fg)",
    display: "flex", alignItems: "center", gap: 4,
    whiteSpace: "nowrap",
  },
  subtle: { color: "var(--fg-mute)", fontSize: 12 },
  headerRight: { display: "flex", alignItems: "center", gap: 8, color: "var(--fg-dim)", fontSize: 12, flexShrink: 0, flexWrap: "wrap", justifyContent: "flex-end" },
  dot: { color: "var(--fg-faint)" },
  count: { color: "var(--fg)", fontVariantNumeric: "tabular-nums" },
  projCount: {
    color: "var(--fg-faint)", fontSize: 10.5,
    fontVariantNumeric: "tabular-nums",
    minWidth: 14, textAlign: "right",
    padding: "1px 5px",
    borderRadius: 3,
    background: "color-mix(in oklab, var(--fg) 5%, transparent)",
  },
  searchBox: {
    display: "flex", alignItems: "center", gap: 8,
    padding: "6px 10px",
    background: "var(--bg-soft)",
    border: "1px solid var(--border-hi)",
    borderRadius: 6,
    minWidth: 260,
  },
  searchInput: {
    flex: 1, background: "transparent", border: 0, outline: 0,
    color: "var(--fg)", fontFamily: "inherit", fontSize: 13,
  },

  listWrap: {
    minHeight: 0,
    overflow: "hidden",
    display: "grid",
    gridTemplateRows: "1fr",
  },
  listScroll: {
    overflowY: "auto",
    padding: "10px 18px 20px",
  },
  row: {
    display: "flex",
    alignItems: "center",
    gap: 8,
    padding: "8px 10px 8px 6px",
    borderRadius: 6,
    cursor: "pointer",
    borderBottom: "1px solid transparent",
  },
  rowActive: {
    background: "color-mix(in oklab, var(--accent) 12%, transparent)",
    outline: "1px solid color-mix(in oklab, var(--accent) 30%, transparent)",
  },
  // Fixed width so the active-row ▍ indicator doesn't shove the rest of the
  // row to the right as the cursor moves.
  rowGutter: { display: "flex", alignItems: "center", justifyContent: "center", height: 20, width: 8, flexShrink: 0 },
  check: {
    background: "transparent", border: 0, padding: 2, cursor: "pointer",
    display: "inline-flex", alignItems: "center", justifyContent: "center",
    height: 20, width: 20,
  },
  rowBody: { minWidth: 0, flex: 1 },
  title: { color: "var(--fg)", wordBreak: "break-word", fontSize: 14 },
  note: { color: "var(--fg-mute)", fontSize: 12, marginTop: 2, fontStyle: "italic" },
  rowMeta: {
    display: "flex", alignItems: "center", gap: 10,
    paddingTop: 1,
  },
  editInput: {
    width: "100%",
    background: "var(--bg-soft)",
    border: "1px solid var(--border-hi)",
    color: "var(--fg)",
    padding: "4px 8px",
    borderRadius: 4,
    outline: 0,
    font: "inherit",
  },
  endSpacer: { textAlign: "center", padding: "18px 0 4px", fontSize: 11 },

  statusBar: {
    display: "flex", alignItems: "center", gap: 14,
    padding: "6px 14px",
    borderTop: "1px solid var(--border)",
    background: "var(--bg-elev)",
    fontSize: 11,
    color: "var(--fg-mute)",
    minWidth: 0,
    overflow: "hidden",
    whiteSpace: "nowrap",
  },
  statusMode: {
    color: "#06060b",
    padding: "2px 8px",
    borderRadius: 3,
    fontWeight: 700,
    letterSpacing: 1,
  },
  statusSeg: { display: "inline-flex", gap: 4 },

  detail: {
    background: "var(--bg-elev)",
    overflow: "auto",
    minWidth: 0,
    minHeight: 0,
  },
  detailInner: {
    padding: "18px 18px 22px",
    display: "flex", flexDirection: "column", gap: 14,
    minHeight: "100%",
  },
  detailHeader: {
    display: "flex", alignItems: "center", justifyContent: "space-between",
    paddingBottom: 6,
    borderBottom: "1px dashed var(--border)",
  },
  detailKicker: {
    color: "var(--fg-mute)", fontSize: 11,
    letterSpacing: 1.5, textTransform: "uppercase",
  },
  detailTitle: {
    color: "var(--fg)", fontSize: 18, lineHeight: 1.35, wordBreak: "break-word",
  },
  metaGrid: {
    display: "grid",
    gridTemplateColumns: "72px 1fr",
    rowGap: 8, columnGap: 10,
    alignItems: "center",
  },
  metaKey: {
    color: "var(--fg-mute)", fontSize: 11,
    letterSpacing: 1, textTransform: "uppercase",
  },
  metaVal: { color: "var(--fg-dim)", fontSize: 13 },
  select: {
    width: "100%",
    background: "var(--bg-soft)",
    color: "var(--fg)",
    border: "1px solid var(--border)",
    borderRadius: 4,
    padding: "4px 8px",
    font: "inherit",
    outline: 0,
  },
  textarea: {
    width: "100%", minHeight: 90, resize: "vertical",
    marginTop: 4,
    background: "var(--bg-soft)",
    color: "var(--fg-dim)",
    border: "1px solid var(--border)",
    borderRadius: 4,
    padding: "8px 10px",
    font: "inherit", fontSize: 12.5,
    outline: 0,
  },
  detailFooter: {
    marginTop: "auto",
    paddingTop: 10,
    borderTop: "1px dashed var(--border)",
    display: "flex", justifyContent: "flex-end",
  },
  dangerBtn: {
    display: "inline-flex", alignItems: "center", gap: 6,
    background: "transparent",
    color: "var(--danger)",
    border: "1px solid color-mix(in oklab, var(--danger) 40%, transparent)",
    borderRadius: 4,
    padding: "4px 8px",
    cursor: "pointer",
    font: "inherit", fontSize: 12,
  },
  deferBtn: {
    display: "inline-flex", alignItems: "center", gap: 6,
    width: "100%",
    marginTop: 4,
    padding: "6px 10px",
    background: "var(--bg-soft)",
    color: "var(--cyan)",
    border: "1px solid color-mix(in oklab, var(--cyan) 35%, transparent)",
    borderRadius: 4,
    cursor: "pointer",
    font: "inherit", fontSize: 12,
  },

  comments: {
    display: "flex", flexDirection: "column", gap: 8,
    marginTop: 8,
  },
  commentsHeader: {
    display: "flex", alignItems: "baseline", gap: 6,
  },
  commentsEmpty: {
    color: "var(--fg-faint)",
    fontSize: 11.5, fontStyle: "italic",
  },
  commentList: {
    display: "flex", flexDirection: "column", gap: 6,
  },
  commentItem: {
    padding: "8px 10px",
    background: "var(--bg-soft)",
    border: "1px solid var(--border)",
    borderRadius: 4,
    display: "flex", flexDirection: "column", gap: 2,
  },
  commentMeta: {
    display: "flex", alignItems: "baseline", gap: 6,
  },
  commentAuthor: {
    color: "var(--fg)", fontSize: 11.5, fontWeight: 600,
  },
  commentTime: {
    color: "var(--fg-faint)", fontSize: 10.5,
  },
  commentText: {
    color: "var(--fg-dim)", fontSize: 12.5,
    whiteSpace: "pre-wrap", wordBreak: "break-word",
  },
  commentInput: {
    width: "100%", minHeight: 44, resize: "vertical",
    background: "var(--bg-soft)",
    color: "var(--fg)",
    border: "1px solid var(--border)",
    borderRadius: 4,
    padding: "6px 8px",
    font: "inherit", fontSize: 12.5,
    outline: 0,
  },
  commentActions: {
    display: "flex", alignItems: "center", justifyContent: "space-between",
  },
  commentPostBtn: {
    background: "var(--accent)",
    color: "var(--bg)",
    border: 0,
    borderRadius: 4,
    padding: "4px 10px",
    font: "inherit", fontSize: 11.5, fontWeight: 600,
  },

  quickAddWrap: {
    position: "absolute",
    left: 24, right: 24, bottom: 24,
    zIndex: 50,
    animation: "fadein .12s ease-out",
  },
  quickAdd: {
    display: "flex", alignItems: "center", gap: 10,
    padding: "10px 14px",
    background: "var(--bg-elev)",
    border: "1px solid var(--border-hi)",
    borderRadius: 8,
    boxShadow: "var(--shadow)",
  },
  quickAddInput: {
    flex: 1, background: "transparent", border: 0, outline: 0,
    color: "var(--fg)", fontFamily: "inherit", fontSize: 16,
  },
  quickAddHint: {
    display: "flex", gap: 18,
    color: "var(--fg-mute)",
    padding: "6px 14px",
    fontSize: 11,
  },

  toast: {
    position: "fixed",
    bottom: 20,
    left: "50%",
    transform: "translateX(-50%)",
    padding: "6px 12px",
    background: "var(--bg-elev)",
    border: "1px solid var(--border-hi)",
    color: "var(--fg)",
    borderRadius: 6,
    fontSize: 12,
    zIndex: 200,
    boxShadow: "var(--shadow)",
    animation: "fadein .12s ease-out",
  },
};

export default App;

function CtxEditor({ contexts, tasks, onAdd, onRename, onDelete, onClose }) {
  const [v, setV] = aUseState("");
  const [renaming, setRenaming] = aUseState(null);
  const [renameVal, setRenameVal] = aUseState("");
  const addRef = aUseRef(null);
  aUseEffect(() => { addRef.current?.focus(); }, []);

  const counts = aUseMemo(() => {
    const m = {};
    tasks.forEach(t => { if (t.ctx) m[t.ctx] = (m[t.ctx]||0)+1; });
    return m;
  }, [tasks]);

  const commitAdd = () => { if (onAdd(v)) setV(""); };
  const commitRename = (c) => { onRename(c, renameVal); setRenaming(null); };

  return (
    <div style={dpStyles.scrim} onMouseDown={onClose}>
      <div style={dpStyles.box} onMouseDown={e => e.stopPropagation()}>
        <div style={dpStyles.head}>
          <span style={{ color: "var(--accent-2)" }}>✎</span>
          <span style={{ color: "var(--fg-mute)", fontSize: 11, letterSpacing: 1.5, textTransform: "uppercase" }}>manage contexts</span>
          <span style={{ flex: 1 }} />
          <span className="kbd">esc</span>
        </div>

        <div style={dpStyles.sectionLabel}>add</div>
        <div style={{ display: "flex", gap: 8, padding: "0 10px 4px" }}>
          <span style={{ color: "var(--accent)", alignSelf: "center" }}>+</span>
          <input ref={addRef} value={v}
            onChange={e => setV(e.target.value)}
            onKeyDown={e => {
              if (e.key === "Enter") { e.preventDefault(); commitAdd(); }
              if (e.key === "Escape") { e.preventDefault(); onClose(); }
            }}
            placeholder="@name"
            style={{ ...dpStyles.input, flex: 1 }} />
          <button onClick={commitAdd} style={dpStyles.confirmBtn}>add <span className="kbd">↵</span></button>
        </div>

        <div style={dpStyles.sectionLabel}>contexts ({contexts.length})</div>
        <div style={{ ...dpStyles.list, maxHeight: 260, overflowY: "auto" }}>
          {contexts.length === 0 && (
            <div style={{ padding: "10px 12px", color: "var(--fg-mute)", fontSize: 12 }}>none yet.</div>
          )}
          {contexts.map(c => {
            const isRenaming = renaming === c;
            const n = counts[c] || 0;
            return (
              <div key={c} style={{ ...dpStyles.row, cursor: "default" }}>
                <span style={{ color: CTX_COLOR[c] || "var(--fg-dim)" }}>●</span>
                {isRenaming ? (
                  <input autoFocus value={renameVal}
                    onChange={e => setRenameVal(e.target.value)}
                    onBlur={() => commitRename(c)}
                    onKeyDown={e => {
                      if (e.key === "Enter") { e.preventDefault(); commitRename(c); }
                      if (e.key === "Escape") { e.preventDefault(); setRenaming(null); }
                    }}
                    style={{ ...dpStyles.input, flex: 1, fontSize: 13 }} />
                ) : (
                  <span style={{ color: "var(--fg)", flex: 1 }}>{c}</span>
                )}
                <span style={{ color: "var(--fg-mute)", fontSize: 11 }}>{n}</span>
                <button onClick={() => { setRenaming(c); setRenameVal(c); }}
                  style={S.ctxIconBtn} title="rename">
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z"/></svg>
                </button>
                <button onClick={() => { if (confirm("Delete " + c + "?" + (n ? " (" + n + " task" + (n>1?"s":"") + " will lose this context)" : ""))) onDelete(c); }}
                  style={{ ...S.ctxIconBtn, color: "var(--danger)" }} title="delete">✕</button>
              </div>
            );
          })}
        </div>

        <div style={dpStyles.footer}>
          <span><span className="kbd">↵</span> add / rename</span>
          <span><span className="kbd">esc</span> close</span>
          <span style={{ marginLeft: "auto", color: "var(--fg-faint)" }}>renames propagate to all tasks</span>
        </div>
      </div>
    </div>
  );
}

function ProjEditor({ projects, tasks, counts, focus, onAdd, onRename, onUpdate, onDelete, onClose }) {
  const PROJ_ICONS = ["folder", "briefcase", "home", "box", "sparkles", "inbox", "clock"];
  const PROJ_ACCENTS = [
    { name: "orange",  v: "var(--orange)"  },
    { name: "accent",  v: "var(--accent)"  },
    { name: "accent-2",v: "var(--accent-2)"},
    { name: "cyan",    v: "var(--cyan)"    },
    { name: "green",   v: "var(--green)"   },
    { name: "magenta", v: "var(--magenta)" },
    { name: "yellow",  v: "var(--yellow)"  },
  ];

  const [v, setV] = aUseState("");
  const [addIcon, setAddIcon] = aUseState("folder");
  const [addAccent, setAddAccent] = aUseState(PROJ_ACCENTS[0].v);
  const [renaming, setRenaming] = aUseState(null);
  const [renameVal, setRenameVal] = aUseState("");
  const [sel, setSel] = aUseState(() => {
    if (focus === "add" || !focus) return -1;
    const i = projects.findIndex(p => p.id === focus);
    return i >= 0 ? i : -1;
  });
  const addRef = aUseRef(null);
  const rootRef = aUseRef(null);

  aUseEffect(() => {
    if (focus === "add" || focus == null) addRef.current?.focus();
    else rootRef.current?.focus();
  }, []);

  const commitAdd = () => {
    const name = v.trim();
    if (!name) return;
    onAdd(name, { icon: addIcon, accent: addAccent });
    setV("");
    setAddIcon("folder");
    setAddAccent(PROJ_ACCENTS[0].v);
  };
  const commitRename = (id) => {
    if (onRename(id, renameVal)) {
      setRenaming(null);
    }
  };
  const confirmDelete = (p) => {
    const n = tasks.filter(t => t.list === p.id).length;
    const ok = confirm("delete project “" + p.name + "”?" + (n ? " " + n + " task" + (n>1?"s":"") + " will move to inbox." : ""));
    if (ok) onDelete(p.id);
  };

  const onKey = (e) => {
    if (renaming) return;
    if (e.key === "Escape") { e.preventDefault(); onClose(); return; }
    if (document.activeElement === addRef.current) return; // let input handle
    if (e.key === "j" || e.key === "ArrowDown") { e.preventDefault(); setSel(s => Math.min(projects.length - 1, (s < 0 ? 0 : s + 1))); }
    else if (e.key === "k" || e.key === "ArrowUp") { e.preventDefault(); setSel(s => s <= 0 ? -1 : s - 1); }
    else if (e.key === "a") { e.preventDefault(); addRef.current?.focus(); }
    else if (e.key === "Enter" && sel >= 0) { e.preventDefault(); const p = projects[sel]; if (p) { setRenaming(p.id); setRenameVal(p.name); } }
    else if ((e.key === "d" || e.key === "Delete" || e.key === "Backspace") && sel >= 0) { e.preventDefault(); const p = projects[sel]; if (p) confirmDelete(p); }
  };

  return (
    <div style={dpStyles.scrim} onMouseDown={onClose}>
      <div style={{ ...dpStyles.box, maxWidth: 560 }} onMouseDown={e => e.stopPropagation()}
           ref={rootRef} tabIndex={-1} onKeyDown={onKey}>
        <div style={dpStyles.head}>
          <span style={{ color: "var(--accent-2)", display: "inline-flex" }}><Icon name="folder" size={14} /></span>
          <span style={{ color: "var(--fg-mute)", fontSize: 11, letterSpacing: 1.5, textTransform: "uppercase" }}>manage projects</span>
          <span style={{ flex: 1 }} />
          <span className="kbd">esc</span>
        </div>

        <div style={dpStyles.sectionLabel}>add</div>
        <div style={{ display: "flex", gap: 8, padding: "0 10px 6px", alignItems: "center" }}>
          <span style={{ color: addAccent, display: "inline-flex" }}><Icon name={addIcon} size={14} /></span>
          <input ref={addRef} value={v}
            onChange={e => setV(e.target.value)}
            onKeyDown={e => {
              if (e.key === "Enter") { e.preventDefault(); commitAdd(); }
              if (e.key === "Escape") { e.preventDefault(); onClose(); }
            }}
            placeholder="project name"
            style={{ ...dpStyles.input, flex: 1 }} />
          <button onClick={commitAdd} style={dpStyles.confirmBtn}>add <span className="kbd">↵</span></button>
        </div>

        <div style={{ display: "flex", gap: 10, padding: "0 10px 4px", alignItems: "center", flexWrap: "wrap" }}>
          <span style={{ color: "var(--fg-faint)", fontSize: 10, letterSpacing: 1, textTransform: "uppercase", minWidth: 40 }}>icon</span>
          <div style={{ display: "flex", gap: 4 }}>
            {PROJ_ICONS.map(ic => (
              <button key={ic} onClick={() => setAddIcon(ic)}
                title={ic}
                style={{
                  ...peStyles.swatch,
                  borderColor: addIcon === ic ? addAccent : "transparent",
                  color: addIcon === ic ? addAccent : "var(--fg-dim)",
                }}>
                <Icon name={ic} size={13} />
              </button>
            ))}
          </div>
        </div>
        <div style={{ display: "flex", gap: 10, padding: "0 10px 10px", alignItems: "center", flexWrap: "wrap" }}>
          <span style={{ color: "var(--fg-faint)", fontSize: 10, letterSpacing: 1, textTransform: "uppercase", minWidth: 40 }}>color</span>
          <div style={{ display: "flex", gap: 4 }}>
            {PROJ_ACCENTS.map(a => (
              <button key={a.name} onClick={() => setAddAccent(a.v)}
                title={a.name}
                style={{
                  ...peStyles.dotSwatch,
                  background: a.v,
                  outline: addAccent === a.v ? "2px solid var(--fg)" : "none",
                  outlineOffset: 2,
                }} />
            ))}
          </div>
        </div>

        <div style={dpStyles.sectionLabel}>projects ({projects.length})</div>
        <div style={{ ...dpStyles.list, maxHeight: 280, overflowY: "auto" }}>
          {projects.length === 0 && (
            <div style={{ padding: "10px 12px", color: "var(--fg-mute)", fontSize: 12 }}>none yet — add one above.</div>
          )}
          {projects.map((p, i) => {
            const isRenaming = renaming === p.id;
            const active = sel === i;
            const n = counts[p.id] || 0;
            return (
              <div key={p.id}
                onMouseEnter={() => setSel(i)}
                style={{ ...dpStyles.row, ...(active ? dpStyles.rowActive : null), cursor: "default" }}>
                <span style={{ color: p.accent, display: "inline-flex" }}><Icon name={p.icon || "folder"} size={14} /></span>
                {isRenaming ? (
                  <input autoFocus value={renameVal}
                    onChange={e => setRenameVal(e.target.value)}
                    onBlur={() => commitRename(p.id)}
                    onKeyDown={e => {
                      if (e.key === "Enter") { e.preventDefault(); commitRename(p.id); }
                      if (e.key === "Escape") { e.preventDefault(); setRenaming(null); }
                    }}
                    style={{ ...dpStyles.input, flex: 1, fontSize: 13 }} />
                ) : (
                  <span style={{ color: "var(--fg)", flex: 1, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{p.name}</span>
                )}
                <span style={{ color: "var(--fg-mute)", fontSize: 11, minWidth: 60, textAlign: "right", fontVariantNumeric: "tabular-nums" }} title="active / total">
                  {n}<span style={{ color: "var(--fg-faint)" }}> / {tasks.filter(t => t.list === p.id).length}</span>
                </span>

                {/* icon cycler */}
                <button onClick={() => {
                    const idx = PROJ_ICONS.indexOf(p.icon || "folder");
                    onUpdate(p.id, { icon: PROJ_ICONS[(idx + 1) % PROJ_ICONS.length] });
                  }}
                  style={peStyles.miniBtn}
                  title="cycle icon">
                  <Icon name={p.icon || "folder"} size={11} />
                </button>

                {/* accent cycler */}
                <button onClick={() => {
                    const idx = PROJ_ACCENTS.findIndex(a => a.v === p.accent);
                    onUpdate(p.id, { accent: PROJ_ACCENTS[(idx + 1) % PROJ_ACCENTS.length].v });
                  }}
                  style={{ ...peStyles.miniBtn, color: p.accent }}
                  title="cycle color">●</button>

                <button onClick={() => { setRenaming(p.id); setRenameVal(p.name); }}
                  style={S.ctxIconBtn} title="rename">
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z"/></svg>
                </button>
                <button onClick={() => confirmDelete(p)}
                  style={{ ...S.ctxIconBtn, color: "var(--danger)" }} title="delete">✕</button>
              </div>
            );
          })}
        </div>

        <div style={dpStyles.footer}>
          <span><span className="kbd">↵</span> add / rename</span>
          <span><span className="kbd">d</span> delete</span>
          <span><span className="kbd">a</span> focus add</span>
          <span><span className="kbd">esc</span> close</span>
          <span style={{ marginLeft: "auto", color: "var(--fg-faint)" }}>tasks follow their project</span>
        </div>
      </div>
    </div>
  );
}

const peStyles = {
  swatch: {
    width: 26, height: 26, borderRadius: 4,
    display: "inline-grid", placeItems: "center",
    background: "var(--bg-elev-2)",
    border: "1px solid transparent",
    color: "var(--fg-dim)",
    cursor: "pointer",
  },
  dotSwatch: {
    width: 16, height: 16, borderRadius: "50%",
    border: "none", cursor: "pointer", padding: 0,
  },
  miniBtn: {
    width: 22, height: 22,
    display: "inline-grid", placeItems: "center",
    background: "transparent",
    border: "1px solid color-mix(in oklab, var(--fg) 12%, transparent)",
    color: "var(--fg-dim)",
    borderRadius: 3, cursor: "pointer",
    font: "inherit", fontSize: 11,
  },
};

function CtxAddRow({ onAdd }) {
  const [v, setV] = aUseState("");
  return (
    <div style={S.ctxAddWrap}>
      <span style={{ color: "var(--accent)" }}>+</span>
      <input value={v}
        onChange={e => setV(e.target.value)}
        onKeyDown={e => {
          if (e.key === "Enter") { if (onAdd(v)) setV(""); }
          if (e.key === "Escape") setV("");
        }}
        placeholder="new context (@name)"
        style={S.ctxAddInput} />
    </div>
  );
}

function DeferPicker({ task, onCancel, onConfirm }) {
  const initTs = task?.deferUntil || defer9am(1);
  const [date, setDate] = aUseState(toInputDate(initTs));
  const [time, setTime] = aUseState(toInputTime(initTs));
  const [sel, setSel] = aUseState(0);
  const dateRef = aUseRef(null);

  const quickOptions = [
    { id: "later-today", label: "later today",     sub: "+3h",                 ts: () => Date.now() + 1000*60*60*3 },
    { id: "tomorrow",    label: "tomorrow",        sub: "09:00",               ts: () => defer9am(1) },
    { id: "weekend",     label: "this weekend",    sub: "saturday 09:00",      ts: () => deferNextWeekday(6) },
    { id: "next-week",   label: "next week",       sub: "monday 09:00",        ts: () => deferNextWeekday(1) },
    { id: "two-weeks",   label: "in two weeks",    sub: "+14d 09:00",          ts: () => defer9am(14) },
    { id: "next-month",  label: "next month",      sub: "+30d 09:00",          ts: () => defer9am(30) },
  ];

  const commitQuick = (opt) => onConfirm(opt.ts(), opt.label);
  const commitPicker = () => {
    const ts = combineDateTime(date, time);
    if (!ts) return;
    onConfirm(ts, null);
  };

  const onKey = (e) => {
    if (e.key === "Escape") { e.preventDefault(); onCancel(); return; }
    if (e.key === "Enter" && (e.target.tagName === "INPUT")) { e.preventDefault(); commitPicker(); return; }
    if (e.key === "Enter") {
      e.preventDefault();
      if (sel < quickOptions.length) commitQuick(quickOptions[sel]);
      else commitPicker();
      return;
    }
    if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) { e.preventDefault(); setSel(s => Math.min(quickOptions.length, s+1)); return; }
    if (e.key === "ArrowUp"   || (e.ctrlKey && e.key === "p")) { e.preventDefault(); setSel(s => Math.max(0, s-1)); return; }
  };

  const previewTs = combineDateTime(date, time);

  return (
    <div style={dpStyles.scrim} onMouseDown={onCancel} onKeyDown={onKey} tabIndex={-1} ref={el => el && el.focus()}>
      <div style={dpStyles.box} onMouseDown={e => e.stopPropagation()}>
        <div style={dpStyles.head}>
          <span style={{ color: "var(--cyan)", display: "inline-flex" }}><Icon name="moon" size={14} /></span>
          <span style={{ color: "var(--fg-mute)", fontSize: 11, letterSpacing: 1.5, textTransform: "uppercase" }}>defer</span>
          <span style={{ color: "var(--fg-dim)", flex: 1, marginLeft: 6, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
            {task?.title}
          </span>
          <span className="kbd">esc</span>
        </div>

        <div style={dpStyles.sectionLabel}>quick</div>
        <div style={dpStyles.list}>
          {quickOptions.map((o, i) => {
            const active = sel === i;
            return (
              <button key={o.id}
                onClick={() => commitQuick(o)}
                onMouseEnter={() => setSel(i)}
                style={{ ...dpStyles.row, ...(active ? dpStyles.rowActive : null) }}>
                <span style={{ color: "var(--cyan)" }}>●</span>
                <span style={{ color: active ? "var(--fg)" : "var(--fg-dim)", flex: 1, textAlign: "left" }}>{o.label}</span>
                <span style={{ color: "var(--fg-mute)", fontSize: 12 }}>{o.sub}</span>
              </button>
            );
          })}
        </div>

        <div style={dpStyles.sectionLabel}>pick a date &amp; time</div>
        <div
          style={{ ...dpStyles.pickerRow, ...(sel === quickOptions.length ? dpStyles.rowActive : null) }}
          onMouseEnter={() => setSel(quickOptions.length)}>
          <input ref={dateRef} type="date" value={date} onChange={e => setDate(e.target.value)}
            onKeyDown={onKey} style={dpStyles.input} />
          <input type="time" value={time} onChange={e => setTime(e.target.value)}
            onKeyDown={onKey} style={dpStyles.input} />
          <button onClick={commitPicker} style={dpStyles.confirmBtn}>
            defer <span className="kbd">↵</span>
          </button>
        </div>
        {previewTs && (
          <div style={dpStyles.preview}>
            → wakes {formatDeferUntil(previewTs)}
          </div>
        )}

        <div style={dpStyles.footer}>
          <span><span className="kbd">↑</span><span className="kbd">↓</span> navigate</span>
          <span><span className="kbd">↵</span> confirm</span>
          <span style={{ marginLeft: "auto", color: "var(--fg-faint)" }}>task moves to /deferred until its time</span>
        </div>
      </div>
    </div>
  );
}

const dpStyles = {
  scrim: {
    position: "fixed", inset: 0, zIndex: 90,
    background: "rgba(0,0,0,.5)", backdropFilter: "blur(4px)",
    display: "grid", placeItems: "start center",
    paddingTop: "14vh",
    animation: "fadein .12s ease-out",
    outline: 0,
  },
  box: {
    width: "min(540px, 92vw)",
    background: "var(--bg-elev)",
    border: "1px solid var(--border-hi)",
    borderRadius: 10,
    boxShadow: "var(--shadow)",
    overflow: "hidden",
    padding: 8,
    fontFamily: "inherit",
  },
  head: {
    display: "flex", alignItems: "center", gap: 8,
    padding: "8px 10px 10px",
    borderBottom: "1px dashed var(--border)",
  },
  sectionLabel: {
    color: "var(--fg-mute)", fontSize: 11, letterSpacing: 1.5,
    textTransform: "uppercase",
    padding: "10px 10px 4px",
  },
  list: { display: "flex", flexDirection: "column", gap: 2, padding: "0 4px" },
  row: {
    display: "flex", alignItems: "center", gap: 10,
    padding: "7px 10px",
    background: "transparent", border: "1px solid transparent",
    borderRadius: 6, cursor: "pointer",
    font: "inherit",
  },
  rowActive: {
    background: "color-mix(in oklab, var(--accent) 16%, transparent)",
    borderColor: "color-mix(in oklab, var(--accent) 30%, transparent)",
  },
  pickerRow: {
    display: "flex", alignItems: "center", gap: 8,
    margin: "0 4px",
    padding: "8px 10px",
    border: "1px solid transparent",
    borderRadius: 6,
  },
  input: {
    background: "var(--bg-soft)",
    color: "var(--fg)",
    border: "1px solid var(--border)",
    borderRadius: 4,
    padding: "5px 8px",
    font: "inherit", fontSize: 13,
    outline: 0,
    colorScheme: "dark",
  },
  confirmBtn: {
    marginLeft: "auto",
    display: "inline-flex", alignItems: "center", gap: 6,
    padding: "5px 10px",
    background: "color-mix(in oklab, var(--cyan) 18%, transparent)",
    color: "var(--cyan)",
    border: "1px solid color-mix(in oklab, var(--cyan) 40%, transparent)",
    borderRadius: 4,
    cursor: "pointer",
    font: "inherit", fontSize: 12,
  },
  preview: {
    padding: "4px 14px 2px",
    color: "var(--fg-mute)", fontSize: 11,
  },
  footer: {
    display: "flex", alignItems: "center", gap: 14,
    padding: "10px 12px 6px",
    marginTop: 4,
    borderTop: "1px solid var(--border)",
    fontSize: 11, color: "var(--fg-mute)",
  },
};

