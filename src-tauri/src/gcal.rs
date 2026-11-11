//! Google Calendar (primary calendar), mapped onto the same `events` table as Graph.
//!
//! Sync: the first run lists the rolling window with singleEvents=true (recurring series
//! expanded into instances) and keeps the nextSyncToken; later runs send only that token
//! and get what changed (deleted instances come back with status "cancelled"). A 410 means
//! the token expired: full resync. RSVP patches your attendee entry (sendUpdates=all, so
//! the organiser hears back); quick add creates an event. Invite cards for Gmail come from
//! the invitation's text/calendar part: its UID is looked up with iCalUID.

use std::collections::HashSet;

use reqwest::Method;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use crate::context::Core;

use crate::calendar::{self, EventUpsert, Invite};
use crate::error::{AppError, Result};
use crate::outbox::{NewEvent, Op, Rsvp};
use crate::state::AppState;

const DEFAULT_BASE: &str = "https://www.googleapis.com/calendar/v3";
static BASE: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| crate::testenv::get("PEETMAIL_GCAL_URL").unwrap_or_else(|| DEFAULT_BASE.to_string()));

fn events_url() -> String {
    format!("{}/calendars/primary/events", *BASE)
}

/// Calendar call. A Google account that signed in before calendar access was requested
/// gets "insufficient authentication scopes": say what to do about it.
async fn call(state: &AppState, account_id: i64, method: Method, url: &str, body: Option<&Value>) -> Result<Value> {
    match crate::gmail::call(state, account_id, method, url, body).await {
        Err(AppError::Graph { status: 403, message }) if message.to_ascii_lowercase().contains("insufficient") => Err(AppError::Graph {
            status: 403,
            message: "Google Calendar needs permission: sign in to this Google account again (Settings → Accounts → Add Google account)".into(),
        }),
        other => other,
    }
}

// ---------- parsing ----------

/// RFC 3339 with any offset -> "YYYY-MM-DDTHH:MM:SSZ".
fn to_utc(s: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// Google's attendee responseStatus -> the Graph vocabulary the UI speaks.
fn response_word(s: &str) -> &'static str {
    match s {
        "accepted" => "accepted",
        "tentative" => "tentativelyAccepted",
        "declined" => "declined",
        _ => "notResponded",
    }
}

pub fn parse_event(v: &Value) -> Option<EventUpsert> {
    let s = |p: &str| v.pointer(p).and_then(Value::as_str).map(String::from);
    let id = s("/id")?;
    let (start_at, end_at, is_all_day) = match (s("/start/dateTime"), s("/start/date")) {
        (Some(st), _) => (to_utc(&st)?, to_utc(&s("/end/dateTime")?)?, false),
        (None, Some(d)) => (d, s("/end/date")?, true),
        _ => return None,
    };
    let attendees: Vec<&Value> = v["attendees"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let me = attendees.iter().find(|a| a["self"].as_bool() == Some(true));
    let is_organizer = v.pointer("/organizer/self").and_then(Value::as_bool).unwrap_or(false)
        || me.and_then(|a| a["organizer"].as_bool()).unwrap_or(false);
    let response = match me {
        Some(a) if !is_organizer => Some(response_word(a["responseStatus"].as_str().unwrap_or("needsAction")).to_string()),
        _ if is_organizer => Some("organizer".into()),
        _ => Some("none".into()),
    };
    let attendees_json: Vec<Value> = attendees
        .iter()
        .map(|a| {
            json!({
                "name": a["displayName"],
                "address": a["email"],
                "response": a["responseStatus"].as_str().map(response_word),
                "type": if a["optional"].as_bool() == Some(true) { "optional" } else { "required" },
            })
        })
        .collect();
    let join_url = s("/hangoutLink").or_else(|| {
        v.pointer("/conferenceData/entryPoints")
            .and_then(Value::as_array)
            .and_then(|eps| eps.iter().find(|e| e["entryPointType"] == "video"))
            .and_then(|e| e["uri"].as_str())
            .map(String::from)
    });
    let description = s("/description").unwrap_or_default();
    let preview: String = crate::sanitize::html_to_text(&crate::sanitize::clean_html(&description)).chars().take(255).collect();
    let recurring_master = v.get("recurrence").is_some_and(|r| r.is_array());
    Some(EventUpsert {
        remote_id: id,
        subject: s("/summary").unwrap_or_default(),
        start_at,
        end_at,
        is_all_day,
        location: s("/location").filter(|l| !l.trim().is_empty()),
        organizer_name: s("/organizer/displayName"),
        organizer_address: s("/organizer/email"),
        attendees_json: Value::Array(attendees_json).to_string(),
        response,
        is_organizer,
        is_cancelled: s("/status").as_deref() == Some("cancelled"),
        show_as: Some(if s("/transparency").as_deref() == Some("transparent") { "free" } else { "busy" }.into()),
        join_url,
        web_link: s("/htmlLink"),
        preview,
        event_type: if recurring_master {
            "seriesMaster".into()
        } else if v.get("recurringEventId").is_some() {
            "occurrence".into()
        } else {
            "singleInstance".into()
        },
        series_master_id: s("/recurringEventId"),
        has_details: true,
    })
}

// ---------- sync ----------

pub async fn sync_account(app: &Core, state: &AppState, account_id: i64) -> Result<()> {
    let stored = state.db.with(|c| calendar::sync_state(c, account_id))?;
    let mut sync = match stored {
        Some(s) if !calendar::window_is_stale(&s.window_start, &s.window_end) => s,
        _ => {
            let (window_start, window_end) = calendar::fresh_window();
            calendar::SyncState { delta_link: None, window_start, window_end }
        }
    };
    let mut restarted = false;
    'full: loop {
        let full = sync.delta_link.is_none();
        let mut seen: HashSet<String> = HashSet::new();
        let mut changed = false;
        let mut page_token: Option<String> = None;
        loop {
            let mut q: Vec<(&str, String)> = vec![("singleEvents", "true".into()), ("maxResults", "250".into())];
            match &sync.delta_link {
                Some(token) => q.push(("syncToken", token.clone())),
                None => {
                    q.push(("timeMin", sync.window_start.clone()));
                    q.push(("timeMax", sync.window_end.clone()));
                }
            }
            if let Some(p) = &page_token {
                q.push(("pageToken", p.clone()));
            }
            let url = url::Url::parse_with_params(&events_url(), &q).expect("valid url").to_string();
            let page = match call(state, account_id, Method::GET, &url, None).await {
                Ok(p) => p,
                Err(AppError::Graph { status: 410, .. }) if !restarted => {
                    log::warn!("google calendar sync token expired for account {account_id}, resyncing");
                    restarted = true;
                    sync.delta_link = None;
                    continue 'full;
                }
                Err(e) => return Err(e),
            };
            let items = page["items"].as_array().cloned().unwrap_or_default();
            if !items.is_empty() {
                changed = true;
                state.db.with(|c| {
                    let busy = crate::outbox::busy_remote_ids(c, account_id)?;
                    let tx = c.transaction()?;
                    for item in &items {
                        let Some(id) = item["id"].as_str() else { continue };
                        if busy.contains(id) {
                            seen.insert(id.to_string());
                            continue;
                        }
                        if item["status"] == "cancelled" {
                            calendar::delete_event(&tx, account_id, id)?;
                        } else if let Some(e) = parse_event(item) {
                            seen.insert(e.remote_id.clone());
                            calendar::upsert_event(&tx, account_id, &e)?;
                        }
                    }
                    tx.commit()?;
                    Ok(())
                })?;
            }
            match page["nextPageToken"].as_str() {
                Some(p) => page_token = Some(p.to_string()),
                None => {
                    // Without a token the next sync is a full one again.
                    sync.delta_link = page["nextSyncToken"].as_str().map(String::from);
                    break;
                }
            }
        }
        state.db.with(|c| {
            if full {
                changed |= calendar::sweep_unseen(c, account_id, &seen)?;
            }
            calendar::save_sync_state(c, account_id, &sync)
        })?;
        if changed {
            let _ = app.emit(calendar::EVENT_CALENDAR_CHANGED, json!({ "accountId": account_id }));
        }
        return Ok(());
    }
}

// ---------- writes (outbox) ----------

fn create_body(e: &NewEvent) -> Value {
    let (start, end) = if e.all_day {
        (json!({ "date": e.start }), json!({ "date": e.end }))
    } else {
        (json!({ "dateTime": e.start, "timeZone": "UTC" }), json!({ "dateTime": e.end, "timeZone": "UTC" }))
    };
    let mut body = json!({ "summary": e.subject, "start": start, "end": end, "transparency": if e.all_day { "transparent" } else { "opaque" } });
    if let Some(l) = &e.location {
        body["location"] = json!(l);
    }
    if let Value::Object(extra) = crate::event_edit::google_body(&e.details) {
        for (k, v) in extra {
            body[k] = v;
        }
    }
    body
}

/// One event, as Google has it now.
pub async fn get_event(state: &AppState, account_id: i64, id: &str) -> Result<Value> {
    call(state, account_id, Method::GET, &format!("{}/{id}", events_url()), None).await
}

fn google_status(r: Rsvp) -> &'static str {
    match r {
        Rsvp::Accept => "accepted",
        Rsvp::Tentative => "tentative",
        Rsvp::Decline => "declined",
    }
}

pub async fn execute(state: &AppState, account_id: i64, remote_id: Option<&str>, op: &Op) -> Result<Value> {
    match op {
        // conferenceDataVersion=1: without it Google ignores a Meet request.
        Op::CreateEvent(e) => call(state, account_id, Method::POST, &format!("{}?conferenceDataVersion=1", events_url()), Some(&create_body(e))).await,
        Op::Respond { response } => {
            let id = remote_id.ok_or_else(|| AppError::Other("RSVP has no event".into()))?;
            let url = format!("{}/{id}", events_url());
            let event = call(state, account_id, Method::GET, &url, None).await?;
            let mut attendees = event["attendees"].as_array().cloned().unwrap_or_default();
            let Some(me) = attendees.iter_mut().find(|a| a["self"].as_bool() == Some(true)) else {
                return Ok(Value::Null); // not an invitation (your own event): nothing to answer
            };
            me["responseStatus"] = json!(google_status(*response));
            // PATCH replaces the attendee list, so send it whole. sendUpdates tells the organiser.
            call(state, account_id, Method::PATCH, &format!("{url}?sendUpdates=all"), Some(&json!({ "attendees": attendees }))).await
        }
        Op::UpdateEvent { changes } => {
            let id = remote_id.ok_or_else(|| AppError::Other("edit has no event".into()))?;
            // Guests hear about it (Google sends nothing for events without any).
            let url = format!("{}/{id}?sendUpdates=all&conferenceDataVersion=1", events_url());
            call(state, account_id, Method::PATCH, &url, Some(&crate::event_edit::google_body(changes))).await
        }
        Op::DeleteEvent { cancel, .. } => {
            let id = remote_id.ok_or_else(|| AppError::Other("delete has no event".into()))?;
            let url = format!("{}/{id}?sendUpdates={}", events_url(), if *cancel { "all" } else { "none" });
            call(state, account_id, Method::DELETE, &url, None).await
        }
        _ => Err(AppError::Graph { status: 400, message: "not a calendar operation".into() }),
    }
}

// ---------- full details ----------

pub async fn event_detail(state: &AppState, account_id: i64, remote_id: &str) -> Result<calendar::EventDetail> {
    let url = |id: &str| format!("{}/{id}", events_url());
    let v = call(state, account_id, Method::GET, &url(remote_id), None).await?;
    let master_id = v["recurringEventId"].as_str().map(String::from);
    let lines = |ev: &Value| ev["recurrence"].as_array().map(|a| a.iter().filter_map(|l| l.as_str().map(String::from)).collect::<Vec<_>>());
    let start_date = |ev: &Value| {
        ev.pointer("/start/date")
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| ev.pointer("/start/dateTime").and_then(Value::as_str).and_then(|d| d.get(..10)).map(String::from))
            .unwrap_or_default()
    };
    let recurrence = match (&master_id, lines(&v)) {
        (_, Some(l)) => crate::recurrence::from_rrule(&l, &start_date(&v)),
        (Some(m), None) => match call(state, account_id, Method::GET, &url(m), None).await {
            Ok(master) => lines(&master).and_then(|l| crate::recurrence::from_rrule(&l, &start_date(&master))),
            Err(e) => {
                log::info!("series master {m}: {e}");
                None
            }
        },
        _ => None,
    };
    // An instance that no longer sits where the series puts it has been changed on its own.
    let moved = master_id.is_some()
        && v.pointer("/originalStartTime/dateTime").and_then(Value::as_str).and_then(to_utc)
            != v.pointer("/start/dateTime").and_then(Value::as_str).and_then(to_utc);
    let reminders = &v["reminders"];
    let use_default = reminders["useDefault"].as_bool().unwrap_or(true);
    let description = v["description"].as_str().unwrap_or_default();
    Ok(calendar::EventDetail {
        // Google descriptions are HTML or plain text with newlines.
        body_html: if description.contains('<') { crate::sanitize::clean_html(description) } else { crate::sanitize::text_to_html(description) },
        recurrence,
        series_master_id: master_id.clone(),
        is_occurrence: master_id.is_some(),
        is_exception: moved,
        reminder_minutes: if use_default {
            None
        } else {
            reminders["overrides"].as_array().into_iter().flatten().filter_map(|o| o["minutes"].as_i64()).min()
        },
        reminder_default: use_default,
        sensitivity: match v["visibility"].as_str() {
            Some("private") => "private",
            Some("confidential") => "confidential",
            _ => "normal",
        }
        .to_string(),
        categories: vec![],
        importance: "normal".into(),
        join_url: None,
        join_provider: None,
    })
}

/// Google Meet, if the primary calendar allows it (`conferenceProperties`): some Workspace
/// domains turn conferencing off, and asking for it there fails the whole save.
pub async fn meeting_solution(state: &AppState, account_id: i64) -> Result<Option<String>> {
    let v = call(state, account_id, Method::GET, &format!("{}/calendars/primary?fields=conferenceProperties", *BASE), None).await?;
    let allowed = v.pointer("/conferenceProperties/allowedConferenceSolutionTypes").and_then(Value::as_array);
    Ok(allowed.is_some_and(|a| a.iter().any(|t| t == "hangoutsMeet")).then(|| "hangoutsMeet".to_string()))
}

// ---------- invites ----------

/// What an invitation's text/calendar part says.
#[derive(Debug, Clone, PartialEq)]
pub struct Ics {
    pub uid: String,
    /// REQUEST | CANCEL | REPLY | ...
    pub method: String,
    /// For REPLY: the attendee's answer (ACCEPTED / DECLINED / TENTATIVE).
    pub partstat: Option<String>,
    /// For one occurrence of a series: its RECURRENCE-ID as `params:value`
    /// (`TZID=Europe/London:20261110T103000`, `:20261110T103000Z`, `VALUE=DATE:20261110`).
    pub recurrence_id: Option<String>,
}

pub fn parse_ics(text: &str) -> Option<Ics> {
    let unfolded = text.replace("\r\n ", "").replace("\r\n\t", "").replace("\n ", "").replace("\n\t", "");
    let mut method = String::from("PUBLISH");
    let (mut uid, mut partstat, mut recurrence_id, mut in_event) = (None, None, None, false);
    for line in unfolded.lines() {
        let line = line.trim_end_matches('\r');
        let (key, value) = line.split_once(':').unwrap_or((line, ""));
        let name = key.split(';').next().unwrap_or("").to_ascii_uppercase();
        match name.as_str() {
            "METHOD" if !in_event => method = value.trim().to_ascii_uppercase(),
            "BEGIN" if value.eq_ignore_ascii_case("VEVENT") => in_event = true,
            "END" if value.eq_ignore_ascii_case("VEVENT") => break,
            "UID" if in_event && uid.is_none() => uid = Some(value.trim().to_string()),
            "RECURRENCE-ID" if in_event => {
                recurrence_id = Some(format!("{}:{}", key.split_once(';').map_or("", |(_, p)| p), value.trim()));
            }
            "ATTENDEE" if in_event && partstat.is_none() => {
                partstat = key.split(';').find_map(|p| p.to_ascii_uppercase().strip_prefix("PARTSTAT=").map(String::from));
            }
            _ => {}
        }
    }
    Some(Ics { uid: uid.filter(|u| !u.is_empty())?, method, partstat, recurrence_id })
}

fn meeting_type(ics: &Ics) -> &'static str {
    match (ics.method.as_str(), ics.partstat.as_deref()) {
        ("CANCEL", _) => "meetingCancelled",
        ("REPLY", Some("DECLINED")) => "meetingDeclined",
        ("REPLY", Some("TENTATIVE")) => "meetingTentativelyAccepted",
        ("REPLY", _) => "meetingAccepted",
        _ => "meetingRequest",
    }
}

/// Remembers what a message's calendar part said (None: it has none), so opening it again
/// doesn't refetch the message.
pub fn record_invite(conn: &Connection, message_id: i64, ics: Option<&Ics>) -> Result<()> {
    conn.execute(
        "INSERT INTO message_invites (message_id, uid, method, partstat, recurrence_id) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (message_id) DO UPDATE SET uid = excluded.uid, method = excluded.method, partstat = excluded.partstat,
           recurrence_id = excluded.recurrence_id",
        params![message_id, ics.map(|i| &i.uid), ics.map(|i| &i.method), ics.and_then(|i| i.partstat.as_ref()), ics.and_then(|i| i.recurrence_id.as_ref())],
    )?;
    Ok(())
}

/// Some(None): checked, no invitation. None: not checked yet.
fn recorded_invite(conn: &Connection, message_id: i64) -> Result<Option<Option<Ics>>> {
    Ok(conn
        .query_row("SELECT uid, method, partstat, recurrence_id FROM message_invites WHERE message_id = ?1", [message_id], |r| {
            Ok(r.get::<_, Option<String>>(0)?.map(|uid| Ics {
                uid,
                method: r.get::<_, Option<String>>(1).ok().flatten().unwrap_or_default(),
                partstat: r.get(2).ok().flatten(),
                recurrence_id: r.get(3).ok().flatten(),
            }))
        })
        .optional()?)
}

/// Which of the events sharing an invitation's UID it's about. An invitation to one occurrence
/// (RECURRENCE-ID) is about that occurrence, never the series: with no match there's no card,
/// rather than one that answers every meeting. Otherwise the series (or the one event).
fn pick_invite_item<'a>(items: &'a [Value], recurrence_id: Option<&str>) -> Option<&'a Value> {
    let Some(rid) = recurrence_id else {
        return items.iter().find(|i| i.get("recurringEventId").is_none()).or_else(|| items.first());
    };
    let (params, value) = rid.split_once(':').unwrap_or(("", rid));
    let value = value.trim();
    items.iter().filter(|i| i.get("recurringEventId").is_some()).find(|i| {
        if let Some(date) = i.pointer("/originalStartTime/date").and_then(Value::as_str) {
            return date.replace('-', "") == value.get(..8).unwrap_or(value);
        }
        let Some(dt) = i.pointer("/originalStartTime/dateTime").and_then(Value::as_str) else { return false };
        let Ok(start) = chrono::DateTime::parse_from_rfc3339(dt) else { return false };
        if let Some(utc) = value.strip_suffix('Z') {
            return start.with_timezone(&chrono::Utc).format("%Y%m%dT%H%M%S").to_string() == utc;
        }
        // A local time in the organiser's zone: Google gives the original start in that zone too.
        let zone = params.split(';').find_map(|p| p.strip_prefix("TZID="));
        let same_zone = zone.is_none() || zone == i.pointer("/originalStartTime/timeZone").and_then(Value::as_str);
        same_zone && start.format("%Y%m%dT%H%M%S").to_string() == value
    })
}

/// The calendar event behind a Gmail invitation, via the UID in its text/calendar part.
pub async fn invite_for_message(state: &AppState, message_id: i64, account_id: i64, remote_id: &str) -> Result<Option<Invite>> {
    let ics = match state.db.with(|c| recorded_invite(c, message_id))? {
        Some(known) => known,
        None => {
            let ics = crate::gmail::calendar_part(state, account_id, remote_id).await?.as_deref().and_then(parse_ics);
            state.db.with(|c| record_invite(c, message_id, ics.as_ref()))?;
            ics
        }
    };
    let Some(ics) = ics else { return Ok(None) };
    let url = url::Url::parse_with_params(&events_url(), &[("iCalUID", ics.uid.as_str())]).expect("valid url").to_string();
    let found = call(state, account_id, Method::GET, &url, None).await?;
    let Some(item) = found["items"].as_array().and_then(|items| pick_invite_item(items, ics.recurrence_id.as_deref())) else { return Ok(None) };
    let Some(parsed) = parse_event(item) else { return Ok(None) };
    // Google's start is the series' first day in its own zone (dateTime carries the offset).
    let first_day = item.pointer("/start/dateTime").or_else(|| item.pointer("/start/date")).and_then(Value::as_str).and_then(|s| s.get(..10));
    let lines: Option<Vec<String>> = item["recurrence"].as_array().map(|a| a.iter().filter_map(|l| l.as_str().map(String::from)).collect());
    let recurrence = lines.zip(first_day).and_then(|(l, d)| crate::recurrence::from_rrule(&l, d));
    let (event, next) = state.db.with(|c| {
        let busy = crate::outbox::busy_remote_ids(c, account_id)?;
        if !busy.contains(&parsed.remote_id) || calendar::event_by_remote(c, account_id, &parsed.remote_id)?.is_none() {
            calendar::upsert_event(c, account_id, &parsed)?;
        }
        let event = calendar::event_by_remote(c, account_id, &parsed.remote_id)?.ok_or_else(|| AppError::Other("event vanished".into()))?;
        let next = calendar::next_for_invite(c, account_id, &event)?;
        Ok((event, next))
    })?;
    Ok(Some(Invite {
        meeting_message_type: Some(meeting_type(&ics).into()),
        meeting_request_type: None,
        previous_start: None,
        previous_end: None,
        previous_location: None,
        recurrence,
        event,
        next,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_invitations_series_and_all_day() {
        let invite = json!({
            "kind": "calendar#event", "id": "ev1", "status": "confirmed", "summary": "Team lunch", "location": "Café",
            "htmlLink": "https://calendar.google.com/event?eid=1", "hangoutLink": "https://meet.google.com/abc",
            "start": { "dateTime": "2026-10-28T12:00:00+11:00" }, "end": { "dateTime": "2026-10-28T13:00:00+11:00" },
            "organizer": { "email": "kim@x", "displayName": "Kim" },
            "attendees": [
                { "email": "kim@x", "organizer": true, "responseStatus": "accepted" },
                { "email": "me@gmail.com", "self": true, "responseStatus": "needsAction" }
            ],
            "description": "<b>Bring</b> ideas"
        });
        let e = parse_event(&invite).unwrap();
        assert_eq!((e.start_at.as_str(), e.end_at.as_str()), ("2026-10-28T01:00:00Z", "2026-10-28T02:00:00Z"));
        assert_eq!(e.response.as_deref(), Some("notResponded"));
        assert!(!e.is_organizer);
        assert_eq!(e.join_url.as_deref(), Some("https://meet.google.com/abc"));
        assert_eq!(e.preview, "Bring ideas");
        assert_eq!(e.show_as.as_deref(), Some("busy"));

        let mine = json!({ "id": "ev2", "summary": "Dentist", "start": { "date": "2026-11-02" }, "end": { "date": "2026-11-03" },
            "organizer": { "email": "me@gmail.com", "self": true }, "transparency": "transparent" });
        let e = parse_event(&mine).unwrap();
        assert!(e.is_all_day && e.is_organizer);
        assert_eq!((e.start_at.as_str(), e.response.as_deref(), e.show_as.as_deref()), ("2026-11-02", Some("organizer"), Some("free")));

        let instance = json!({ "id": "yoga_20261029T070000Z", "recurringEventId": "yoga", "summary": "Yoga",
            "start": { "dateTime": "2026-10-29T07:00:00Z" }, "end": { "dateTime": "2026-10-29T08:00:00Z" } });
        let e = parse_event(&instance).unwrap();
        assert_eq!((e.event_type.as_str(), e.series_master_id.as_deref()), ("occurrence", Some("yoga")));
        let master = json!({ "id": "yoga", "recurrence": ["RRULE:FREQ=WEEKLY"], "summary": "Yoga",
            "start": { "dateTime": "2026-10-01T07:00:00Z" }, "end": { "dateTime": "2026-10-01T08:00:00Z" } });
        assert_eq!(parse_event(&master).unwrap().event_type, "seriesMaster");
    }

    #[test]
    fn reads_the_uid_and_method_of_an_invitation() {
        let ics = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VTIMEZONE\r\nTZID:X\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:lunch-1\r\n 23@google.com\r\nATTENDEE;CN=Me;PARTSTAT=NEEDS-ACTION:mailto:me@gmail.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        assert_eq!(parse_ics(ics), Some(Ics { uid: "lunch-123@google.com".into(), method: "REQUEST".into(), partstat: Some("NEEDS-ACTION".into()), recurrence_id: None }));
        let i = parse_ics(&ics.replace("METHOD:REQUEST", "METHOD:CANCEL")).unwrap();
        assert_eq!(meeting_type(&i), "meetingCancelled");
        let r = parse_ics(&ics.replace("METHOD:REQUEST", "METHOD:REPLY").replace("NEEDS-ACTION", "DECLINED")).unwrap();
        assert_eq!(meeting_type(&r), "meetingDeclined");
        assert_eq!(parse_ics("BEGIN:VCALENDAR\r\nEND:VCALENDAR"), None);
    }

    #[test]
    fn an_invitation_to_one_occurrence_finds_that_occurrence_never_the_series() {
        let ics = |rid: &str| format!("BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:standup@google.com\r\n{rid}END:VEVENT\r\nEND:VCALENDAR\r\n");
        assert_eq!(parse_ics(&ics("")).unwrap().recurrence_id, None);
        let rid = parse_ics(&ics("RECURRENCE-ID;TZID=Australia/Sydney:20261110T103000\r\n")).unwrap().recurrence_id;
        assert_eq!(rid.as_deref(), Some("TZID=Australia/Sydney:20261110T103000"));

        let master = json!({ "id": "standup", "recurrence": ["RRULE:FREQ=WEEKLY"] });
        let moved = json!({ "id": "standup_20261109T233000Z", "recurringEventId": "standup",
            "originalStartTime": { "dateTime": "2026-11-10T10:30:00+11:00", "timeZone": "Australia/Sydney" } });
        let items = [master.clone(), moved.clone()];
        assert_eq!(pick_invite_item(&items, None), Some(&master));
        assert_eq!(pick_invite_item(&items, rid.as_deref()), Some(&moved));
        assert_eq!(pick_invite_item(&items, Some(":20261109T233000Z")), Some(&moved));
        // Another occurrence, or one Google didn't list: no card, rather than answering the series.
        assert_eq!(pick_invite_item(&items, Some("TZID=Australia/Sydney:20261117T103000")), None);
        assert_eq!(pick_invite_item(&[master], rid.as_deref()), None);
        let day = json!({ "id": "leave_20261110", "recurringEventId": "leave", "originalStartTime": { "date": "2026-11-10" } });
        assert_eq!(pick_invite_item(std::slice::from_ref(&day), Some("VALUE=DATE:20261110")), Some(&day));
    }

    #[test]
    fn create_bodies() {
        let e = NewEvent { subject: "Lunch".into(), start: "2026-10-30T02:00:00Z".into(), end: "2026-10-30T02:30:00Z".into(), all_day: false, location: None, time_zone: "UTC".into(), details: Default::default() };
        assert_eq!(create_body(&e)["start"], json!({ "dateTime": "2026-10-30T02:00:00Z", "timeZone": "UTC" }));
        let d = NewEvent { start: "2026-11-06".into(), end: "2026-11-07".into(), all_day: true, location: Some("Home".into()), ..e };
        let b = create_body(&d);
        assert_eq!((b["start"].clone(), b["location"].clone(), b["transparency"].clone()), (json!({ "date": "2026-11-06" }), json!("Home"), json!("transparent")));
    }
}
