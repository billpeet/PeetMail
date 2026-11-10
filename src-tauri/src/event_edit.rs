//! Creating, editing and deleting events (the event editor). One `EventChanges` shape for
//! both services: each field is optional and only what changed is sent. It's translated
//! to a Graph PATCH / POST body or a Google Calendar one here, and applied to the local
//! row at once so the calendar shows the change before the server confirms it.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::error::{AppError, Result};
use crate::recurrence::{self, Recurrence};
use crate::state::AppState;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Times {
    /// Timed: UTC ISO instants. All-day: YYYY-MM-DD (end exclusive).
    pub start: String,
    pub end: String,
    pub all_day: bool,
    /// IANA zone the dates of all-day events (and a series' range) are in.
    pub time_zone: String,
    /// The same start and end as wall-clock times in `time_zone` (YYYY-MM-DDTHH:MM:SS), sent
    /// for a series: it repeats at that local time, across daylight-saving changes. (Pinned
    /// to UTC, a 9am meeting would move to 8 or 10 when the clocks change.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_end: Option<String>,
}

impl Times {
    /// The wall-clock pair, when given for a timed event.
    fn local(&self) -> Option<(&str, &str)> {
        match (&self.local_start, &self.local_end) {
            (Some(s), Some(e)) if !self.all_day => Some((s, e)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditAttendee {
    pub name: Option<String>,
    pub address: String,
    #[serde(default)]
    pub optional: bool,
    /// Their answer so far (kept when the list is sent back): Graph vocabulary.
    #[serde(default)]
    pub response: Option<String>,
    /// A room (or other resource) being booked, not a person.
    #[serde(default)]
    pub resource: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reminder {
    /// Minutes before the start; None turns the reminder off.
    pub minutes: Option<i64>,
}

/// What an edit changes (None: unchanged). Also the extra fields of a new event.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventChanges {
    pub subject: Option<String>,
    pub times: Option<Times>,
    /// "" clears it.
    pub location: Option<String>,
    /// The room's mailbox when the location is a room (sent with the location).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_email: Option<String>,
    pub description_html: Option<String>,
    pub attendees: Option<Vec<EditAttendee>>,
    /// free | tentative | busy | oof | workingElsewhere
    pub show_as: Option<String>,
    pub reminder: Option<Reminder>,
    /// normal | private
    pub sensitivity: Option<String>,
    /// Some(None) stops it repeating.
    #[serde(default, with = "double_option", skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<Option<Recurrence>>,
    /// Adds an online meeting from this provider (Graph's onlineMeetingProvider, e.g.
    /// teamsForBusiness; "hangoutsMeet" for Google). Once added it can't be taken off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online_meeting: Option<String>,
}

/// Distinguishes "recurrence": null (stop repeating) from a missing field (unchanged).
mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer, T: Serialize>(v: &Option<Option<T>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            None => s.serialize_none(),
            Some(inner) => inner.serialize(s),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
        Ok(Some(Option::<T>::deserialize(d)?))
    }
}

impl EventChanges {
    pub fn is_empty(&self) -> bool {
        *self == EventChanges::default()
    }
}

/// Everything the editor shows, loaded fresh from the server.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditableEvent {
    /// What saving changes: the occurrence, or the series master.
    pub remote_id: String,
    pub subject: String,
    pub times: Times,
    pub location: String,
    /// Set when the location is a room.
    pub location_email: Option<String>,
    pub description_html: String,
    pub attendees: Vec<EditAttendee>,
    pub show_as: String,
    /// None: no reminder (unless reminder_default).
    pub reminder_minutes: Option<i64>,
    pub reminder_default: bool,
    pub sensitivity: String,
    /// The series' pattern when editing the series.
    pub recurrence: Option<Recurrence>,
    /// It already has an online meeting (Teams, Meet...).
    pub online_meeting: bool,
    /// You organise it (or it's your own event): every field is yours to change.
    pub is_organizer: bool,
    /// Editing the whole series (not one occurrence).
    pub series: bool,
}

// ---------- Graph ----------

fn graph_times(t: &Times) -> (Value, Value) {
    if t.all_day {
        (
            json!({ "dateTime": format!("{}T00:00:00", t.start), "timeZone": t.time_zone }),
            json!({ "dateTime": format!("{}T00:00:00", t.end), "timeZone": t.time_zone }),
        )
    } else if let Some((s, e)) = t.local() {
        (json!({ "dateTime": s, "timeZone": t.time_zone }), json!({ "dateTime": e, "timeZone": t.time_zone }))
    } else {
        let bare = |s: &str| s.trim_end_matches('Z').to_string();
        (json!({ "dateTime": bare(&t.start), "timeZone": "UTC" }), json!({ "dateTime": bare(&t.end), "timeZone": "UTC" }))
    }
}

/// Graph fields for these changes (a PATCH body, or merged into a POST).
pub fn graph_body(c: &EventChanges, time_zone: &str) -> Value {
    let mut b = Map::new();
    if let Some(s) = &c.subject {
        b.insert("subject".into(), json!(s));
    }
    if let Some(t) = &c.times {
        let (start, end) = graph_times(t);
        b.insert("start".into(), start);
        b.insert("end".into(), end);
        b.insert("isAllDay".into(), json!(t.all_day));
    }
    if let Some(l) = &c.location {
        let loc = match c.location_email.as_deref().filter(|e| !e.is_empty()) {
            Some(email) => json!({ "displayName": l, "locationEmailAddress": email, "locationType": "conferenceRoom" }),
            None => json!({ "displayName": l }),
        };
        b.insert("location".into(), loc);
    }
    if let Some(h) = &c.description_html {
        b.insert("body".into(), json!({ "contentType": "HTML", "content": crate::sanitize::clean_html(h) }));
    }
    if let Some(list) = &c.attendees {
        let people: Vec<Value> = list
            .iter()
            .map(|a| {
                let kind = if a.resource { "resource" } else if a.optional { "optional" } else { "required" };
                json!({ "emailAddress": { "address": a.address, "name": a.name.clone().unwrap_or_else(|| a.address.clone()) }, "type": kind })
            })
            .collect();
        b.insert("attendees".into(), Value::Array(people));
    }
    if let Some(s) = &c.show_as {
        b.insert("showAs".into(), json!(s));
    }
    if let Some(r) = &c.reminder {
        b.insert("isReminderOn".into(), json!(r.minutes.is_some()));
        if let Some(m) = r.minutes {
            b.insert("reminderMinutesBeforeStart".into(), json!(m));
        }
    }
    if let Some(s) = &c.sensitivity {
        b.insert("sensitivity".into(), json!(if s == "private" { "private" } else { "normal" }));
    }
    if let Some(r) = &c.recurrence {
        let tz = c.times.as_ref().map(|t| t.time_zone.as_str()).unwrap_or(time_zone);
        b.insert("recurrence".into(), r.as_ref().map(|r| recurrence::to_graph(r, tz)).unwrap_or(Value::Null));
    }
    if let Some(provider) = &c.online_meeting {
        b.insert("isOnlineMeeting".into(), json!(true));
        b.insert("onlineMeetingProvider".into(), json!(provider));
    }
    Value::Object(b)
}

fn graph_instant(v: &Value) -> Option<String> {
    let dt = v.get("dateTime")?.as_str()?;
    let base = dt.split('.').next()?.trim_end_matches('Z');
    (base.len() == 19).then(|| format!("{base}Z"))
}

pub fn from_graph(v: &Value, series: bool, local_zone: &str) -> Option<EditableEvent> {
    let all_day = v["isAllDay"].as_bool().unwrap_or(false);
    let date = |p: &str| v.pointer(p).and_then(Value::as_str).and_then(|d| d.get(..10)).map(String::from);
    let times = if all_day {
        Times { start: date("/start/dateTime")?, end: date("/end/dateTime")?, all_day, time_zone: local_zone.into(), local_start: None, local_end: None }
    } else {
        Times { start: graph_instant(&v["start"])?, end: graph_instant(&v["end"])?, all_day, time_zone: local_zone.into(), local_start: None, local_end: None }
    };
    let body = v.pointer("/body/content").and_then(Value::as_str).unwrap_or_default();
    Some(EditableEvent {
        remote_id: v["id"].as_str()?.to_string(),
        subject: v["subject"].as_str().unwrap_or_default().to_string(),
        times,
        location: v.pointer("/location/displayName").and_then(Value::as_str).unwrap_or_default().to_string(),
        location_email: v.pointer("/location/locationEmailAddress").and_then(Value::as_str).filter(|e| !e.is_empty()).map(String::from),
        description_html: match v.pointer("/body/contentType").and_then(Value::as_str) {
            Some("text") => crate::sanitize::text_to_html(body),
            _ => crate::sanitize::clean_html(body),
        },
        attendees: v["attendees"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                Some(EditAttendee {
                    name: a.pointer("/emailAddress/name").and_then(Value::as_str).map(String::from),
                    address: a.pointer("/emailAddress/address").and_then(Value::as_str)?.to_string(),
                    optional: a["type"] == "optional",
                    response: a.pointer("/status/response").and_then(Value::as_str).map(String::from),
                    resource: a["type"] == "resource",
                })
            })
            .collect(),
        show_as: v["showAs"].as_str().unwrap_or("busy").to_string(),
        reminder_minutes: v["isReminderOn"].as_bool().unwrap_or(false).then(|| v["reminderMinutesBeforeStart"].as_i64().unwrap_or(15)),
        reminder_default: false,
        sensitivity: if v["sensitivity"] == "private" { "private".into() } else { "normal".into() },
        recurrence: v.get("recurrence").filter(|r| !r.is_null()).and_then(recurrence::from_graph),
        online_meeting: v["isOnlineMeeting"].as_bool().unwrap_or(false) || v.pointer("/onlineMeeting/joinUrl").and_then(Value::as_str).is_some(),
        is_organizer: v["isOrganizer"].as_bool().unwrap_or(true),
        series,
    })
}

// ---------- Google ----------

fn google_response(r: Option<&str>) -> &'static str {
    match r {
        Some("accepted") => "accepted",
        Some("tentativelyAccepted") => "tentative",
        Some("declined") => "declined",
        _ => "needsAction",
    }
}

/// Google Calendar fields for these changes.
pub fn google_body(c: &EventChanges) -> Value {
    let mut b = Map::new();
    if let Some(s) = &c.subject {
        b.insert("summary".into(), json!(s));
    }
    if let Some(t) = &c.times {
        if t.all_day {
            b.insert("start".into(), json!({ "date": t.start }));
            b.insert("end".into(), json!({ "date": t.end }));
        } else if let Some((s, e)) = t.local() {
            // Google needs the zone for a recurring event's times anyway.
            b.insert("start".into(), json!({ "dateTime": s, "timeZone": t.time_zone }));
            b.insert("end".into(), json!({ "dateTime": e, "timeZone": t.time_zone }));
        } else {
            b.insert("start".into(), json!({ "dateTime": t.start, "timeZone": "UTC" }));
            b.insert("end".into(), json!({ "dateTime": t.end, "timeZone": "UTC" }));
        }
    }
    if let Some(l) = &c.location {
        b.insert("location".into(), json!(l));
    }
    if let Some(h) = &c.description_html {
        b.insert("description".into(), json!(crate::sanitize::clean_html(h)));
    }
    if let Some(list) = &c.attendees {
        let people: Vec<Value> = list
            .iter()
            .map(|a| {
                let mut p = json!({ "email": a.address, "optional": a.optional, "responseStatus": google_response(a.response.as_deref()) });
                if a.resource {
                    p["resource"] = json!(true);
                }
                if let Some(n) = &a.name {
                    p["displayName"] = json!(n);
                }
                p
            })
            .collect();
        b.insert("attendees".into(), Value::Array(people));
    }
    if let Some(s) = &c.show_as {
        b.insert("transparency".into(), json!(if s == "free" { "transparent" } else { "opaque" }));
    }
    if let Some(r) = &c.reminder {
        let overrides: Vec<Value> = r.minutes.map(|m| json!({ "method": "popup", "minutes": m })).into_iter().collect();
        b.insert("reminders".into(), json!({ "useDefault": false, "overrides": overrides }));
    }
    if let Some(s) = &c.sensitivity {
        b.insert("visibility".into(), json!(if s == "private" { "private" } else { "default" }));
    }
    if let Some(r) = &c.recurrence {
        b.insert("recurrence".into(), json!(r.as_ref().map(|r| vec![recurrence::to_rrule(r)]).unwrap_or_default()));
    }
    if c.online_meeting.is_some() {
        // Only honoured with conferenceDataVersion=1 on the request (gcal::execute adds it).
        let request_id = format!("peetmail-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default());
        b.insert("conferenceData".into(), json!({ "createRequest": { "requestId": request_id, "conferenceSolutionKey": { "type": "hangoutsMeet" } } }));
    }
    Value::Object(b)
}

pub fn from_google(v: &Value, series: bool, local_zone: &str) -> Option<EditableEvent> {
    let s = |p: &str| v.pointer(p).and_then(Value::as_str).map(String::from);
    let utc = |x: &str| chrono::DateTime::parse_from_rfc3339(x).ok().map(|d| d.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%SZ").to_string());
    let times = match (s("/start/dateTime"), s("/start/date")) {
        (Some(st), _) => Times { start: utc(&st)?, end: utc(&s("/end/dateTime")?)?, all_day: false, time_zone: local_zone.into(), local_start: None, local_end: None },
        (None, Some(d)) => Times { start: d, end: s("/end/date")?, all_day: true, time_zone: local_zone.into(), local_start: None, local_end: None },
        _ => return None,
    };
    let description = s("/description").unwrap_or_default();
    let reminders = &v["reminders"];
    let use_default = reminders["useDefault"].as_bool().unwrap_or(true);
    let lines: Option<Vec<String>> = v["recurrence"].as_array().map(|a| a.iter().filter_map(|l| l.as_str().map(String::from)).collect());
    let start_date = times.start.get(..10).unwrap_or_default().to_string();
    let attendees: Vec<EditAttendee> = v["attendees"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["organizer"].as_bool() != Some(true) || a["self"].as_bool() != Some(true))
        .filter_map(|a| {
            Some(EditAttendee {
                name: a["displayName"].as_str().map(String::from),
                address: a["email"].as_str()?.to_string(),
                optional: a["optional"].as_bool().unwrap_or(false),
                response: a["responseStatus"].as_str().map(|r| match r {
                    "accepted" => "accepted",
                    "tentative" => "tentativelyAccepted",
                    "declined" => "declined",
                    _ => "notResponded",
                }.to_string()),
                resource: a["resource"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    Some(EditableEvent {
        remote_id: s("/id")?,
        subject: s("/summary").unwrap_or_default(),
        times,
        location: s("/location").unwrap_or_default(),
        location_email: None,
        description_html: if description.contains('<') { crate::sanitize::clean_html(&description) } else { crate::sanitize::text_to_html(&description) },
        attendees,
        show_as: if s("/transparency").as_deref() == Some("transparent") { "free".into() } else { "busy".into() },
        reminder_minutes: if use_default { None } else { reminders["overrides"].as_array().into_iter().flatten().filter_map(|o| o["minutes"].as_i64()).min() },
        reminder_default: use_default,
        sensitivity: if matches!(s("/visibility").as_deref(), Some("private" | "confidential")) { "private".into() } else { "normal".into() },
        recurrence: lines.and_then(|l| recurrence::from_rrule(&l, &start_date)),
        online_meeting: v.get("conferenceData").is_some_and(|c| !c.is_null()) || v.get("hangoutLink").is_some_and(|l| l.is_string()),
        // Google marks your own events with organizer.self; events without an organizer are yours.
        is_organizer: v.pointer("/organizer/self").and_then(Value::as_bool).unwrap_or(v.get("organizer").is_none()),
        series,
    })
}

// ---------- loading ----------

/// The event the editor works on: the occurrence itself, or (series) its master.
pub async fn load(state: &AppState, account_id: i64, remote_id: &str, series: bool, local_zone: &str) -> Result<EditableEvent> {
    let target = if series {
        state
            .db
            .with(|c| crate::calendar::event_by_remote(c, account_id, remote_id))?
            .and_then(|e| e.series_master_id)
            .unwrap_or_else(|| remote_id.to_string())
    } else {
        remote_id.to_string()
    };
    let google = state.db.with(|c| crate::db::account_provider(c, account_id))? == "google";
    let v = if google {
        crate::gcal::get_event(state, account_id, &target).await?
    } else {
        let url = format!("{}/me/events/{target}", crate::graph::base());
        crate::mail::graph_call(
            state,
            account_id,
            reqwest::Method::GET,
            &url,
            None,
            &[crate::graph::PREFER_IMMUTABLE, r#"outlook.timezone="UTC""#, r#"outlook.body-content-type="html""#],
        )
        .await?
    };
    let parsed = if google { from_google(&v, series, local_zone) } else { from_graph(&v, series, local_zone) };
    parsed.ok_or_else(|| AppError::Other("couldn't read that event".into()))
}

// ---------- online meetings ----------

/// Which online meeting a new event can get from this calendar (`GET /me/calendar` with
/// allowed/defaultOnlineMeetingProvider): its default when that's allowed, else the first
/// allowed one, Teams before Skype. None when it offers none.
pub fn pick_meeting_provider(calendar: &Value) -> Option<String> {
    let allowed: Vec<&str> = calendar["allowedOnlineMeetingProviders"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|p| *p != "unknown").collect();
    if let Some(d) = calendar["defaultOnlineMeetingProvider"].as_str().filter(|d| allowed.contains(d)) {
        return Some(d.to_string());
    }
    ["teamsForBusiness", "skypeForBusiness", "skypeForConsumer"].into_iter().find(|p| allowed.contains(p)).or(allowed.first().copied()).map(String::from)
}

/// The online meeting the editor offers for this account: Google Meet for Google, else
/// what the Outlook calendar allows (Teams for work accounts).
pub async fn meeting_provider(state: &AppState, account_id: i64) -> Result<Option<String>> {
    if state.db.with(|c| crate::db::account_provider(c, account_id))? == "google" {
        return Ok(Some("hangoutsMeet".into()));
    }
    let url = format!("{}/me/calendar?$select=allowedOnlineMeetingProviders,defaultOnlineMeetingProvider", crate::graph::base());
    let v = crate::mail::graph_call(state, account_id, reqwest::Method::GET, &url, None, &[]).await?;
    Ok(pick_meeting_provider(&v))
}

// ---------- local copy ----------

fn attendees_json(list: &[EditAttendee]) -> String {
    Value::Array(
        list.iter()
            .map(|a| json!({ "name": a.name, "address": a.address, "response": a.response.clone().unwrap_or_else(|| "notResponded".into()), "type": if a.resource { "resource" } else if a.optional { "optional" } else { "required" } }))
            .collect(),
    )
    .to_string()
}

/// Shows an edit in the calendar at once (the server's copy replaces it on the next sync).
pub fn apply_locally(conn: &Connection, account_id: i64, remote_id: &str, c: &EventChanges) -> Result<()> {
    let set = |sql: &str, v: &dyn rusqlite::ToSql| -> Result<()> {
        conn.execute(&format!("UPDATE events SET {sql} WHERE account_id = ?1 AND remote_id = ?2"), params![account_id, remote_id, v])?;
        Ok(())
    };
    if let Some(s) = &c.subject {
        set("subject = ?3", s)?;
    }
    if let Some(t) = &c.times {
        set("start_at = ?3", &t.start)?;
        set("end_at = ?3", &t.end)?;
        set("is_all_day = ?3", &t.all_day)?;
    }
    if let Some(l) = &c.location {
        let v: Option<&String> = (!l.trim().is_empty()).then_some(l);
        set("location = ?3", &v)?;
    }
    if let Some(h) = &c.description_html {
        let text: String = crate::sanitize::html_to_text(&crate::sanitize::clean_html(h)).chars().take(255).collect();
        set("preview = ?3", &text)?;
    }
    if let Some(a) = &c.attendees {
        set("attendees_json = ?3", &attendees_json(a))?;
    }
    if let Some(s) = &c.show_as {
        set("show_as = ?3", s)?;
    }
    Ok(())
}

/// Removes an event (and, for a series master, its occurrences) from the local calendar.
pub fn delete_locally(conn: &Connection, account_id: i64, remote_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM events WHERE account_id = ?1 AND (remote_id = ?2 OR series_master_id = ?2)",
        params![account_id, remote_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changes() -> EventChanges {
        EventChanges {
            subject: Some("Planning".into()),
            times: Some(Times { start: "2026-10-30T02:00:00Z".into(), end: "2026-10-30T03:00:00Z".into(), all_day: false, time_zone: "Australia/Sydney".into(), local_start: None, local_end: None }),
            location: Some("Room 2".into()),
            location_email: None,
            description_html: Some("<p>Bring <b>numbers</b></p><script>x()</script>".into()),
            attendees: Some(vec![
                EditAttendee { name: Some("Sam".into()), address: "sam@x.com".into(), optional: false, response: Some("accepted".into()), resource: false },
                EditAttendee { name: None, address: "jo@x.com".into(), optional: true, response: None, resource: false },
            ]),
            show_as: Some("free".into()),
            reminder: Some(Reminder { minutes: Some(30) }),
            sensitivity: Some("private".into()),
            recurrence: None,
            online_meeting: None,
        }
    }

    #[test]
    fn an_online_meeting_asks_graph_for_teams_and_google_for_meet() {
        let c = EventChanges { online_meeting: Some("teamsForBusiness".into()), ..Default::default() };
        assert_eq!(graph_body(&c, "UTC"), json!({ "isOnlineMeeting": true, "onlineMeetingProvider": "teamsForBusiness" }));
        let g = google_body(&EventChanges { online_meeting: Some("hangoutsMeet".into()), ..Default::default() });
        assert_eq!(g["conferenceData"]["createRequest"]["conferenceSolutionKey"]["type"], "hangoutsMeet");
        assert!(g["conferenceData"]["createRequest"]["requestId"].as_str().is_some_and(|id| !id.is_empty()));
        assert!(graph_body(&changes(), "UTC").get("isOnlineMeeting").is_none(), "only when asked");
        assert!(google_body(&changes()).get("conferenceData").is_none());
    }

    #[test]
    fn picks_the_calendars_meeting_provider() {
        let cal = |allowed: Value, default: &str| json!({ "allowedOnlineMeetingProviders": allowed, "defaultOnlineMeetingProvider": default });
        assert_eq!(pick_meeting_provider(&cal(json!(["teamsForBusiness"]), "teamsForBusiness")).as_deref(), Some("teamsForBusiness"));
        // A default it doesn't allow (or none) falls back to what's allowed, Teams first.
        assert_eq!(pick_meeting_provider(&cal(json!(["skypeForBusiness", "teamsForBusiness"]), "unknown")).as_deref(), Some("teamsForBusiness"));
        assert_eq!(pick_meeting_provider(&cal(json!(["skypeForConsumer"]), "teamsForBusiness")).as_deref(), Some("skypeForConsumer"));
        assert_eq!(pick_meeting_provider(&cal(json!(["unknown"]), "unknown")), None);
        assert_eq!(pick_meeting_provider(&json!({})), None);
    }

    #[test]
    fn the_editor_knows_when_an_event_already_has_an_online_meeting() {
        let base = json!({ "id": "e", "subject": "s", "isAllDay": false,
            "start": { "dateTime": "2026-10-30T02:00:00.0000000" }, "end": { "dateTime": "2026-10-30T03:00:00.0000000" } });
        assert!(!from_graph(&base, false, "UTC").unwrap().online_meeting);
        let mut teams = base.clone();
        teams["isOnlineMeeting"] = json!(true);
        assert!(from_graph(&teams, false, "UTC").unwrap().online_meeting);
        let g = json!({ "id": "g", "summary": "s", "start": { "dateTime": "2026-10-30T02:00:00Z" }, "end": { "dateTime": "2026-10-30T03:00:00Z" } });
        assert!(!from_google(&g, false, "UTC").unwrap().online_meeting);
        let mut meet = g.clone();
        meet["hangoutLink"] = json!("https://meet.google.com/abc-defg-hij");
        assert!(from_google(&meet, false, "UTC").unwrap().online_meeting);
    }

    #[test]
    fn graph_bodies_carry_only_what_changed() {
        let b = graph_body(&changes(), "UTC");
        assert_eq!(b["start"], json!({ "dateTime": "2026-10-30T02:00:00", "timeZone": "UTC" }));
        assert_eq!(b["body"]["content"], "<p>Bring <b>numbers</b></p>", "sanitized");
        assert_eq!(b["attendees"][1], json!({ "emailAddress": { "address": "jo@x.com", "name": "jo@x.com" }, "type": "optional" }));
        assert_eq!((b["showAs"].clone(), b["isReminderOn"].clone(), b["reminderMinutesBeforeStart"].clone(), b["sensitivity"].clone()), (json!("free"), json!(true), json!(30), json!("private")));
        assert!(b.get("recurrence").is_none());

        let only = graph_body(&EventChanges { location: Some(String::new()), reminder: Some(Reminder { minutes: None }), ..Default::default() }, "UTC");
        assert_eq!(only, json!({ "location": { "displayName": "" }, "isReminderOn": false }));
        let stop = graph_body(&EventChanges { recurrence: Some(None), ..Default::default() }, "UTC");
        assert_eq!(stop, json!({ "recurrence": null }));
    }

    #[test]
    fn a_series_is_sent_in_local_time_so_it_survives_daylight_saving() {
        let c = EventChanges {
            times: Some(Times {
                start: "2026-10-27T22:00:00Z".into(),
                end: "2026-10-27T22:30:00Z".into(),
                all_day: false,
                time_zone: "Australia/Sydney".into(),
                local_start: Some("2026-10-28T09:00:00".into()),
                local_end: Some("2026-10-28T09:30:00".into()),
            }),
            recurrence: Some(Some(crate::recurrence::Recurrence {
                freq: crate::recurrence::Freq::Weekly,
                interval: 1,
                days: vec!["wed".into()],
                ordinal: None,
                day_of_month: None,
                month: None,
                end: crate::recurrence::End::Count { count: 5 },
                start: "2026-10-28".into(),
            })),
            ..Default::default()
        };
        let g = graph_body(&c, "Australia/Sydney");
        assert_eq!(g["start"], json!({ "dateTime": "2026-10-28T09:00:00", "timeZone": "Australia/Sydney" }));
        assert_eq!(g["recurrence"]["pattern"]["daysOfWeek"], json!(["wednesday"]));
        assert_eq!(g["recurrence"]["range"]["numberOfOccurrences"], 5);
        let gg = google_body(&c);
        assert_eq!(gg["end"], json!({ "dateTime": "2026-10-28T09:30:00", "timeZone": "Australia/Sydney" }));
        assert_eq!(gg["recurrence"], json!(["RRULE:FREQ=WEEKLY;BYDAY=WE;COUNT=5"]));
    }

    #[test]
    fn a_room_goes_in_as_the_location_and_a_resource_attendee() {
        let c = EventChanges {
            location: Some("Boardroom".into()),
            location_email: Some("board@x.com".into()),
            attendees: Some(vec![EditAttendee { name: Some("Boardroom".into()), address: "board@x.com".into(), optional: false, response: None, resource: true }]),
            ..Default::default()
        };
        let b = graph_body(&c, "UTC");
        assert_eq!(b["location"], json!({ "displayName": "Boardroom", "locationEmailAddress": "board@x.com", "locationType": "conferenceRoom" }));
        assert_eq!(b["attendees"][0]["type"], "resource");
        assert_eq!(google_body(&c)["attendees"][0]["resource"], true);
    }

    #[test]
    fn google_bodies_keep_answers_and_map_free_busy_reminders_privacy() {
        let b = google_body(&changes());
        assert_eq!(b["summary"], "Planning");
        assert_eq!(b["attendees"][0], json!({ "email": "sam@x.com", "displayName": "Sam", "optional": false, "responseStatus": "accepted" }));
        assert_eq!(b["attendees"][1]["responseStatus"], "needsAction");
        assert_eq!((b["transparency"].clone(), b["visibility"].clone()), (json!("transparent"), json!("private")));
        assert_eq!(b["reminders"], json!({ "useDefault": false, "overrides": [{ "method": "popup", "minutes": 30 }] }));
        let all_day = google_body(&EventChanges { times: Some(Times { start: "2026-11-06".into(), end: "2026-11-07".into(), all_day: true, time_zone: "UTC".into(), local_start: None, local_end: None }), ..Default::default() });
        assert_eq!(all_day, json!({ "start": { "date": "2026-11-06" }, "end": { "date": "2026-11-07" } }));
        assert_eq!(google_body(&EventChanges { recurrence: Some(None), ..Default::default() }), json!({ "recurrence": [] }));
    }

    #[test]
    fn recurrence_null_means_stop_and_missing_means_unchanged() {
        let stop: EventChanges = serde_json::from_str(r#"{"recurrence": null}"#).unwrap();
        assert_eq!(stop.recurrence, Some(None));
        let same: EventChanges = serde_json::from_str("{}").unwrap();
        assert_eq!(same.recurrence, None);
        assert!(same.is_empty());
        // Through the outbox (stored as JSON and read back) both survive as they were.
        let back = |c: &EventChanges| serde_json::from_str::<EventChanges>(&serde_json::to_string(c).unwrap()).unwrap();
        assert_eq!(back(&same).recurrence, None, "unchanged must not turn into 'stop repeating'");
        assert_eq!(back(&stop).recurrence, Some(None));
    }

    #[test]
    fn reads_editable_events_from_both_services() {
        let g = json!({
            "id": "e1", "subject": "Review", "isAllDay": false, "isOrganizer": true,
            "start": { "dateTime": "2026-10-30T02:00:00.0000000", "timeZone": "UTC" }, "end": { "dateTime": "2026-10-30T03:00:00.0000000", "timeZone": "UTC" },
            "location": { "displayName": "Room 2" }, "body": { "contentType": "html", "content": "<p>Hi</p>" },
            "attendees": [{ "emailAddress": { "name": "Sam", "address": "sam@x.com" }, "type": "optional", "status": { "response": "accepted" } }],
            "showAs": "tentative", "isReminderOn": true, "reminderMinutesBeforeStart": 10, "sensitivity": "private"
        });
        let e = from_graph(&g, false, "Australia/Sydney").unwrap();
        assert_eq!((e.times.start.as_str(), e.location.as_str(), e.reminder_minutes, e.sensitivity.as_str()), ("2026-10-30T02:00:00Z", "Room 2", Some(10), "private"));
        assert!(e.attendees[0].optional && e.is_organizer);

        let google = json!({
            "id": "g1", "summary": "Yoga", "start": { "date": "2026-11-06" }, "end": { "date": "2026-11-07" },
            "organizer": { "email": "me@gmail.com", "self": true }, "transparency": "transparent",
            "reminders": { "useDefault": true }, "recurrence": ["RRULE:FREQ=WEEKLY;COUNT=3"],
            "attendees": [{ "email": "me@gmail.com", "self": true, "organizer": true, "responseStatus": "accepted" }, { "email": "kim@x.com", "responseStatus": "tentative" }]
        });
        let e = from_google(&google, true, "UTC").unwrap();
        assert!(e.times.all_day && e.is_organizer && e.reminder_default && e.series);
        assert_eq!(e.show_as, "free");
        assert_eq!(e.attendees.iter().map(|a| a.address.as_str()).collect::<Vec<_>>(), vec!["kim@x.com"], "you aren't your own guest");
        assert_eq!(e.attendees[0].response.as_deref(), Some("tentativelyAccepted"));
        assert_eq!(e.recurrence.unwrap().end, crate::recurrence::End::Count { count: 3 });
    }
}
