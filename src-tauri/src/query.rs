//! One small query language for search and split inboxes.
//!
//!   from:contoso.com  to:sam  subject:invoice  is:unread  is:read  is:flagged
//!   is:focused  is:other  is:invite  has:attachment  "exact phrase"  free words
//!
//! Prefix any term with `-` to negate it. Terms are ANDed. Free words hit the FTS
//! index (subject, sender, recipients, preview, and bodies once fetched) as prefixes;
//! "quoted text" matches as an exact phrase.

use rusqlite::types::Value as SqlValue;

#[derive(Debug, Clone, PartialEq)]
enum Term {
    From(String),
    To(String),
    Subject(String),
    Is(String),
    HasAttachment,
    /// account:work — mail of accounts whose address contains this.
    Account(String),
    Text(String),
    Phrase(String),
}

#[derive(Debug, Clone, PartialEq)]
struct Clause {
    negated: bool,
    term: Term,
}

/// A compiled filter over the `messages` table aliased as `m`.
#[derive(Debug, Default)]
pub struct Filter {
    pub sql: String,
    pub params: Vec<SqlValue>,
}

/// Adds "not these accounts" to a filter (accounts hidden in the UI).
pub fn exclude_accounts(f: Filter, ids: &[i64]) -> Filter {
    if ids.is_empty() {
        return f;
    }
    let marks = vec!["?"; ids.len()].join(", ");
    let mut params = f.params;
    params.extend(ids.iter().map(|&i| SqlValue::Integer(i)));
    Filter { sql: format!("({}) AND m.account_id NOT IN ({marks})", f.sql), params }
}

fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in input.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                cur.push(ch);
            }
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

fn unquote(s: &str) -> String {
    s.trim_matches('"').to_string()
}

fn parse(input: &str) -> Vec<Clause> {
    tokenize(input)
        .into_iter()
        .filter_map(|tok| {
            let (negated, body) = match tok.strip_prefix('-') {
                Some(rest) if !rest.is_empty() => (true, rest.to_string()),
                _ => (false, tok.clone()),
            };
            let term = match body.split_once(':') {
                Some((k, v)) if !v.is_empty() => match k.to_ascii_lowercase().as_str() {
                    "from" => Term::From(unquote(v)),
                    "to" | "cc" => Term::To(unquote(v)),
                    "subject" => Term::Subject(unquote(v)),
                    "is" => Term::Is(v.to_ascii_lowercase()),
                    "has" if v.starts_with("attach") => Term::HasAttachment,
                    "account" | "in" => Term::Account(unquote(v)),
                    _ => Term::Text(unquote(&body)),
                },
                _ if body.starts_with('"') => Term::Phrase(unquote(&body)),
                _ => Term::Text(unquote(&body)),
            };
            match &term {
                Term::Text(t) | Term::Phrase(t) if t.trim().is_empty() => None,
                _ => Some(Clause { negated, term }),
            }
        })
        .collect()
}

/// FTS5 MATCH expression for free text: each word becomes a quoted prefix query.
fn fts_match(text: &str) -> String {
    text.split_whitespace()
        .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn like(v: &str) -> SqlValue {
    SqlValue::Text(format!("%{}%", v.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")))
}

/// How many free-text terms and how many operators (`from:`, `is:`…) a query has: its
/// shape for telemetry, without any of its words.
pub fn shape(input: &str) -> (usize, usize) {
    let clauses = parse(input);
    let text = clauses.iter().filter(|c| matches!(c.term, Term::Text(_) | Term::Phrase(_))).count();
    (text, clauses.len() - text)
}

/// Compiles a query into a SQL condition (always valid, `1` when empty).
pub fn compile(input: &str) -> Filter {
    compile_clauses(parse(input))
}

fn compile_clauses(clauses: Vec<Clause>) -> Filter {
    let mut parts = Vec::new();
    let mut params = Vec::new();
    for Clause { negated, term } in clauses {
        let cond = match term {
            Term::Account(v) => {
                params.push(like(&v));
                "m.account_id IN (SELECT id FROM accounts WHERE email LIKE ? ESCAPE '\\')".to_string()
            }
            Term::From(v) => {
                params.extend([like(&v), like(&v)]);
                "(m.from_address LIKE ? ESCAPE '\\' OR m.from_name LIKE ? ESCAPE '\\')".to_string()
            }
            Term::To(v) => {
                params.extend([like(&v), like(&v)]);
                "(m.to_json LIKE ? ESCAPE '\\' OR m.cc_json LIKE ? ESCAPE '\\')".to_string()
            }
            Term::Subject(v) => {
                params.push(like(&v));
                "m.subject LIKE ? ESCAPE '\\'".to_string()
            }
            Term::Is(v) => match v.as_str() {
                "unread" => "m.is_read = 0".into(),
                "read" => "m.is_read = 1".into(),
                "flagged" | "starred" => "m.is_flagged = 1".into(),
                "focused" | "important" => "COALESCE(m.inference, 'focused') = 'focused'".into(),
                "other" => "m.inference = 'other'".into(),
                "invite" | "calendar" => "m.message_type = 'eventMessage'".into(),
                _ => "0".into(), // unknown is: matches nothing, so typos are visible
            },
            Term::HasAttachment => "m.has_attachments = 1".into(),
            Term::Text(t) => {
                params.push(SqlValue::Text(fts_match(&t)));
                "m.id IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)".into()
            }
            Term::Phrase(t) => {
                params.push(SqlValue::Text(format!("\"{}\"", t.replace('"', "\"\""))));
                "m.id IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)".into()
            }
        };
        parts.push(if negated { format!("NOT ({cond})") } else { cond });
    }
    Filter {
        sql: if parts.is_empty() { "1".into() } else { parts.join(" AND ") },
        params,
    }
}

// ---------- server search ----------
//
// The same query, sent to the server for mail outside the cache window: Graph `$search`
// (KQL) and Gmail's `q`. Terms a server can't search on (`is:focused`, `account:`...) are
// left out and checked locally instead (`with_server_hits`).

/// A value safe inside a server query: no quotes or backslashes, and quoted (taken
/// literally) unless it's a plain word or address. Quoting keeps search syntax in what was
/// typed (`re:budget`, `(draft`, an uppercase `OR`/`NOT`) from acting as operators.
fn server_value(v: &str) -> Option<String> {
    let v: String = v.chars().filter(|c| *c != '"' && *c != '\\').collect();
    let v = v.trim();
    let plain = v.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '@' | '_' | '-' | '\''))
        && !v.starts_with('-')
        && !matches!(v, "AND" | "OR" | "NOT" | "NEAR" | "ONEAR" | "AROUND");
    if v.is_empty() {
        None
    } else if plain {
        Some(v.to_string())
    } else {
        Some(format!("\"{v}\""))
    }
}

/// A phrase, always quoted.
fn server_phrase(t: &str) -> Option<String> {
    server_value(t).map(|v| if v.starts_with('"') { v } else { format!("\"{v}\"") })
}

/// The query as Graph `$search` text (KQL), or None when nothing in it is searchable there
/// (KQL can't start with NOT, so at least one term must be positive).
pub fn graph_search(input: &str) -> Option<String> {
    let mut parts = Vec::new();
    let mut positive = false;
    for Clause { negated, term } in parse(input) {
        let kql = match term {
            Term::From(v) => server_value(&v).map(|v| format!("from:{v}")),
            Term::To(v) => server_value(&v).map(|v| format!("recipients:{v}")),
            Term::Subject(v) => server_value(&v).map(|v| format!("subject:{v}")),
            Term::HasAttachment => Some("hasAttachments:true".into()),
            Term::Text(t) => server_value(&t),
            Term::Phrase(t) => server_phrase(&t),
            Term::Is(_) | Term::Account(_) => None,
        };
        let Some(kql) = kql else { continue };
        positive |= !negated;
        parts.push(if negated { format!("NOT {kql}") } else { kql });
    }
    // Positive terms first: "NOT x AND y" isn't valid KQL, "y AND NOT x" is.
    parts.sort_by_key(|p| p.starts_with("NOT "));
    positive.then(|| parts.join(" AND "))
}

/// The value of Graph's `$search` parameter: the KQL in double quotes, its own quotes escaped.
pub fn graph_search_param(kql: &str) -> String {
    format!("\"{}\"", kql.replace('"', "\\\""))
}

/// The query as Gmail search (`q`), or None when nothing in it is searchable there.
pub fn gmail_search(input: &str) -> Option<String> {
    let mut parts = Vec::new();
    let mut positive = false;
    for Clause { negated, term } in parse(input) {
        let q = match term {
            Term::From(v) => server_value(&v).map(|v| format!("from:{v}")),
            Term::To(v) => server_value(&v).map(|v| format!("{{to:{v} cc:{v}}}")),
            Term::Subject(v) => server_value(&v).map(|v| format!("subject:{v}")),
            Term::HasAttachment => Some("has:attachment".into()),
            Term::Is(v) => match v.as_str() {
                "unread" => Some("is:unread".into()),
                "read" => Some("is:read".into()),
                "flagged" | "starred" => Some("is:starred".into()),
                _ => None,
            },
            Term::Text(t) => server_value(&t),
            Term::Phrase(t) => server_phrase(&t),
            Term::Account(_) => None,
        };
        let Some(q) = q else { continue };
        positive |= !negated;
        parts.push(if negated { format!("-{q}") } else { q });
    }
    positive.then(|| parts.join(" "))
}

/// Whether the query's `account:` terms let it search this account.
pub fn account_matches(input: &str, email: &str) -> bool {
    let email = email.to_lowercase();
    parse(input).into_iter().all(|Clause { negated, term }| match term {
        Term::Account(v) => email.contains(&v.to_lowercase()) != negated,
        _ => true,
    })
}

/// Adds the server search's results to a search filter: messages the server matched that
/// also pass the terms only checked here (`is:`, `account:`). Free text isn't checked again:
/// the server matched it, often in a body that isn't cached.
pub fn with_server_hits(local: Filter, input: &str) -> Filter {
    let local_only: Vec<Clause> = parse(input).into_iter().filter(|c| matches!(c.term, Term::Is(_) | Term::Account(_))).collect();
    let checks = if local_only.is_empty() { None } else { Some(compile_clauses(local_only)) };
    let mut params = local.params;
    let extra = match checks {
        Some(f) => {
            params.extend(f.params);
            format!(" AND {}", f.sql)
        }
        None => String::new(),
    };
    Filter {
        sql: format!("({}) OR (m.id IN (SELECT message_id FROM search_cache WHERE hit = 1){extra})", local.sql),
        params,
    }
}

/// Filter for one tab of the split inbox, so every message lands in exactly one tab.
///
/// Tab 0 is the built-in Inbox: mail no split claims. Tab `i` (1-based) is split `i - 1`'s
/// matches that no earlier split already claimed, so order decides ties. Splits with an
/// empty query claim nothing (rather than everything).
pub fn split_filter(queries: &[String], tab: usize) -> Filter {
    let compiled: Vec<Option<Filter>> =
        queries.iter().map(|q| (!q.trim().is_empty()).then(|| compile(q))).collect();
    let (own, earlier): (Option<&Filter>, &[Option<Filter>]) = match tab {
        0 => (None, &compiled[..]),
        i if i <= compiled.len() => match &compiled[i - 1] {
            Some(f) => (Some(f), &compiled[..i - 1]),
            None => return Filter { sql: "0".into(), params: vec![] },
        },
        _ => return Filter { sql: "0".into(), params: vec![] },
    };
    let mut parts = Vec::new();
    let mut params = Vec::new();
    if let Some(f) = own {
        parts.push(format!("({})", f.sql));
        params.extend(f.params.iter().cloned());
    }
    for f in earlier.iter().flatten() {
        parts.push(format!("NOT ({})", f.sql));
        params.extend(f.params.iter().cloned());
    }
    Filter { sql: if parts.is_empty() { "1".into() } else { parts.join(" AND ") }, params }
}

#[cfg(test)]
mod tests {

    #[test]
    fn shape_counts_terms_and_operators() {
        assert_eq!(super::shape(r#"from:sam is:unread budget "q4 plan" -has:attachment"#), (2, 3));
        assert_eq!(super::shape(""), (0, 0));
    }
    use super::*;

    #[test]
    fn parses_keys_negation_and_phrases() {
        let c = parse(r#"from:contoso.com -is:read "q4 plan" budget has:attachments"#);
        assert_eq!(
            c,
            vec![
                Clause { negated: false, term: Term::From("contoso.com".into()) },
                Clause { negated: true, term: Term::Is("read".into()) },
                Clause { negated: false, term: Term::Phrase("q4 plan".into()) },
                Clause { negated: false, term: Term::Text("budget".into()) },
                Clause { negated: false, term: Term::HasAttachment },
            ]
        );
    }

    #[test]
    fn unknown_keys_are_text() {
        assert_eq!(parse("http://x.com")[0].term, Term::Text("http://x.com".into()));
        assert_eq!(parse("re:")[0].term, Term::Text("re:".into()));
    }

    #[test]
    fn excluding_accounts_appends_a_parameterised_clause() {
        let f = exclude_accounts(compile("from:sam"), &[2, 5]);
        assert!(f.sql.ends_with("AND m.account_id NOT IN (?, ?)"), "{}", f.sql);
        assert_eq!(f.params[f.params.len() - 2..], [SqlValue::Integer(2), SqlValue::Integer(5)]);
        assert_eq!(exclude_accounts(compile(""), &[]).sql, compile("").sql);
    }

    #[test]
    fn compiles_to_parameterised_sql() {
        let f = compile("from:sam is:unread hello");
        assert_eq!(f.params.len(), 3);
        assert!(f.sql.contains("m.is_read = 0"));
        assert!(f.sql.contains("MATCH ?"));
        assert_eq!(compile("").sql, "1");
        assert_eq!(fts_match(r#"say "hi"#), r#""say"* """hi"*"#);
        let p = compile(r#""quick question""#);
        assert_eq!(p.params.len(), 1);
        assert_eq!(p.params, vec![SqlValue::Text(r#""quick question""#.into())]);
    }

    #[test]
    fn graph_search_is_kql_with_local_only_terms_left_out() {
        assert_eq!(
            graph_search(r#"from:sam budget "q4 plan" -subject:draft is:unread account:work has:attachment"#).as_deref(),
            Some(r#"from:sam AND budget AND "q4 plan" AND hasAttachments:true AND NOT subject:draft"#)
        );
        assert_eq!(graph_search("to:kim").as_deref(), Some("recipients:kim"));
        assert_eq!(graph_search(r#"from:"Sam Carter""#).as_deref(), Some(r#"from:"Sam Carter""#));
        // Nothing positive for the server to search on.
        assert_eq!(graph_search("is:unread account:work"), None);
        assert_eq!(graph_search("-from:sam"), None);
        assert_eq!(graph_search(""), None);
        // Quotes and backslashes can't break out of the value.
        assert_eq!(graph_search(r"from:a\b").as_deref(), Some("from:ab"));
        assert_eq!(graph_search_param(r#"subject:"q4 plan""#), r#""subject:\"q4 plan\"""#);
        // Search syntax in what was typed is taken literally, not as operators.
        assert_eq!(graph_search("to do NOT forget").as_deref(), Some(r#"to AND do AND "NOT" AND forget"#));
        assert_eq!(graph_search("re:budget (draft").as_deref(), Some(r#""re:budget" AND "(draft""#));
        assert_eq!(graph_search("sam@contoso.com o'brien").as_deref(), Some("sam@contoso.com AND o'brien"));
    }

    #[test]
    fn gmail_search_maps_terms_to_gmail_operators() {
        assert_eq!(
            gmail_search(r#"from:sam budget "q4 plan" -to:kim is:unread is:flagged is:focused account:me"#).as_deref(),
            Some(r#"from:sam budget "q4 plan" -{to:kim cc:kim} is:unread is:starred"#)
        );
        assert_eq!(gmail_search("cats OR dogs {x}").as_deref(), Some(r#"cats "OR" dogs "{x}""#));
        assert_eq!(gmail_search("is:focused"), None);
        assert_eq!(gmail_search("-from:sam"), None);
    }

    #[test]
    fn account_terms_pick_the_accounts_to_search() {
        assert!(account_matches("budget", "me@work.com"));
        assert!(account_matches("account:WORK budget", "me@work.com"));
        assert!(!account_matches("account:gmail", "me@work.com"));
        assert!(!account_matches("-account:work", "me@work.com"));
    }

    #[test]
    fn server_hits_join_local_results_with_local_only_checks() {
        let f = with_server_hits(compile("budget is:unread"), "budget is:unread");
        assert!(f.sql.starts_with("(m.id IN (SELECT rowid FROM messages_fts"), "{}", f.sql);
        assert!(f.sql.ends_with("OR (m.id IN (SELECT message_id FROM search_cache WHERE hit = 1) AND m.is_read = 0)"), "{}", f.sql);
        assert_eq!(f.params.len(), 1, "is: has no parameter");
        let a = with_server_hits(compile("budget account:work"), "budget account:work");
        assert_eq!(a.params.len(), 3, "the account check's parameter follows the local filter's");
        assert!(with_server_hits(compile("budget"), "budget").sql.ends_with("WHERE hit = 1))"));
    }

    #[test]
    fn split_tabs_are_exclusive_and_inbox_catches_the_rest() {
        let q = vec!["from:github.com".to_string(), "is:other".to_string(), "  ".to_string()];
        let inbox = split_filter(&q, 0);
        assert_eq!(inbox.sql, "NOT ((m.from_address LIKE ? ESCAPE '\\' OR m.from_name LIKE ? ESCAPE '\\')) AND NOT (m.inference = 'other')");
        assert_eq!(inbox.params.len(), 2);
        let github = split_filter(&q, 1);
        assert!(github.sql.starts_with("((m.from_address"));
        assert!(!github.sql.contains("NOT"));
        let other = split_filter(&q, 2);
        assert!(other.sql.starts_with("(m.inference = 'other') AND NOT ("));
        assert_eq!(other.params.len(), 2, "params follow the SQL order");
        assert_eq!(split_filter(&q, 3).sql, "0", "an empty query claims nothing");
        assert_eq!(split_filter(&[], 0).sql, "1", "no splits: Inbox is everything");
    }
}
