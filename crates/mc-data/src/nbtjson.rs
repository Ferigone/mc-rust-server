//! Konwersja JSON (format datapacków) -> NBT. Odpowiada ścieżce vanilla `codec.encodeStart(NbtOps)`
//! z jedną różnicą: nie znamy typów pól z kodeków, więc liczby całkowite są `Int`/`Long`,
//! a ułamkowe `Double`. Klient dekoduje liczby z NbtOps tolerancyjnie (każdy `NumericTag`),
//! więc jest to równoważne semantycznie; bajty mogą się różnić od vanilla typem znacznika.

use mc_protocol::nbt::{Compound, Tag};
use serde_json::Value;

pub fn json_to_nbt(v: &Value) -> Tag {
    match v {
        Value::Null => Tag::Compound(Compound::new()),
        Value::Bool(b) => Tag::Byte(*b as i8),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                if let Ok(i) = i32::try_from(i) {
                    Tag::Int(i)
                } else {
                    Tag::Long(i)
                }
            } else {
                Tag::Double(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => Tag::String(s.clone()),
        Value::Array(a) => {
            let items: Vec<Tag> = a.iter().map(json_to_nbt).collect();
            Tag::List(unify_list(items))
        }
        Value::Object(o) => Tag::Compound(Compound(o.iter().map(|(k, v)| (k.clone(), json_to_nbt(v))).collect())),
    }
}

/// Lista NBT musi być jednorodna: mieszane liczby promujemy do `Double`/`Long`,
/// a mieszane typy opakowujemy tak jak `NbtOps` (compound z kluczem pustym).
fn unify_list(items: Vec<Tag>) -> Vec<Tag> {
    let Some(first) = items.first() else { return items };
    let id = first.id();
    if items.iter().all(|t| t.id() == id) {
        return items;
    }
    let numeric = |t: &Tag| matches!(t, Tag::Int(_) | Tag::Long(_) | Tag::Double(_) | Tag::Float(_) | Tag::Byte(_) | Tag::Short(_));
    if items.iter().all(numeric) {
        return items
            .into_iter()
            .map(|t| match t {
                Tag::Byte(v) => Tag::Double(v as f64),
                Tag::Short(v) => Tag::Double(v as f64),
                Tag::Int(v) => Tag::Double(v as f64),
                Tag::Long(v) => Tag::Double(v as f64),
                Tag::Float(v) => Tag::Double(v as f64),
                other => other,
            })
            .collect();
    }
    items.into_iter().map(|t| if matches!(t, Tag::Compound(_)) { t } else { Tag::Compound(Compound::new().with("", t)) }).collect()
}
