// restore.rs — fast recovery of a deleted task's text from Automerge change
// bytes. String values (title, note) are stored as contiguous UTF-8 in the
// change value-column, so we scan for readable runs (control bytes delimit
// columns; newlines/tabs are kept so a multi-line note stays one run) and
// surface the longest run containing the search term.
//
//   cargo run -q -p todarchy-core --example restore -- <term>
//
// Read-only. Use the printed text to re-create the task (e.g. via todarchy-mcp
// add_task) — replaying history to rebuild the struct is O(N²) here and slow.

use automerge::Automerge;

fn readable_runs(bytes: &[u8]) -> Vec<String> {
    let mut runs = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    for &b in bytes {
        if b == b'\n' || b == b'\t' || b >= 0x20 {
            cur.push(b);
        } else {
            if cur.len() >= 6 {
                runs.push(String::from_utf8_lossy(&cur).into_owned());
            }
            cur.clear();
        }
    }
    if cur.len() >= 6 {
        runs.push(String::from_utf8_lossy(&cur).into_owned());
    }
    runs
}

fn main() {
    let term = std::env::args().nth(1).unwrap_or_default().to_lowercase();
    if term.is_empty() {
        eprintln!("usage: restore <term>");
        std::process::exit(2);
    }
    let path = dirs::data_local_dir().unwrap_or_default().join("todarchy/tasks.automerge");
    let bytes = std::fs::read(&path).expect("read tasks.automerge");
    let doc = Automerge::load(&bytes).expect("load doc");

    let mut best = String::new();
    for ch in doc.get_changes(&[]) {
        for run in readable_runs(ch.raw_bytes()) {
            if run.to_lowercase().contains(&term) && run.len() > best.len() {
                best = run;
            }
        }
    }

    if best.is_empty() {
        println!("no readable run containing '{term}' in history.");
        return;
    }
    let out = "/tmp/todarchy-recovered.txt";
    std::fs::write(out, &best).expect("write recovered text");
    println!("longest recovered run containing '{term}' ({} bytes) → wrote {out}", best.len());
}
