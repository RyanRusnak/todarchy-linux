// Minimal line icons. Stroke uses currentColor so they inherit text color.
export const Icon = ({ name, size = 14 }) => {
  const s = size;
  const p = {
    width: s, height: s, viewBox: "0 0 24 24",
    fill: "none", stroke: "currentColor", strokeWidth: 1.8,
    strokeLinecap: "round", strokeLinejoin: "round",
  };
  switch (name) {
    case "inbox":  return <svg {...p}><path d="M4 13l3-8h10l3 8v5a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-5z"/><path d="M4 13h4l1 2h6l1-2h4"/></svg>;
    case "arrow":  return <svg {...p}><path d="M5 12h14"/><path d="M13 6l6 6-6 6"/></svg>;
    case "clock":  return <svg {...p}><circle cx="12" cy="12" r="8"/><path d="M12 8v4l3 2"/></svg>;
    case "check":  return <svg {...p}><path d="M5 12l5 5 9-11"/></svg>;
    case "moon":   return <svg {...p}><path d="M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z"/></svg>;
    case "box":    return <svg {...p}><rect x="5" y="5" width="14" height="14" rx="3"/></svg>;
    case "boxOk":  return <svg {...p}><rect x="5" y="5" width="14" height="14" rx="3"/><path d="M9 12l2 2 4-5"/></svg>;
    case "search": return <svg {...p}><circle cx="11" cy="11" r="6"/><path d="M20 20l-4-4"/></svg>;
    case "plus":   return <svg {...p}><path d="M12 5v14M5 12h14"/></svg>;
    case "dot":    return <svg {...p}><circle cx="12" cy="12" r="3" fill="currentColor" stroke="none"/></svg>;
    case "cmd":    return <svg {...p}><path d="M9 6h6v12H9z"/><path d="M6 6a3 3 0 1 1 3 3H6V6zM18 6a3 3 0 1 0-3 3h3V6zM6 18a3 3 0 1 0 3-3H6v3zM18 18a3 3 0 1 1-3-3h3v3z"/></svg>;
    case "slash":  return <svg {...p}><path d="M17 4L7 20"/></svg>;
    case "trash":  return <svg {...p}><path d="M4 7h16"/><path d="M10 11v6M14 11v6"/><path d="M6 7l1 13a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-13"/><path d="M9 7V4h6v3"/></svg>;
    case "briefcase": return <svg {...p}><rect x="3" y="8" width="18" height="12" rx="2"/><path d="M9 8V5a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2v3"/><path d="M3 14h18"/></svg>;
    case "home": return <svg {...p}><path d="M4 11l8-7 8 7"/><path d="M6 10v9a1 1 0 0 0 1 1h3v-6h4v6h3a1 1 0 0 0 1-1v-9"/></svg>;
    case "sparkles": return <svg {...p}><path d="M12 4v5M12 15v5M4 12h5M15 12h5"/><path d="M7 7l2 2M15 15l2 2M7 17l2-2M15 9l2-2"/></svg>;
    case "folder": return <svg {...p}><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7z"/></svg>;
    case "eye": return <svg {...p}><path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7-10-7-10-7z"/><circle cx="12" cy="12" r="3"/></svg>;
    case "eyeOff": return <svg {...p}><path d="M3 3l18 18"/><path d="M10.6 6.1A10 10 0 0 1 22 12s-1.5 2.5-4 4.5"/><path d="M6.5 7.5C3.5 9.5 2 12 2 12s4 7 10 7a9 9 0 0 0 4.5-1.2"/><path d="M9.9 9.9a3 3 0 0 0 4.2 4.2"/></svg>;
    default: return null;
  }
};
