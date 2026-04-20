// Seed data + helpers. GTD-light: inbox + projects. Done & deferred are filters.

// Built-in lists (currently just inbox — projects are dynamic).
const LISTS = [
  { id: "inbox", label: "inbox", icon: "inbox", accent: "var(--orange)", desc: "capture. sort later." },
];

// Seed projects. Users can rename/add/delete these at runtime.
const seedProjects = [
  { id: "p_work",    name: "work",            icon: "briefcase", accent: "var(--accent)"   },
  { id: "p_home",    name: "home",            icon: "home",      accent: "var(--cyan)"     },
  { id: "p_wedding", name: "wedding planning", icon: "sparkles",  accent: "var(--accent-2)" },
];

// format a defer timestamp into a short humanized string
function formatDeferUntil(ts) {
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
Object.assign(window, { formatDeferUntil });

const CONTEXTS = ["@home","@work","@errands","@mac","@phone","@read"];

// pubkey-ish ids (stable for React keys)
let _id = 1000;
const nid = () => {
  // include per-session random + monotonic counter + time to avoid collisions across reloads
  const t = Date.now().toString(36);
  const r = Math.floor(Math.random() * 46656).toString(36).padStart(3, "0");
  return (++_id).toString(36) + t.slice(-4) + r;
};

const seedTasks = [
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
const CTX_COLOR = {
  "@home":    "var(--cyan)",
  "@work":    "var(--accent)",
  "@errands": "var(--warn)",
  "@mac":     "var(--accent-2)",
  "@phone":   "var(--success)",
  "@read":    "var(--orange)",
};

const DUE_COLOR = {
  "today":     "var(--danger)",
  "tomorrow":  "var(--warn)",
  "this week": "var(--accent)",
};

// fuzzy score (char-in-order). higher = better. null = no match.
function fuzzyScore(needle, hay) {
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
function parseQuickAdd(raw) {
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

function timeAgo(ts) {
  const s = Math.floor((Date.now() - ts) / 1000);
  if (s < 60) return s + "s";
  const m = Math.floor(s / 60);
  if (m < 60) return m + "m";
  const h = Math.floor(m / 60);
  if (h < 24) return h + "h";
  const d = Math.floor(h / 24);
  return d + "d";
}

Object.assign(window, {
  LISTS, CONTEXTS, seedTasks, seedProjects, CTX_COLOR, DUE_COLOR,
  nid, fuzzyScore, parseQuickAdd, timeAgo,
});
