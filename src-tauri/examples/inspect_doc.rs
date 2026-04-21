// Dump the raw structure of a tasks.automerge file so we can tell what
// shape each section is in without guessing.
//
//   cargo run --example inspect_doc -- /path/to/tasks.automerge
//
// Reports whether `tasks`, `projects`, `contexts` are Map, List, or
// missing, along with a count and the first few ids. That's enough to
// answer "is this the old List-schema or the new Map-schema, and how
// many entries survived whatever write last touched it?"

use std::path::PathBuf;

use automerge::{Automerge, ObjType, ReadDoc, Value, ROOT};

fn main() -> anyhow::Result<()> {
    let path: PathBuf = std::env::args()
        .nth(1)
        .expect("usage: cargo run --example inspect_doc -- <path>")
        .into();
    let bytes = std::fs::read(&path)?;
    let doc = Automerge::load(&bytes)?;

    println!("file: {}", path.display());
    println!("size: {} bytes", bytes.len());
    let first4 = bytes.iter().take(4).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
    println!("magic: {first4}   (expected: 85 6f 4a 83)\n");

    for key in ["tasks", "projects", "contexts"] {
        println!("--- {key} ---");
        match doc.get(ROOT, key) {
            Ok(Some((Value::Object(ObjType::Map), id))) => {
                let keys: Vec<String> = doc.keys(&id).collect();
                println!("  shape: Map  (count={})", keys.len());
                for k in &keys {
                    let title = doc
                        .get(&id, k)
                        .ok()
                        .flatten()
                        .and_then(|(v, child)| match v {
                            Value::Object(ObjType::Map) => doc
                                .get(&child, "title")
                                .ok()
                                .flatten()
                                .and_then(|(v, _)| match v {
                                    Value::Scalar(s) => Some(s.to_string()),
                                    _ => None,
                                }),
                            _ => None,
                        })
                        .unwrap_or_default();
                    println!("    • {k}  {title}");
                }
            }
            Ok(Some((Value::Object(ObjType::List), id))) => {
                let len = doc.length(&id);
                println!("  shape: List (count={len})");
                for i in 0..len.min(6) {
                    if let Ok(Some((v, _))) = doc.get(&id, i) {
                        let preview = match v {
                            Value::Object(ObjType::Map) => {
                                // Try to grab a representative field
                                if let Ok(Some((_, child_id))) = doc.get(&id, i) {
                                    let id_val = doc
                                        .get(&child_id, "id")
                                        .ok()
                                        .flatten()
                                        .and_then(|(v, _)| match v {
                                            Value::Scalar(s) => Some(s.to_string()),
                                            _ => None,
                                        })
                                        .unwrap_or_else(|| "<no id>".into());
                                    let title = doc
                                        .get(&child_id, "title")
                                        .ok()
                                        .flatten()
                                        .and_then(|(v, _)| match v {
                                            Value::Scalar(s) => Some(s.to_string()),
                                            _ => None,
                                        })
                                        .unwrap_or_default();
                                    format!("id={id_val} title={title}")
                                } else {
                                    "<map>".into()
                                }
                            }
                            Value::Scalar(s) => s.to_string(),
                            other => format!("{other:?}"),
                        };
                        println!("    [{i}] {preview}");
                    }
                }
                if len > 6 {
                    println!("    … +{} more", len - 6);
                }
            }
            Ok(Some((v, _))) => {
                println!("  shape: UNEXPECTED ({v:?})");
            }
            Ok(None) => {
                println!("  missing!");
            }
            Err(e) => {
                println!("  read error: {e}");
            }
        }
        println!();
    }

    Ok(())
}
