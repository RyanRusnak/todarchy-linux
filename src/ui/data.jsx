// Seed data + helpers. GTD-light: inbox + projects. Done & deferred are filters.

// Built-in lists (currently just inbox — projects are dynamic).
export const LISTS = [
  { id: "inbox", label: "inbox", icon: "inbox", accent: "var(--orange)", desc: "capture. sort later." },
];

// Seed projects. Users can rename/add/delete these at runtime.
export const seedProjects = [
  { id: "p_work",    name: "work",            icon: "briefcase", accent: "var(--accent)"   },
  { id: "p_home",    name: "home",            icon: "home",      accent: "var(--cyan)"     },
  { id: "p_wedding", name: "wedding planning", icon: "sparkles",  accent: "var(--accent-2)" },
];

// format a defer timestamp into a short humanized string
export function formatDeferUntil(ts) {
  if (!ts) return "";
  const d = new Date(ts);
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  const tomorrow = new Date(now); tomorrow.setDate(now.getDate()+1);
  const isTomorrow = d.toDateString() === tomorrow.toDateString();
  const diffDays = Math.round((d - now) / (1000*60*60*24));
  const t = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
  if (sameDay)    return "today " + t;
  if (isTomorrow) return "tomorrow " + t;
  if (diffDays > 1 && diffDays < 7) return d.toLocaleDateString([], { weekday: "short" }).toLowerCase() + " " + t;
  return d.toLocaleDateString([], { month: "short", day: "numeric" }).toLowerCase() + " " + t;
}

export const CONTEXTS = ["@home","@work","@errands","@mac","@phone","@read"];

// pubkey-ish ids (stable for React keys)
let _id = 1000;
// Task/project id generator. Emits RFC 4122 UUIDs so Apple platforms can
// decode the ids via Swift's strict `UUID(uuidString:)` initializer — the
// earlier short-id scheme (e.g. "sdndt3gvg") made Linux-created tasks
// invisible on macOS/iOS because their decoder silently dropped any
// entry whose id didn't parse as a UUID.
export const nid = () => {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  // Fallback for jsdom / older WebViews that don't expose crypto.randomUUID.
  // RFC 4122 v4 assembly from getRandomValues.
  const bytes = new Uint8Array(16);
  (typeof crypto !== 'undefined' && crypto.getRandomValues)
    ? crypto.getRandomValues(bytes)
    : bytes.forEach((_, i) => { bytes[i] = Math.floor(Math.random() * 256); });
  bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
  bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant
  const h = Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
};

export const seedTasks = [
  // inbox
  { id: nid(), list: "inbox",    title: "figure out btrfs snapshots for /home",                ctx: "@mac",     note: "limine + snapper. hyprland wiki link in bookmarks.", created: Date.now()-1000*60*60*2 },
  { id: nid(), list: "inbox",    title: "reply to matteo re. keyboard layout swap",            ctx: "@work",    note: "", created: Date.now()-1000*60*60 },
  { id: nid(), list: "inbox",    title: "pick up pasta + tomatoes",                            ctx: "@errands", note: "", created: Date.now()-1000*60*30 },

  // work
  { id: nid(), list: "p_work",   title: "draft Q2 planning doc",                               ctx: "@work",    note: "3 themes. link: notion://planning-q2", created: Date.now()-1000*60*60*26, due: "today" },
  { id: nid(), list: "p_work",   title: "review pull requests (3)",                            ctx: "@work",    note: "#412 #418 #421", created: Date.now()-1000*60*60*9, due: "tomorrow" },
  { id: nid(), list: "p_work",   title: "1:1 prep with rohan",                                 ctx: "@work",    note: "growth areas · q2 goals", created: Date.now()-1000*60*60*14, due: "this week" },
  { id: nid(), list: "p_work",   title: "update team OKRs in notion",                          ctx: "@work",    note: "", created: Date.now()-1000*60*60*48, deferUntil: Date.now()+1000*60*60*24*2 },
  { id: nid(), list: "p_work",   title: "kickoff infra migration",                             ctx: "@work",    note: "", created: Date.now()-1000*60*60*24*3, doneAt: Date.now()-1000*60*60*24 },

  // home
  { id: nid(), list: "p_home",   title: "rewrite hyprland keybinds for tiling sanity",         ctx: "@mac",     note: "super+h/j/k/l for focus.\nbind = SUPER, h, movefocus, l", created: Date.now()-1000*60*60*20, due: "today" },
  { id: nid(), list: "p_home",   title: "20min run + stretch",                                 ctx: "@home",    note: "", created: Date.now()-1000*60*60*5, due: "today" },
  { id: nid(), list: "p_home",   title: "call dentist, move to thursday",                      ctx: "@phone",   note: "", created: Date.now()-1000*60*60*50, due: "this week" },
  { id: nid(), list: "p_home",   title: "build mechanical keyboard from kit",                  ctx: "@home",    note: "lily58 // choc low-pro", created: Date.now()-1000*60*60*24*10 },
  { id: nid(), list: "p_home",   title: "follow up with the landlord re. lease",               ctx: "@phone",   note: "", created: Date.now()-1000*60*60*24*2, deferUntil: Date.now()+1000*60*60*20 },
  { id: nid(), list: "p_home",   title: "migrate dotfiles to chezmoi",                         ctx: "@mac",     note: "", created: Date.now()-1000*60*60*24*2, doneAt: Date.now()-1000*60*60*20 },

  // wedding planning
  { id: nid(), list: "p_wedding", title: "finalize guest list (round 2)",                      ctx: "@home",    note: "target: 110 ± 10", created: Date.now()-1000*60*60*30, due: "this week" },
  { id: nid(), list: "p_wedding", title: "tasting appointment — saturday 2pm",                 ctx: "@errands", note: "bring notes from rehearsal", created: Date.now()-1000*60*60*48, due: "this week" },
  { id: nid(), list: "p_wedding", title: "send save-the-dates",                                ctx: "@errands", note: "", created: Date.now()-1000*60*60*24*4, doneAt: Date.now()-1000*60*60*24*2 },
  { id: nid(), list: "p_wedding", title: "book florist consultation",                          ctx: "@phone",   note: "", created: Date.now()-1000*60*60*24*6, deferUntil: Date.now()+1000*60*60*24*3 },
  { id: nid(), list: "p_wedding", title: "first dance song shortlist",                         ctx: "@read",    note: "", created: Date.now()-1000*60*60*24*7 },
  { id: nid(), list: "p_wedding", title: "research honeymoon destinations",                    ctx: "@read",    note: "lisbon / tokyo / crete", created: Date.now()-1000*60*60*24*12 },
];

// context -> color
export const CTX_COLOR = {
  "@home":    "var(--cyan)",
  "@work":    "var(--accent)",
  "@errands": "var(--warn)",
  "@mac":     "var(--accent-2)",
  "@phone":   "var(--success)",
  "@read":    "var(--orange)",
};

export const DUE_COLOR = {
  "today":     "var(--danger)",
  "tomorrow":  "var(--warn)",
  "this week": "var(--accent)",
};

// fuzzy score (char-in-order). higher = better. null = no match.
export function fuzzyScore(needle, hay) {
  if (!needle) return 0.0001;
  needle = needle.toLowerCase();
  hay = hay.toLowerCase();
  let hi = 0, score = 0, streak = 0;
  for (let i = 0; i < needle.length; i++) {
    const c = needle[i];
    const idx = hay.indexOf(c, hi);
    if (idx === -1) return null;
    score += 1 / (1 + (idx - hi));
    if (idx === hi) { streak++; score += streak * 0.5; } else streak = 0;
    hi = idx + 1;
  }
  return score;
}

// quick-add parser: "read @read !today /note here"
export function parseQuickAdd(raw) {
  const t = { title: raw, ctx: "", due: "", note: "" };
  // context
  const ctxM = raw.match(/(@\w+)/);
  if (ctxM) { t.ctx = ctxM[1]; t.title = t.title.replace(ctxM[1], ""); }
  // due
  const dueM = raw.match(/!(today|tomorrow|week)/i);
  if (dueM) {
    t.due = dueM[1].toLowerCase() === "week" ? "this week" : dueM[1].toLowerCase();
    t.title = t.title.replace(dueM[0], "");
  }
  // note after '/'
  const noteM = raw.match(/\s\/(.+)$/);
  if (noteM) { t.note = noteM[1].trim(); t.title = t.title.replace(noteM[0], ""); }
  t.title = t.title.replace(/\s+/g," ").trim();
  return t;
}

// Natural-language defer parser — mirrors the macOS `DeferParser` so the two
// platforms accept the same typed input. Returns a timestamp (ms) at 09:00
// local time for relative forms, or null when the text isn't recognized.
//   today · tomorrow / tmrw · +3d / +1w / +1m · mon..sun · weekend ·
//   next week · YYYY-MM-DD
const DEFER_WEEKDAYS = { sun: 0, mon: 1, tue: 2, wed: 3, thu: 4, fri: 5, sat: 6 };
function deferAtNine(date) {
  date.setHours(9, 0, 0, 0);
  return date.getTime();
}
function deferNextDow(targetDow) {
  const d = new Date();
  const delta = (targetDow - d.getDay() + 7) % 7 || 7; // always strictly ahead
  d.setDate(d.getDate() + delta);
  return deferAtNine(d);
}
export function parseDeferText(input) {
  const s = (input || "").trim().toLowerCase();
  if (!s) return null;

  if (s === "today") return deferAtNine(new Date());
  if (s === "tomorrow" || s === "tmrw") {
    const d = new Date(); d.setDate(d.getDate() + 1); return deferAtNine(d);
  }
  if (s === "weekend" || s === "this weekend") return deferNextDow(6); // saturday
  if (s === "next week") return deferNextDow(1);                       // monday

  // +Nd / +Nw / +Nm
  const rel = s.match(/^\+(\d+)\s*([dwm])$/);
  if (rel) {
    const n = parseInt(rel[1], 10);
    const d = new Date();
    if (rel[2] === "d") d.setDate(d.getDate() + n);
    else if (rel[2] === "w") d.setDate(d.getDate() + n * 7);
    else d.setMonth(d.getMonth() + n);
    return deferAtNine(d);
  }

  // weekday abbreviation
  if (Object.prototype.hasOwnProperty.call(DEFER_WEEKDAYS, s)) {
    return deferNextDow(DEFER_WEEKDAYS[s]);
  }

  // ISO date YYYY-MM-DD
  const iso = s.match(/^(\d{4})-(\d{2})-(\d{2})$/);
  if (iso) {
    const [, y, m, day] = iso.map(Number);
    const d = new Date(y, m - 1, day);
    // reject impossible dates (e.g. 2025-02-31 rolling over)
    if (d.getFullYear() === y && d.getMonth() === m - 1 && d.getDate() === day) {
      return deferAtNine(d);
    }
  }

  return null;
}

// Export the store as a human-readable Markdown checklist, grouped by list.
// Mirrors macOS `ExportImport.exportMarkdown`.
export function exportMarkdown(tasks, projects, now = Date.now()) {
  const lines = [`# todarchy — ${new Date(now).toISOString()}`, ""];
  const heading = (listId) =>
    listId === "inbox" ? "inbox" : (projects.find(p => p.id === listId)?.name || listId);
  const listIds = ["inbox", ...projects.map(p => p.id)];
  for (const listId of listIds) {
    const forList = tasks.filter(t => t.list === listId);
    if (forList.length === 0) continue;
    lines.push(`## ${heading(listId)}`, "");
    for (const task of forList) {
      const box = task.doneAt ? "[x]" : "[ ]";
      let title = task.title || "";
      if (task.ctx) title += ` ${task.ctx}`;
      if (task.due) title += ` !${task.due === "this week" ? "week" : task.due}`;
      lines.push(`- ${box} ${title}`);
      if (task.note) {
        for (const line of String(task.note).split("\n")) lines.push(`  > ${line}`);
      }
    }
    lines.push("");
  }
  return lines.join("\n");
}

export function timeAgo(ts) {
  const s = Math.floor((Date.now() - ts) / 1000);
  if (s < 60) return s + "s";
  const m = Math.floor(s / 60);
  if (m < 60) return m + "m";
  const h = Math.floor(m / 60);
  if (h < 24) return h + "h";
  const d = Math.floor(h / 24);
  return d + "d";
}

// Per-device display name stamped on new task comments. Matches the iOS
// app's `CommentAuthor` UserDefaults key (`todarchy.comment.displayName`)
// so the two platforms behave consistently — set the name in either app
// and freshly-posted comments use it. The value is intentionally local
// per device (not synced) so each user controls their own identity.
export const COMMENT_AUTHOR_KEY = "todarchy.comment.displayName";

export function getCommentAuthor() {
  try {
    const stored = (localStorage.getItem(COMMENT_AUTHOR_KEY) || "").trim();
    if (stored) return stored;
  } catch { /* localStorage may be unavailable in jsdom */ }
  return "Me";
}

export function setCommentAuthor(name) {
  const trimmed = (name || "").trim();
  try {
    if (!trimmed) localStorage.removeItem(COMMENT_AUTHOR_KEY);
    else localStorage.setItem(COMMENT_AUTHOR_KEY, trimmed);
  } catch { /* ignore — display name will fall back to "Me" */ }
}

