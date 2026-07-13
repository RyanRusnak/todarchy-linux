// recover.rs — scan the Automerge doc's full history for task titles matching
// a term, including tasks that were later deleted (their data survives in the
// change history even though the current projection tombstones them).
//
//   cargo run -q -p todarchy-core --example recover -- <term> [path]
//
// Read-only: loads and replays changes; never writes.

use std::collections::BTreeSet;

use automerge::{Automerge, ObjType, ReadDoc, ScalarValue, Value, ROOT};

fn get_str(doc: &Automerge, obj: &automerge::ObjId, key: &str) -> String {
    match doc.get(obj, key) {
        Ok(Some((Value::Scalar(s), _))) => match s.as_ref() {
            ScalarValue::Str(st) => st.to_string(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let term = args.next().unwrap_or_default().to_lowercase();
    if term.is_empty() {
        eprintln!("usage: recover <term> [path-to-tasks.automerge]");
        std::process::exit(2);
    }
    let path = args.next().unwrap_or_else(|| {
        dirs::data_local_dir()
            .unwrap_or_default()
            .join("todarchy/tasks.automerge")
            .to_string_lossy()
            .into_owned()
    });

    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("cannot read {path}: {e}");
        std::process::exit(1);
    });
    let full = Automerge::load(&bytes).expect("load automerge doc");

    // Titles still present in the current projection (so we can say whether a
    // history hit is live or was deleted).
    let mut present: BTreeSet<String> = BTreeSet::new();
    if let Ok(Some((Value::Object(ObjType::Map), tasks))) = full.get(ROOT, "tasks") {
        for id in full.keys(&tasks) {
            if let Ok(Some((Value::Object(ObjType::Map), entry))) = full.get(&tasks, &id) {
                present.insert(get_str(&full, &entry, "title").to_lowercase());
            }
        }
    }

    // Fast path: every string value ever written lives in the change history.
    // Scan each change's raw bytes for the term (string values are stored as
    // UTF-8), then pull readable ASCII runs around the hit for context.
    let changes = full.get_changes(&[]);
    let needle = term.as_bytes();
    let mut hits: BTreeSet<String> = BTreeSet::new();
    for ch in &changes {
        let raw = ch.raw_bytes();
        let lower: Vec<u8> = raw.iter().map(|b| b.to_ascii_lowercase()).collect();
        let mut i = 0;
        while let Some(pos) = find(&lower[i..], needle) {
            let at = i + pos;
            // widen to the surrounding printable run
            let mut s = at;
            while s > 0 && is_readable(raw[s - 1]) {
                s -= 1;
            }
            let mut e = at + needle.len();
            while e < raw.len() && is_readable(raw[e]) {
                e += 1;
            }
            if let Ok(text) = std::str::from_utf8(&raw[s..e]) {
                let t = text.trim();
                if t.len() >= term.len() {
                    hits.insert(t.to_string());
                }
            }
            i = at + needle.len();
        }
    }

    println!("scanned {} changes in history for '{term}'", changes.len());
    if hits.is_empty() {
        println!("→ no match: a task containing '{term}' was NEVER in this device's doc.");
        return;
    }
    println!("→ found {} readable fragment(s) containing '{term}':", hits.len());
    for h in &hits {
        let live = if present.iter().any(|t| t.contains(&term) && t == &h.to_lowercase()) {
            "still present"
        } else {
            "not in current tasks (deleted or edited)"
        };
        println!("  • {:?}   [{live}]", h);
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn is_readable(b: u8) -> bool {
    b == b' ' || b == b'\'' || (b'!'..=b'~').contains(&b)
}
