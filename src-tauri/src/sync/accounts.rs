//! The synced list of accounts (docs/SYNC.md, stage 4): one setting per account,
//! `account:<provider>:<lowercase address>` = `{app, name}`, written when it signs in. Another
//! PC that doesn't have it offers to sign in to it. Never tokens: each PC signs in itself.
//!
//! Signing an account out asks (when sync is on) whether that's for this PC only, which hides
//! the offer here (`account_dismissed:<provider>:<address>`, local), or everywhere, which
//! removes the setting, and with it the account on every PC that syncs.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::db;
use crate::error::Result;
use crate::items;

pub const PREFIX: &str = "account:";

fn key(provider: &str, email: &str) -> String {
    format!("account:{provider}:{}", email.to_lowercase())
}

fn dismissed_key(provider: &str, email: &str) -> String {
    format!("account_dismissed:{provider}:{}", email.to_lowercase())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Stored {
    /// The OAuth app it signs in with: "builtin" or "custom".
    app: String,
    #[serde(default)]
    name: Option<String>,
}

/// An account signed in here: it joins the synced list (and is no longer hidden here).
pub fn remember(c: &Connection, provider: &str, email: &str, app: &str, name: Option<&str>) -> Result<()> {
    db::set_setting(c, &key(provider, email), &serde_json::to_string(&Stored { app: app.into(), name: name.map(String::from) })?)?;
    db::delete_setting(c, &dismissed_key(provider, email))
}

/// Signed out on this PC only: the other PCs keep it, and this one stops offering it.
pub fn forget_here(c: &Connection, provider: &str, email: &str) -> Result<()> {
    db::set_setting(c, &dismissed_key(provider, email), "1")
}

/// Signed out everywhere: off the synced list, so every PC that syncs signs it out too.
pub fn forget_everywhere(c: &Connection, provider: &str, email: &str) -> Result<()> {
    db::delete_setting(c, &key(provider, email))?;
    db::delete_setting(c, &dismissed_key(provider, email))
}

/// An account on the synced list that isn't on this PC.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Elsewhere {
    pub provider: String,
    pub email: String,
    pub oauth_app: String,
    pub display_name: Option<String>,
}

/// The synced accounts this PC could sign in to: not here already, not hidden here.
pub fn elsewhere(c: &Connection) -> Result<Vec<Elsewhere>> {
    let here: Vec<(String, String)> = db::list_accounts(c)?.into_iter().map(|a| (a.provider, a.email.to_lowercase())).collect();
    let mut out = Vec::new();
    for (id, stored) in items::load::<Stored>(c, PREFIX)? {
        let Some((provider, email)) = id.split_once(':') else { continue };
        if here.iter().any(|(p, e)| p == provider && e == email) || db::get_setting(c, &dismissed_key(provider, email))?.is_some() {
            continue;
        }
        out.push(Elsewhere { provider: provider.into(), email: email.into(), oauth_app: stored.app, display_name: stored.name });
    }
    Ok(out)
}

/// The accounts on this PC that another PC signed out everywhere: of the settings another
/// device just changed (`changed`), the account ones that are now gone.
pub fn removed_elsewhere(c: &Connection, changed: &[String]) -> Result<Vec<i64>> {
    let mut ids = Vec::new();
    for k in changed.iter().filter(|k| k.starts_with(PREFIX)) {
        if db::get_setting(c, k)?.is_some() {
            continue;
        }
        let Some((provider, email)) = k[PREFIX.len()..].split_once(':') else { continue };
        ids.extend(db::list_accounts(c)?.into_iter().filter(|a| a.provider == provider && a.email.eq_ignore_ascii_case(email)).map(|a| a.id));
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_db(f: impl FnOnce(&mut Connection) -> Result<()>) {
        db::Db::open_in_memory().unwrap().with(f).unwrap();
    }

    #[test]
    fn accounts_signed_in_elsewhere_are_offered_until_signed_in_or_hidden() {
        with_db(|c| {
            // Arrived from another PC.
            remember(c, "microsoft", "Boris@Work.example", "builtin", Some("Boris"))?;
            remember(c, "google", "sam@gmail.com", "custom", None)?;
            let offered = elsewhere(c)?;
            assert_eq!(offered.len(), 2);
            assert_eq!(offered.iter().find(|e| e.provider == "microsoft").unwrap(), &Elsewhere {
                provider: "microsoft".into(),
                email: "boris@work.example".into(),
                oauth_app: "builtin".into(),
                display_name: Some("Boris".into())
            });
            // Signed in here: no longer offered.
            db::upsert_account(c, "microsoft", "boris@work.example", None, b"t")?;
            assert_eq!(elsewhere(c)?.iter().map(|e| e.email.as_str()).collect::<Vec<_>>(), ["sam@gmail.com"]);
            // Hidden here.
            forget_here(c, "google", "sam@gmail.com")?;
            assert!(elsewhere(c)?.is_empty());
            // Signing in again un-hides (and the setting is unchanged, so no new stamp needed).
            remember(c, "google", "sam@gmail.com", "custom", None)?;
            assert_eq!(db::get_setting(c, &dismissed_key("google", "sam@gmail.com"))?, None);
            Ok(())
        });
    }

    #[test]
    fn removal_everywhere_finds_the_local_account() {
        with_db(|c| {
            let a = db::upsert_account(c, "microsoft", "Boris@work.example", None, b"t")?.id;
            remember(c, "microsoft", "boris@work.example", "builtin", None)?;
            let k = key("microsoft", "boris@work.example");
            // Still on the list: nothing to remove.
            assert!(removed_elsewhere(c, std::slice::from_ref(&k))?.is_empty());
            forget_everywhere(c, "microsoft", "boris@work.example")?;
            assert_eq!(removed_elsewhere(c, &[k.clone(), "snippet:x".into()])?, [a]);
            // The removal goes up as a tombstone.
            assert_eq!(crate::sync::store::dirty(c, 100)?.iter().find(|d| d.key == k).map(|d| d.value.clone()), Some(None));
            Ok(())
        });
    }
}
