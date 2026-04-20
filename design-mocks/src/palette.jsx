// Command palette overlay. Opens on : or cmd-k. Fuzzy-searches commands AND tasks.

const { useState, useEffect, useMemo, useRef } = React;

function Palette({ open, onClose, commands, tasks, projects, onRunCommand, onJumpTask }) {
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const inputRef = useRef(null);
  const listRef = useRef(null);
  const activeRef = useRef(null);

  useEffect(() => {
    if (open) {
      setQ(""); setSel(0);
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  const items = useMemo(() => {
    const needle = q.replace(/^[:>]/, "").trim();
    const isCmdMode = q.startsWith(":") || q.startsWith(">") || needle.length === 0;
    const showTasks = !q.startsWith(":") && !q.startsWith(">");

    const cmdHits = commands
      .map(c => ({ kind: "cmd", item: c, score: fuzzyScore(needle, c.title + " " + (c.hint||"")) }))
      .filter(x => x.score !== null)
      .sort((a,b) => b.score - a.score);

    const taskHits = showTasks ? tasks
      .map(t => ({ kind: "task", item: t, score: fuzzyScore(needle, t.title + " " + (t.ctx||"")) }))
      .filter(x => x.score !== null)
      .sort((a,b) => b.score - a.score)
      .slice(0, 8) : [];

    if (isCmdMode) return [...cmdHits, ...taskHits];
    return [...cmdHits.slice(0, 6), ...taskHits];
  }, [q, commands, tasks]);

  useEffect(() => { setSel(0); }, [q]);

  // keep the active row visible inside the palette's scroll container
  useEffect(() => {
    const el = activeRef.current;
    const parent = listRef.current;
    if (!el || !parent) return;
    const er = el.getBoundingClientRect();
    const pr = parent.getBoundingClientRect();
    if (er.top < pr.top + 4) parent.scrollBy({ top: er.top - pr.top - 4 });
    else if (er.bottom > pr.bottom - 4) parent.scrollBy({ top: er.bottom - pr.bottom + 4 });
  }, [sel, items.length]);

  const onKey = (e) => {
    if (e.key === "Escape") { e.preventDefault(); onClose(); return; }
    if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) { e.preventDefault(); setSel(s => Math.min(items.length-1, s+1)); return; }
    if (e.key === "ArrowUp"   || (e.ctrlKey && e.key === "p")) { e.preventDefault(); setSel(s => Math.max(0, s-1)); return; }
    if (e.key === "Enter") {
      e.preventDefault();
      const hit = items[sel];
      if (!hit) return;
      if (hit.kind === "cmd")  onRunCommand(hit.item);
      if (hit.kind === "task") onJumpTask(hit.item);
    }
  };

  if (!open) return null;

  return (
    <div style={pStyles.scrim} onMouseDown={onClose}>
      <div style={pStyles.box} onMouseDown={e => e.stopPropagation()}>
        <div style={pStyles.prompt}>
          <span style={{ color: "var(--accent)" }}>:</span>
          <input
            ref={inputRef}
            value={q}
            onChange={e => setQ(e.target.value)}
            onKeyDown={onKey}
            placeholder="type a command or search tasks…   (esc to close)"
            style={pStyles.input}
            spellCheck={false}
          />
        </div>
        <div ref={listRef} style={pStyles.list}>
          {items.length === 0 && (
            <div style={pStyles.empty}>no matches.</div>
          )}
          {items.map((hit, i) => {
            const active = i === sel;
            if (hit.kind === "cmd") {
              const c = hit.item;
              return (
                <div key={"c"+c.id}
                  ref={active ? activeRef : null}
                  style={{ ...pStyles.row, ...(active ? pStyles.rowActive : null) }}
                  onMouseEnter={() => setSel(i)}
                  onClick={() => onRunCommand(c)}>
                  <div style={pStyles.rowLeft}>
                    <span style={pStyles.rowBadge}>cmd</span>
                    <span style={{ color: active ? "var(--fg)" : "var(--fg-dim)" }}>{c.title}</span>
                    {c.hint && <span style={pStyles.hint}>— {c.hint}</span>}
                  </div>
                  {c.keys && (
                    <div style={{ display: "flex", gap: 4 }}>
                      {c.keys.map((k,ix) => <span key={ix} className="kbd">{k}</span>)}
                    </div>
                  )}
                </div>
              );
            }
            const t = hit.item;
            const done = !!t.doneAt;
            const label = t.list === "inbox" ? "inbox" : ((projects||[]).find(p => p.id === t.list)?.name || t.list);
            return (
              <div key={"t"+t.id}
                ref={active ? activeRef : null}
                style={{ ...pStyles.row, ...(active ? pStyles.rowActive : null) }}
                onMouseEnter={() => setSel(i)}
                onClick={() => onJumpTask(t)}>
                <div style={pStyles.rowLeft}>
                  <span style={{ ...pStyles.rowBadge, background: "transparent", color: "var(--fg-mute)" }}>
                    {label}
                  </span>
                  <span style={{ color: active ? "var(--fg)" : "var(--fg-dim)", textDecoration: done ? "line-through" : "none", opacity: done ? .6 : 1 }}>
                    {t.title}
                  </span>
                </div>
                {t.ctx && (
                  <span style={{ color: CTX_COLOR[t.ctx] || "var(--fg-mute)", fontSize: 12 }}>{t.ctx}</span>
                )}
              </div>
            );
          })}
        </div>
        <div style={pStyles.footer}>
          <span><span className="kbd">↑</span> <span className="kbd">↓</span> navigate</span>
          <span><span className="kbd">↵</span> run</span>
          <span><span className="kbd">esc</span> close</span>
          <span style={{ marginLeft: "auto", color: "var(--fg-faint)" }}>: commands · plain text searches tasks</span>
        </div>
      </div>
    </div>
  );
}

const pStyles = {
  scrim: {
    position: "fixed", inset: 0, zIndex: 80,
    background: "rgba(0,0,0,.45)",
    backdropFilter: "blur(4px)",
    display: "grid",
    placeItems: "start center",
    paddingTop: "12vh",
    animation: "fadein .12s ease-out",
  },
  box: {
    width: "min(640px, 92vw)",
    background: "var(--bg-elev)",
    border: "1px solid var(--border-hi)",
    borderRadius: 10,
    boxShadow: "var(--shadow)",
    overflow: "hidden",
    fontFamily: "inherit",
  },
  prompt: {
    display: "flex", alignItems: "center", gap: 8,
    padding: "12px 14px",
    borderBottom: "1px solid var(--border)",
  },
  input: {
    flex: 1,
    background: "transparent", border: 0, outline: 0,
    color: "var(--fg)",
    fontFamily: "inherit", fontSize: 14,
  },
  list: {
    maxHeight: 360, overflowY: "auto",
    padding: 6,
  },
  empty: {
    padding: "24px 14px",
    color: "var(--fg-mute)", textAlign: "center",
  },
  row: {
    display: "flex", alignItems: "center", justifyContent: "space-between",
    padding: "7px 10px",
    borderRadius: 6,
    cursor: "pointer",
    gap: 12,
  },
  rowActive: {
    background: "color-mix(in oklab, var(--accent) 18%, transparent)",
    outline: "1px solid color-mix(in oklab, var(--accent) 30%, transparent)",
  },
  rowLeft: { display: "flex", alignItems: "center", gap: 10, minWidth: 0, flex: 1 },
  rowBadge: {
    display: "inline-grid", placeItems: "center",
    minWidth: 42,
    padding: "2px 6px",
    background: "var(--bg-soft)",
    color: "var(--fg-mute)",
    borderRadius: 4,
    fontSize: 11, letterSpacing: .3, textTransform: "uppercase",
  },
  hint: { color: "var(--fg-mute)", fontSize: 12 },
  footer: {
    display: "flex", alignItems: "center", gap: 14,
    padding: "8px 14px",
    borderTop: "1px solid var(--border)",
    fontSize: 11, color: "var(--fg-mute)",
    background: "var(--bg)",
  },
};

window.Palette = Palette;
