//! Which settings follow you between devices (docs/SYNC.md). Every settings key is either
//! synced or local; `db::set_setting` refuses (in debug builds) one that is neither, and a test
//! scans the source for keys, so each new setting gets a deliberate choice.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Follows you to your other devices.
    Synced,
    /// Belongs to this device: screen-dependent layout, cache sizes, consent, sync cursors.
    Local,
}

const SYNCED: &[&str] = &[
    "ms_client_id",
    "google_client_id",
    "google_client_secret",
    "hide_empty_splits",
    "week_starts_on",
    "undo_send_secs",
    "notify",
];

/// One setting per entry (see `items.rs`).
const SYNCED_PREFIXES: &[&str] = &["split:", "snippet:", "signature:"];

const LOCAL: &[&str] = &[
    "cache_days",
    "body_cache_mb",
    "show_right_pane",
    "right_pane_width",
    "right_pane_tab",
    "layout",
    "list_width",
    "close_to_tray",
    "privacy_notice_seen",
    "changelog_seen",
    "crash_reports",
    "usage_data",
    "install_id",
    "update_channel",
    "restarted_to_update",
    "fake_autostart",
    // Message ids are this database's own (db::clear_search_cache).
    "message_id_floor",
];

/// Per-account bookkeeping. Account ids are this device's own, so these could never sync.
const LOCAL_PREFIXES: &[&str] = &["gmail_history:", "gmail_snooze_label:", "gmail_all_folders:", "gmail_spam:", "notified_through_id:", "snooze_folder:"];

pub fn scope(key: &str) -> Option<Scope> {
    if SYNCED.contains(&key) || SYNCED_PREFIXES.iter().any(|p| key.starts_with(p)) {
        Some(Scope::Synced)
    } else if LOCAL.contains(&key) || LOCAL_PREFIXES.iter().any(|p| key.starts_with(p)) {
        Some(Scope::Local)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_key_is_both_synced_and_local() {
        for k in SYNCED {
            assert!(!LOCAL.contains(k) && !LOCAL_PREFIXES.iter().any(|p| k.starts_with(p)), "{k} is in both lists");
        }
        for p in SYNCED_PREFIXES {
            assert!(!LOCAL_PREFIXES.iter().any(|l| l.starts_with(p) || p.starts_with(l)), "{p} overlaps a local prefix");
        }
    }

    #[test]
    fn classifies_entries_and_bookkeeping() {
        assert_eq!(scope("split:other"), Some(Scope::Synced));
        assert_eq!(scope("signature:priya@fabrikam.io"), Some(Scope::Synced));
        assert_eq!(scope("gmail_history:3"), Some(Scope::Local));
        assert_eq!(scope("list_width"), Some(Scope::Local));
        assert_eq!(scope("splits"), None, "the old single-list key is gone");
    }

    /// Settings keys found in the source: `KEY_*` constants, string literals passed to the
    /// settings functions, and `format!("prefix:{…}")` keys built for them, outside tests.
    /// Each must be classified above.
    #[test]
    fn every_settings_key_in_the_source_is_classified() {
        let constant = regex::Regex::new(r#"const KEY_[A-Z_]+: &str = "([^"]+)""#).unwrap();
        let literal = regex::Regex::new(r#"(?:get|set|delete)_setting\(\s*[&\w]+,\s*"([^"]+)""#).unwrap();
        let built = regex::Regex::new(r#"format!\("([a-z_]+:)\{"#).unwrap();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") || path.file_name().is_some_and(|n| n == "sync.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            // Test modules (at the end of each file) use made-up and retired keys on purpose;
            // set_setting's debug assertion still checks what they write.
            let lines: Vec<&str> = text.lines().take_while(|l| l.trim() != "#[cfg(test)]").collect();
            for (i, line) in lines.iter().enumerate() {
                for c in constant.captures_iter(line).chain(literal.captures_iter(line)) {
                    found.push((path.clone(), c[1].to_string(), false));
                }
                // A key builder: `let key = format!(...)` or the body of a `fn …_key(`.
                let keyish = line.contains("key") || i.checked_sub(1).is_some_and(|p| lines[p].contains("_key("));
                if keyish {
                    for c in built.captures_iter(line) {
                        found.push((path.clone(), c[1].to_string(), true));
                    }
                }
            }
        }
        assert!(found.iter().any(|(_, k, _)| k == "cache_days"), "the scan found nothing; did the source layout change?");
        assert!(found.iter().any(|(_, k, prefix)| *prefix && k == "gmail_history:"), "the scan missed the key builders");
        // `KEY_*` constants that aren't settings keys.
        const NOT_SETTINGS: &[&str] = &["token.key"]; // secure.rs: the Linux key file's name
        for (path, key, prefix) in found.into_iter().filter(|(_, k, _)| !NOT_SETTINGS.contains(&k.as_str())) {
            let ok = if prefix {
                SYNCED_PREFIXES.contains(&key.as_str()) || LOCAL_PREFIXES.contains(&key.as_str())
            } else {
                scope(&key).is_some()
            };
            assert!(ok, "setting {key:?} in {} isn't classified in sync.rs", path.display());
        }
    }
}
