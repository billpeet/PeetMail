//! List-like settings stored as one setting per entry, `<prefix><id>`, so a later sync can
//! merge edits made on two devices entry by entry (docs/SYNC.md, stage 1). Saving writes only
//! the entries that changed; a reorder renumbers only the entries that moved.

use rusqlite::Connection;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::db;
use crate::error::Result;

/// Every `<prefix>*` entry as (id, value), by id. Unreadable values are skipped.
pub fn load<T: DeserializeOwned>(c: &Connection, prefix: &str) -> Result<Vec<(String, T)>> {
    Ok(db::settings_with_prefix(c, prefix)?
        .into_iter()
        .filter_map(|(id, raw)| match serde_json::from_str(&raw) {
            Ok(v) => Some((id, v)),
            Err(e) => {
                log::warn!("skipping an unreadable {prefix} setting: {e}");
                None
            }
        })
        .collect())
}

/// Makes the `<prefix>*` entries exactly `items`: writes the ones that are new or changed and
/// deletes the rest, in one transaction.
pub fn replace<T: Serialize + DeserializeOwned + PartialEq>(c: &mut Connection, prefix: &str, items: &[(String, T)]) -> Result<()> {
    let existing: Vec<(String, T)> = load(c, prefix)?;
    let tx = c.transaction()?;
    for (id, value) in items {
        if !existing.iter().any(|(eid, ev)| eid == id && ev == value) {
            db::set_setting(&tx, &format!("{prefix}{id}"), &serde_json::to_string(value)?)?;
        }
    }
    for (eid, _) in &existing {
        if !items.iter().any(|(id, _)| id == eid) {
            db::delete_setting(&tx, &format!("{prefix}{eid}"))?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// An entry of an ordered list. The order is a number between its neighbours', so moving one
/// entry changes only that entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ordered<T> {
    #[serde(flatten)]
    pub value: T,
    pub order: f64,
}

/// The `<prefix>*` entries in list order (ties, which only a sync could make, by id).
pub fn load_ordered<T: DeserializeOwned>(c: &Connection, prefix: &str) -> Result<Vec<(String, T)>> {
    let mut items: Vec<(String, Ordered<T>)> = load(c, prefix)?;
    items.sort_by(|a, b| a.1.order.total_cmp(&b.1.order).then_with(|| a.0.cmp(&b.0)));
    Ok(items.into_iter().map(|(id, o)| (id, o.value)).collect())
}

/// Saves `items` as the whole list, in this order.
pub fn save_ordered<T: Serialize + DeserializeOwned + PartialEq + Clone>(c: &mut Connection, prefix: &str, items: &[(String, T)]) -> Result<()> {
    let existing: Vec<(String, Ordered<T>)> = load(c, prefix)?;
    let prev: Vec<Option<f64>> = items
        .iter()
        .map(|(id, _)| existing.iter().find(|(eid, _)| eid == id).map(|(_, o)| o.order))
        .collect();
    let orders = assign_orders(&prev);
    let entries: Vec<(String, Ordered<T>)> = items
        .iter()
        .zip(orders)
        .map(|((id, value), order)| (id.clone(), Ordered { value: value.clone(), order }))
        .collect();
    replace(c, prefix, &entries)
}

/// Orders for a list given each entry's previous order (None: new). The longest run of
/// entries that are already in increasing order keeps its orders; the others get numbers
/// spread between their neighbours. Falls back to 1, 2, 3… if the gaps run out of precision.
pub fn assign_orders(prev: &[Option<f64>]) -> Vec<f64> {
    let prev: Vec<Option<f64>> = prev.iter().map(|o| o.filter(|v| v.is_finite())).collect();
    let n = prev.len();
    // Longest strictly increasing subsequence of the known orders (lists are short: O(n²)).
    let mut len = vec![0usize; n];
    let mut back = vec![None; n];
    for i in 0..n {
        let Some(oi) = prev[i] else { continue };
        len[i] = 1;
        for j in 0..i {
            if let Some(oj) = prev[j]
                && oj < oi
                && len[j] + 1 > len[i]
            {
                len[i] = len[j] + 1;
                back[i] = Some(j);
            }
        }
    }
    let mut keep = vec![false; n];
    let mut at = (0..n).filter(|&i| len[i] > 0).max_by_key(|&i| len[i]);
    while let Some(i) = at {
        keep[i] = true;
        at = back[i];
    }

    let mut out = vec![0.0; n];
    let mut i = 0;
    while i < n {
        if keep[i] {
            out[i] = prev[i].unwrap_or_default();
            i += 1;
            continue;
        }
        let start = i;
        while i < n && !keep[i] {
            i += 1;
        }
        let lo = start.checked_sub(1).map(|s| out[s]);
        let hi = if i < n { prev[i] } else { None };
        let k = (i - start) as f64;
        for (m, slot) in out[start..i].iter_mut().enumerate() {
            let m = m as f64 + 1.0;
            *slot = match (lo, hi) {
                (Some(l), Some(h)) => l + (h - l) * m / (k + 1.0),
                (Some(l), None) => l + m,
                (None, Some(h)) => h - (k + 1.0 - m),
                (None, None) => m,
            };
        }
    }
    if out.windows(2).any(|w| w[0] >= w[1]) {
        return (1..=n).map(|i| i as f64).collect();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Item {
        name: String,
    }

    fn item(name: &str) -> Item {
        Item { name: name.into() }
    }

    fn increasing(v: &[f64]) -> bool {
        v.windows(2).all(|w| w[0] < w[1])
    }

    #[test]
    fn orders_keep_what_they_can() {
        assert_eq!(assign_orders(&[]), Vec::<f64>::new());
        assert_eq!(assign_orders(&[None, None, None]), [1.0, 2.0, 3.0]);
        // Unchanged list: nothing moves.
        assert_eq!(assign_orders(&[Some(1.0), Some(2.0), Some(3.0)]), [1.0, 2.0, 3.0]);
        // Appended and prepended entries go past the ends.
        assert_eq!(assign_orders(&[Some(1.0), Some(2.0), None]), [1.0, 2.0, 3.0]);
        assert_eq!(assign_orders(&[None, Some(1.0), Some(2.0)]), [0.0, 1.0, 2.0]);
        // Inserted between: halfway.
        assert_eq!(assign_orders(&[Some(1.0), None, Some(2.0)]), [1.0, 1.5, 2.0]);
        // Moving the last entry to the front renumbers only that one.
        assert_eq!(assign_orders(&[Some(3.0), Some(1.0), Some(2.0)]), [0.0, 1.0, 2.0]);
        // A swap renumbers one of the pair.
        let swapped = assign_orders(&[Some(2.0), Some(1.0), Some(3.0)]);
        assert!(increasing(&swapped));
        assert_eq!(swapped.iter().filter(|o| [1.0, 2.0, 3.0].contains(o)).count(), 2);
    }

    #[test]
    fn orders_fall_back_to_counting_when_gaps_run_out() {
        let lo = 1.0_f64;
        let hi = f64::from_bits(lo.to_bits() + 1); // no number between the two
        assert_eq!(assign_orders(&[Some(lo), None, Some(hi)]), [1.0, 2.0, 3.0]);
        assert_eq!(assign_orders(&[Some(f64::NAN), None]), [1.0, 2.0]);
    }

    #[test]
    fn repeated_moves_stay_ordered() {
        // Keep moving the last entry to second place, as a user dragging could.
        let db = db::Db::open_in_memory().unwrap();
        db.with(|c| {
            let mut list: Vec<(String, Item)> = (0..6).map(|i| (format!("s{i}"), item(&format!("n{i}")))).collect();
            save_ordered(c, "snippet:", &list)?;
            for _ in 0..200 {
                let last = list.pop().unwrap();
                list.insert(1, last);
                save_ordered(c, "snippet:", &list)?;
                assert_eq!(load_ordered::<Item>(c, "snippet:")?, list);
            }
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn saving_writes_only_what_changed() {
        let db = db::Db::open_in_memory().unwrap();
        db.with(|c| {
            let list = vec![("a".to_string(), item("A")), ("b".to_string(), item("B")), ("c".to_string(), item("C"))];
            save_ordered(c, "snippet:", &list)?;
            // Mark every stored row, then save with one entry renamed and one removed.
            c.execute("UPDATE settings SET value = json_set(value, '$.untouched', 1) WHERE key LIKE 'snippet:%'", [])?;
            let list = vec![("a".to_string(), item("A")), ("c".to_string(), item("C2"))];
            save_ordered(c, "snippet:", &list)?;
            let untouched: Vec<String> = c
                .prepare("SELECT key FROM settings WHERE key LIKE 'snippet:%' AND json_extract(value, '$.untouched') = 1 ORDER BY key")?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            assert_eq!(untouched, ["snippet:a"], "the unchanged entry isn't rewritten, the renamed one is");
            assert_eq!(db::get_setting(c, "snippet:b")?, None, "the removed entry is deleted");
            assert_eq!(load_ordered::<Item>(c, "snippet:")?, list);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn unreadable_entries_are_skipped() {
        let db = db::Db::open_in_memory().unwrap();
        db.with(|c| {
            db::set_setting(c, "snippet:bad", "not json")?;
            db::set_setting(c, "snippet:ok", r#"{"name":"ok","order":1}"#)?;
            assert_eq!(load_ordered::<Item>(c, "snippet:")?, [("ok".to_string(), item("ok"))]);
            Ok(())
        })
        .unwrap();
    }
}
