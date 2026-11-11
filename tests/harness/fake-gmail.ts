// A fake Gmail API + Google OAuth, in-process, for tests. Covers what PeetMail uses:
//
//   /oauth/authorize, /oauth/token      Google-style OAuth (PKCE; secret checked if set)
//   GET  /gmail/v1/users/me/profile
//   GET  .../messages?labelIds=&q=after:N in:x -in:x&includeSpamTrash=&maxResults=&pageToken=
//   GET  .../messages/{id}?format=metadata|full
//   GET  .../threads/{id}?format=metadata
//   GET  .../messages/{id}/attachments/{attachmentId}
//   POST .../messages/{id}/modify | trash | untrash
//   GET  .../history?startHistoryId=     (404 when older than the kept history)
//   GET/POST .../labels
//
// Control API under /_test (deliver, label, delete, message?id=, state, config).

import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { createHash, randomBytes } from "node:crypto";

type Json = Record<string, any>;

export const GOOGLE_CLIENT_ID = "1234567890-fake.apps.googleusercontent.com";
export const GOOGLE_CLIENT_SECRET = "GOCSPX-fake-secret";
export const GMAIL_ACCOUNT = { name: "Boris Dragon", email: "boris.dragon@gmail.com" };

export type FakeGmailMessage = {
  id: string;
  threadId: string;
  labelIds: string[];
  internalDate: number; // ms
  from: string;
  to: string;
  cc?: string;
  /** Drafts keep their Bcc header. */
  bcc?: string;
  inReplyTo?: string;
  subject: string;
  snippet: string;
  html?: string;
  /** Raw bytes of the html part, for charset tests (overrides html). */
  htmlBytes?: Buffer;
  htmlCharset?: string;
  text?: string;
  attachments?: { filename: string; mimeType: string; data: Buffer; cid?: string }[];
  references?: string;
  /** For messages the app sent: the raw RFC 2822 text. */
  raw?: string;
  /** An invitation's iCalendar text (a text/calendar part). */
  ics?: string;
};

export type GEvent = Json;

/** iCalendar text of an invitation. */
export function inviteIcs(uid: string, method = "REQUEST", partstat = "NEEDS-ACTION") {
  return [
    "BEGIN:VCALENDAR", "PRODID:-//Google Inc//Google Calendar 70.9054//EN", "VERSION:2.0", `METHOD:${method}`,
    "BEGIN:VEVENT", "DTSTART:20261029T010000Z", "DTEND:20261029T020000Z", `UID:${uid}`,
    `ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=${partstat};CN=${GMAIL_ACCOUNT.email}:mailto:${GMAIL_ACCOUNT.email}`,
    "SUMMARY:Team lunch", "END:VEVENT", "END:VCALENDAR", "",
  ].join("\r\n");
}

const day = 86_400_000;
const at = (now: number, days: number, hour: number, min = 0) => {
  const d = new Date(now + days * day);
  d.setUTCHours(hour, min, 0, 0);
  return d.toISOString().replace(".000Z", "Z");
};
const date = (now: number, days: number) => new Date(now + days * day).toISOString().slice(0, 10);
const me = { email: GMAIL_ACCOUNT.email, displayName: GMAIL_ACCOUNT.name, self: true };

/** A small Google calendar, relative to `now`. Series are given as master + instances. */
export function defaultGcal(now = Date.now()): GEvent[] {
  const yoga = (i: number, days: number) => ({
    kind: "calendar#event", id: `gcal-yoga_${i}`, status: "confirmed", summary: "Yoga", recurringEventId: "gcal-yoga", iCalUID: "yoga@google.com",
    start: { dateTime: at(now, days, 7) }, end: { dateTime: at(now, days, 8) }, originalStartTime: { dateTime: at(now, days, 7) },
    organizer: me, transparency: "transparent", htmlLink: "https://calendar.google.com/yoga",
  });
  return [
    { kind: "calendar#event", id: "gcal-dentist", status: "confirmed", summary: "Dentist", iCalUID: "dentist@google.com", location: "12 Main St",
      start: { dateTime: at(now, 1, 22) }, end: { dateTime: at(now, 1, 22, 30) }, organizer: me, htmlLink: "https://calendar.google.com/dentist" },
    { kind: "calendar#event", id: "gcal-lunch", status: "confirmed", summary: "Team lunch", iCalUID: "lunch-123@google.com", location: "Café Beetroot",
      start: { dateTime: at(now, 2, 1) }, end: { dateTime: at(now, 2, 2) }, hangoutLink: "https://meet.google.com/abc-defg-hij",
      description: "Bring <b>ideas</b> for the offsite.", reminders: { useDefault: false, overrides: [{ method: "popup", minutes: 10 }] },
      organizer: { email: "kim@friends.example", displayName: "Kim Park" },
      attendees: [
        { email: "kim@friends.example", displayName: "Kim Park", organizer: true, responseStatus: "accepted" },
        { ...me, responseStatus: "needsAction" },
        { email: "lee@friends.example", displayName: "Lee Wong", responseStatus: "tentative", optional: true },
      ] },
    { kind: "calendar#event", id: "gcal-yoga", status: "confirmed", summary: "Yoga", iCalUID: "yoga@google.com", recurrence: ["RRULE:FREQ=WEEKLY;COUNT=10"],
      start: { dateTime: at(now, 1, 7) }, end: { dateTime: at(now, 1, 8) }, organizer: me, transparency: "transparent" },
    yoga(1, 1), yoga(2, 8), yoga(3, 15),
    { kind: "calendar#event", id: "gcal-holiday", status: "confirmed", summary: "Public holiday", iCalUID: "holiday@google.com",
      start: { date: date(now, 3) }, end: { date: date(now, 4) }, organizer: me, transparency: "transparent", visibility: "private" },
    { kind: "calendar#event", id: "gcal-old", status: "confirmed", summary: "Long ago", iCalUID: "old@google.com",
      start: { dateTime: at(now, -60, 9) }, end: { dateTime: at(now, -60, 10) }, organizer: me },
  ];
}

const b64u = (b: Buffer | string) => Buffer.from(b).toString("base64url");
const PIXEL_PNG = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");

/** A small Gmail mailbox, relative to `now`. */
export function defaultGmail(now = Date.now()): FakeGmailMessage[] {
  const h = 3_600_000;
  const me = `${GMAIL_ACCOUNT.name} <${GMAIL_ACCOUNT.email}>`;
  return [
    { id: "g-1", threadId: "g-thread-trip", labelIds: ["INBOX", "UNREAD", "CATEGORY_PERSONAL"], internalDate: now - 1 * h, from: "Kim Park <kim@friends.example>", to: me, subject: "Ski trip in January", snippet: "Are you still keen? I found a lodge &amp; it&#39;s cheap.", html: "<p>Are you still keen? I found a lodge &amp; it's cheap.</p>" },
    { id: "g-2", threadId: "g-thread-trip", labelIds: ["INBOX", "CATEGORY_PERSONAL"], internalDate: now - 30 * 60_000, from: "Lee Wong <lee@friends.example>", to: me, cc: "\"Park, Kim\" <kim@friends.example>", subject: "Re: Ski trip in January", snippet: "Count me in.", text: "Count me in.\n\nLee" },
    { id: "g-3", threadId: "g-thread-sale", labelIds: ["INBOX", "UNREAD", "CATEGORY_PROMOTIONS"], internalDate: now - 3 * h, from: "Gear Shop <deals@gear.example>", to: me, subject: "=?UTF-8?B?V2ludGVyIHNhbGUg4p2E?=", snippet: "Up to 40% off jackets", html: "<p>Up to 40% off jackets</p>" },
    {
      id: "g-4", threadId: "g-thread-photos", labelIds: ["INBOX", "UNREAD", "CATEGORY_PERSONAL"], internalDate: now - 5 * h, from: "Mum <mum@family.example>", to: me, subject: "Photos from Sunday", snippet: "Here they are, and the plan.",
      html: '<p>Here they are, and the plan.</p><img src="cid:pixel@mum" alt="pixel">',
      attachments: [
        { filename: "pixel.png", mimeType: "image/png", data: PIXEL_PNG, cid: "pixel@mum" },
        { filename: "plan.pdf", mimeType: "application/pdf", data: Buffer.from("%PDF-1.4 fake plan") },
      ],
    },
    { id: "g-5", threadId: "g-thread-latin", labelIds: ["INBOX", "CATEGORY_PERSONAL"], internalDate: now - 8 * h, from: "Café Lisboa <ola@cafe.example>", to: me, subject: "Reserva", snippet: "Obrigado pela reserva", htmlBytes: Buffer.from("<p>Caf\xe9 confirmado</p>", "latin1"), htmlCharset: "ISO-8859-1" },
    { id: "g-6", threadId: "g-thread-trip", labelIds: ["SENT"], internalDate: now - 45 * 60_000, from: me, to: "Kim Park <kim@friends.example>", subject: "Re: Ski trip in January", snippet: "Yes! Send me the link.", text: "Yes! Send me the link." },
    { id: "g-7", threadId: "g-thread-old-archived", labelIds: ["CATEGORY_PERSONAL"], internalDate: now - 20 * h, from: "Bank <no-reply@bank.example>", to: me, subject: "Statement ready", snippet: "Your statement is ready", text: "Your statement is ready" },
    { id: "g-9", threadId: "g-thread-invite", labelIds: ["INBOX", "UNREAD", "CATEGORY_PERSONAL"], internalDate: now - 2 * h, from: "Kim Park <kim@friends.example>", to: me, subject: "Invitation: Team lunch", snippet: "You have been invited to Team lunch", html: "<p>You have been invited to <b>Team lunch</b>.</p>", ics: inviteIcs("lunch-123@google.com") },
    { id: "g-10", threadId: "g-thread-trashed", labelIds: ["TRASH", "CATEGORY_PERSONAL"], internalDate: now - 22 * h, from: "Bank <no-reply@bank.example>", to: me, subject: "Old offer", snippet: "Limited time", text: "Limited time" },
    { id: "g-11", threadId: "g-thread-spam", labelIds: ["SPAM"], internalDate: now - 21 * h, from: "Prize <win@spam.example>", to: me, subject: "You won", snippet: "Claim now", text: "Claim now" },
    { id: "g-8", threadId: "g-thread-ancient", labelIds: ["INBOX"], internalDate: now - 400 * 24 * h, from: "Old Friend <old@friends.example>", to: me, subject: "Long ago", snippet: "Remember this?", text: "Remember this?" },
  ];
}

/** A parsed MIME entity (what the app sent), for assertions. */
export type MimeEntity = { headers: Record<string, string>; contentType: string; filename: string | null; body: Buffer; parts: MimeEntity[] };

/** Parses a raw RFC 2822 message (CRLF), decoding base64 / quoted-printable bodies. */
export function parseMime(raw: string | Buffer): MimeEntity {
  const text = Buffer.isBuffer(raw) ? raw.toString("latin1") : raw;
  const split = text.indexOf("\r\n\r\n");
  const headText = split >= 0 ? text.slice(0, split) : text;
  const bodyText = split >= 0 ? text.slice(split + 4) : "";
  const headers: Record<string, string> = {};
  for (const line of headText.replace(/\r\n[ \t]+/g, " ").split("\r\n")) {
    const i = line.indexOf(":");
    if (i > 0) headers[line.slice(0, i).toLowerCase()] = line.slice(i + 1).trim();
  }
  const contentType = (headers["content-type"] ?? "text/plain").split(";")[0].trim().toLowerCase();
  const filename = (headers["content-disposition"] ?? "").match(/filename="?([^";]+)"?/)?.[1] ?? null;
  if (contentType.startsWith("multipart/")) {
    const boundary = headers["content-type"].match(/boundary="?([^";]+)"?/)![1];
    const chunks = bodyText.split(`--${boundary}`).slice(1);
    const parts = chunks.filter((c) => !c.startsWith("--")).map((c) => parseMime(c.replace(/^\r\n/, "").replace(/\r\n$/, "")));
    return { headers, contentType, filename, body: Buffer.alloc(0), parts };
  }
  const enc = (headers["content-transfer-encoding"] ?? "").toLowerCase();
  const body = enc === "base64" ? Buffer.from(bodyText.replace(/\s+/g, ""), "base64") : Buffer.from(bodyText, "latin1");
  return { headers, contentType, filename, body, parts: [] };
}

/** Every leaf part, depth first. */
export function leaves(e: MimeEntity): MimeEntity[] {
  return e.parts.length ? e.parts.flatMap(leaves) : [e];
}

function payloadOf(m: FakeGmailMessage, format: string): Json {
  const headers = [
    { name: "From", value: m.from },
    { name: "To", value: m.to },
    ...(m.cc ? [{ name: "Cc", value: m.cc }] : []),
    ...(m.bcc ? [{ name: "Bcc", value: m.bcc }] : []),
    ...(m.inReplyTo ? [{ name: "In-Reply-To", value: m.inReplyTo }] : []),
    { name: "Subject", value: m.subject },
    { name: "Date", value: new Date(m.internalDate).toUTCString().replace("GMT", "+0000") },
    { name: "Message-ID", value: `<${m.id}@mail.example>` },
    ...(m.references ? [{ name: "References", value: m.references }] : []),
  ];
  const alt: Json[] = [];
  if (m.ics) alt.push({ partId: "0.2", mimeType: "text/calendar", filename: "", headers: [{ name: "Content-Type", value: "text/calendar; charset=UTF-8; method=REQUEST" }], body: { size: m.ics.length, data: b64u(m.ics) } });
  if (m.text) alt.push({ partId: "0.0", mimeType: "text/plain", filename: "", headers: [{ name: "Content-Type", value: "text/plain; charset=UTF-8" }], body: { size: m.text.length, data: b64u(m.text) } });
  if (m.html || m.htmlBytes) {
    const bytes = m.htmlBytes ?? Buffer.from(m.html!);
    alt.push({ partId: "0.1", mimeType: "text/html", filename: "", headers: [{ name: "Content-Type", value: `text/html; charset=${m.htmlCharset ?? "UTF-8"}` }], body: { size: bytes.length, data: b64u(bytes) } });
  }
  const atts = (m.attachments ?? []).map((a, i) => ({
    partId: String(i + 1),
    mimeType: a.mimeType,
    filename: a.filename,
    headers: [
      ...(a.cid ? [{ name: "Content-ID", value: `<${a.cid}>` }, { name: "Content-Disposition", value: `inline; filename=${a.filename}` }] : [{ name: "Content-Disposition", value: `attachment; filename=${a.filename}` }]),
    ],
    // Like Gmail: attachment ids change between fetches.
    body: { size: a.data.length, attachmentId: `att-${m.id}-${i}-${randomBytes(3).toString("hex")}` },
  }));
  const payload: Json = atts.length
    ? { mimeType: "multipart/mixed", headers, parts: [{ partId: "0", mimeType: "multipart/alternative", parts: alt }, ...atts] }
    : { mimeType: "multipart/alternative", headers, parts: alt };
  if (format === "metadata") {
    // Metadata keeps the structure's top level but no bodies, and only the asked-for headers.
    return { mimeType: payload.mimeType, headers: headers.filter((h) => ["From", "To", "Cc", "Subject"].includes(h.name)) };
  }
  return payload;
}

/**
 * Gmail search beyond the in:/after: terms sync uses, as PeetMail's server search sends
 * it: from: subject: {to:x cc:x} is:unread|read|starred has:attachment, words and
 * "phrases" (anywhere in the subject, sender, snippet or text), `-` to negate.
 */
function searchMatcher(q: string): (m: FakeGmailMessage) => boolean {
  const rest = q.replace(/-?in:\w+/g, "").replace(/after:\d+/g, "");
  const terms = [...rest.matchAll(/(-?)(\{[^}]*\}|(?:(\w+):)?("[^"]*"|\S+))/g)].map((t) => {
    const group = t[2].startsWith("{");
    return { not: t[1] === "-", key: group ? "to" : t[3]?.toLowerCase(), value: (group ? t[2].match(/to:(\S+)/)?.[1] ?? "" : t[4]).replace(/^"|"$/g, "").toLowerCase() };
  });
  return (m) =>
    terms.every((t) => {
      const hit =
        t.key === "from" ? m.from.toLowerCase().includes(t.value)
        : t.key === "to" ? `${m.to} ${m.cc ?? ""}`.toLowerCase().includes(t.value)
        : t.key === "subject" ? m.subject.toLowerCase().includes(t.value)
        : t.key === "is" ? (t.value === "unread" ? m.labelIds.includes("UNREAD") : t.value === "read" ? !m.labelIds.includes("UNREAD") : m.labelIds.includes("STARRED"))
        : t.key === "has" ? !!m.attachments?.length
        : [m.subject, m.from, m.snippet, m.text ?? "", m.html ?? ""].join(" ").toLowerCase().includes(t.value);
      return hit !== t.not;
    });
}

/** A message as the app wrote it (raw RFC 2822), as the fake stores messages. */
function fromRaw(raw: string, id: string, threadId: string, labelIds: string[]): FakeGmailMessage {
  const mime = parseMime(raw);
  const parts = leaves(mime);
  const decode = (h?: string) => (h ?? "").replace(/=\?UTF-8\?B\?([^?]+)\?=\s*/g, (_, b) => Buffer.from(b, "base64").toString("utf8"));
  return {
    id, threadId, labelIds, internalDate: Date.now(),
    from: mime.headers["from"] ?? GMAIL_ACCOUNT.email, to: mime.headers["to"] ?? "", cc: mime.headers["cc"], bcc: mime.headers["bcc"],
    inReplyTo: mime.headers["in-reply-to"], references: mime.headers["references"], subject: decode(mime.headers["subject"]), snippet: "",
    text: parts.find((l) => l.contentType === "text/plain" && !l.filename)?.body.toString("utf8"),
    html: parts.find((l) => l.contentType === "text/html" && !l.filename)?.body.toString("utf8"),
    attachments: parts.filter((l) => l.filename).map((l) => ({ filename: decode(l.filename!), mimeType: l.contentType, data: l.body, cid: l.headers["content-id"]?.replace(/[<>]/g, "") })),
    raw,
  };
}

/** `n` filler inbox messages (one per thread), newest after the default mailbox. */
export function fillerGmail(n: number, now = Date.now()): FakeGmailMessage[] {
  return Array.from({ length: n }, (_, i) => ({
    id: `g-fill-${i}`, threadId: `g-thread-fill-${i}`, labelIds: ["INBOX", "CATEGORY_PERSONAL"], internalDate: now - (10 + i) * 60_000,
    from: `Person ${i} <p${i}@example.com>`, to: GMAIL_ACCOUNT.email, subject: `Filler ${i}`, snippet: `Message ${i}`, text: `Message ${i}`,
  }));
}

/** Gmail's quota cost per call (developers.google.com/workspace/gmail/api/reference/quota). */
function quotaCost(method: string, rest: string) {
  if (rest.startsWith("/history")) return 2;
  if (rest === "/profile" || (rest === "/labels" && method === "GET")) return 1;
  if (rest.endsWith("/send")) return 100;
  return 5;
}

export async function startFakeGmail({ mailbox, extra = 0 }: { mailbox?: FakeGmailMessage[]; extra?: number } = {}) {
  const state = {
    messages: new Map<string, FakeGmailMessage>(),
    labels: [] as { id: string; name: string; type: string }[],
    historyId: 1000,
    /** Oldest startHistoryId still answered (older ones get 404, like expired history). */
    minHistory: 0,
    history: [] as Json[],
    codes: new Map<string, { challenge: string; redirectUri: string }>(),
    tokens: new Set<string>(),
    refresh: new Set<string>(),
    config: {
      requireSecret: true,
      pageSize: 500,
      /** Google's per-user limits: 15,000 units a minute; and a short-term ceiling. */
      quotaPerSecond: 250,
      quotaPerMinute: 15_000,
      /** The next N Gmail calls fail with a quota error regardless. */
      failQuotaNext: 0,
      /** message id -> how many more GETs of it fail with 500. */
      failIds: {} as Record<string, number>,
      /** The primary calendar's conferenceProperties.allowedConferenceSolutionTypes. */
      conferenceTypes: ["hangoutsMeet"] as string[],
    },
    usage: [] as { at: number; cost: number }[],
    quotaErrors: 0,
    events: new Map<string, GEvent>(),
    eventSeq: 1,
    /** event id -> the seq of its last change (for sync tokens). */
    eventChanged: new Map<string, number>(),
    minSyncToken: 0,
    /** false: the token lacks calendar access (an account that signed in before the scope). */
    grantCalendar: true,
    requests: [] as { method: string; path: string; query: string; status: number }[],
    /** Writes the app made (modify, trash, label creation). */
    sent: [] as Json[],
    /** draft id -> its message id (which changes on every update, like Gmail's). */
    drafts: new Map<string, string>(),
  };
  let draftSeq = 0;
  let draftMessageSeq = 0;
  /** Stores a draft's message, replacing its previous one. Returns the draft resource. */
  function putDraft(draftId: string, m: Omit<FakeGmailMessage, "id" | "labelIds"> & Partial<Pick<FakeGmailMessage, "id">>) {
    const old = state.drafts.get(draftId);
    if (old) state.messages.delete(old);
    const id = `g-draft-msg-${++draftMessageSeq}`;
    state.messages.set(id, { ...m, id, labelIds: ["DRAFT"] });
    state.drafts.set(draftId, id);
    return { id: draftId, message: { id, threadId: m.threadId, labelIds: ["DRAFT"] } };
  }
  for (const m of [...(mailbox ?? defaultGmail()), ...fillerGmail(extra)]) state.messages.set(m.id, m);
  for (const e of defaultGcal()) state.events.set(e.id, e);
  function touchEvent(id: string) {
    state.eventChanged.set(id, ++state.eventSeq);
  }

  function log(entry: Json) {
    state.historyId++;
    state.history.push({ id: String(state.historyId), ...entry });
  }
  function changeLabels(m: FakeGmailMessage, add: string[], remove: string[]) {
    const added = add.filter((l) => !m.labelIds.includes(l));
    const removed = remove.filter((l) => m.labelIds.includes(l));
    m.labelIds = [...m.labelIds.filter((l) => !removed.includes(l)), ...added];
    if (added.length) log({ labelsAdded: [{ message: { id: m.id, threadId: m.threadId }, labelIds: added }] });
    if (removed.length) log({ labelsRemoved: [{ message: { id: m.id, threadId: m.threadId }, labelIds: removed }] });
  }
  const resource = (m: FakeGmailMessage, format: string) => ({
    id: m.id, threadId: m.threadId, labelIds: m.labelIds, snippet: m.snippet, internalDate: String(m.internalDate), historyId: String(state.historyId), payload: payloadOf(m, format),
  });

  const server = createServer((req, res) => handle(req, res).catch((e) => send(res, 500, { error: { message: String(e) } })));
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  const origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;

  function send(res: ServerResponse, status: number, body: unknown, headers: Record<string, string> = {}) {
    const isHtml = typeof body === "string";
    res.writeHead(status, { "Content-Type": isHtml ? "text/html" : "application/json", ...headers });
    res.end(isHtml ? body : JSON.stringify(body));
  }
  async function readBody(req: IncomingMessage): Promise<Json> {
    const chunks: Buffer[] = [];
    for await (const c of req) chunks.push(c as Buffer);
    const raw = Buffer.concat(chunks).toString();
    if (!raw) return {};
    if ((req.headers["content-type"] ?? "").includes("application/x-www-form-urlencoded")) return Object.fromEntries(new URLSearchParams(raw));
    return JSON.parse(raw);
  }
  const err = (status: number, message: string) => ({ error: { code: status, message, status: "ERROR" } });

  async function handle(req: IncomingMessage, res: ServerResponse) {
    const url = new URL(req.url ?? "/", origin);
    const p = url.pathname;
    const method = req.method ?? "GET";
    const reply = (status: number, body: unknown, headers?: Record<string, string>) => {
      if (!p.startsWith("/_test")) state.requests.push({ method, path: p, query: url.search, status });
      send(res, status, body, headers);
    };

    if (p.startsWith("/_test/")) return control(req, url, reply);

    if (p === "/oauth/authorize") {
      const q = url.searchParams;
      if (q.get("client_id") !== GOOGLE_CLIENT_ID) return reply(400, "<h1>Error 401: invalid_client</h1>");
      if (q.get("code_challenge_method") !== "S256") return reply(400, "<h1>PKCE required</h1>");
      if (q.get("access_type") !== "offline") return reply(400, "<h1>expected access_type=offline</h1>");
      const code = randomBytes(12).toString("base64url");
      state.codes.set(code, { challenge: q.get("code_challenge") ?? "", redirectUri: q.get("redirect_uri") ?? "" });
      const back = new URL(q.get("redirect_uri") ?? "");
      back.searchParams.set("code", code);
      back.searchParams.set("state", q.get("state") ?? "");
      return reply(302, "", { Location: back.toString() });
    }
    if (p === "/oauth/token" && method === "POST") {
      const f = await readBody(req);
      if (f.client_id !== GOOGLE_CLIENT_ID) return reply(401, { error: "invalid_client", error_description: "The OAuth client was not found." });
      if (state.config.requireSecret && f.client_secret !== GOOGLE_CLIENT_SECRET) return reply(400, { error: "invalid_request", error_description: "client_secret is missing." });
      const issue = () => {
        const access = `g-at-${randomBytes(9).toString("hex")}`;
        const refresh = `g-rt-${randomBytes(9).toString("hex")}`;
        state.tokens.add(access);
        state.refresh.add(refresh);
        const idToken = `${b64u('{"alg":"none"}')}.${b64u(JSON.stringify({ email: GMAIL_ACCOUNT.email, name: GMAIL_ACCOUNT.name }))}.`;
        return { access, refresh, idToken };
      };
      if (f.grant_type === "authorization_code") {
        const entry = state.codes.get(f.code);
        state.codes.delete(f.code);
        if (!entry || entry.redirectUri !== f.redirect_uri) return reply(400, { error: "invalid_grant", error_description: "Bad code" });
        if (createHash("sha256").update(f.code_verifier ?? "").digest("base64url") !== entry.challenge) return reply(400, { error: "invalid_grant", error_description: "PKCE failed" });
        const t = issue();
        return reply(200, { access_token: t.access, refresh_token: t.refresh, expires_in: 3599, id_token: t.idToken, token_type: "Bearer" });
      }
      if (f.grant_type === "refresh_token") {
        if (!state.refresh.has(f.refresh_token)) return reply(400, { error: "invalid_grant", error_description: "Token has been expired or revoked." });
        const t = issue();
        // Google keeps the refresh token on refresh.
        return reply(200, { access_token: t.access, expires_in: 3599, token_type: "Bearer" });
      }
      return reply(400, { error: "unsupported_grant_type" });
    }

    if (p.startsWith("/calendar/v3/")) return calendar(req, url, reply);
    const uploadBase = "/upload/gmail/v1/users/me";
    const upload = p.startsWith(uploadBase + "/");
    const uploadSend = p === "/upload/gmail/v1/users/me/messages/send";
    const base = "/gmail/v1/users/me";
    if (!p.startsWith(base) && !upload) return reply(404, err(404, "Not found"));
    const token = (req.headers.authorization ?? "").replace(/^Bearer /, "");
    if (!state.tokens.has(token)) return reply(401, err(401, "Request had invalid authentication credentials."));
    const rest = upload ? p.slice(uploadBase.length) : p.slice(base.length);

    // Quota, like Google: per-user units over a rolling second and minute.
    const now = Date.now();
    state.usage = state.usage.filter((u) => now - u.at < 60_000);
    const used = (ms: number) => state.usage.filter((u) => now - u.at < ms).reduce((n, u) => n + u.cost, 0);
    const c = quotaCost(method, rest);
    if (state.config.failQuotaNext > 0 || used(1000) + c > state.config.quotaPerSecond || used(60_000) + c > state.config.quotaPerMinute) {
      if (state.config.failQuotaNext > 0) state.config.failQuotaNext--;
      state.quotaErrors++;
      return reply(
        403,
        { error: { code: 403, message: "Quota exceeded for quota metric 'Total Query Cost' and limit 'Units per minute per user' of service 'gmail.googleapis.com'", errors: [{ reason: "rateLimitExceeded" }], status: "PERMISSION_DENIED" } },
        { "Retry-After": "1" },
      );
    }
    state.usage.push({ at: now, cost: c });

    if (rest === "/profile") return reply(200, { emailAddress: GMAIL_ACCOUNT.email, messagesTotal: state.messages.size, historyId: String(state.historyId) });

    if (rest === "/messages" && method === "GET") {
      const label = url.searchParams.get("labelIds");
      const q = url.searchParams.get("q") ?? "";
      const after = Number(q.match(/after:(\d+)/)?.[1] ?? 0) * 1000;
      // in:/-in: terms, like Gmail's search (in:drafts is the DRAFT label).
      const inLabel = (name: string) => (name === "drafts" ? "DRAFT" : name.toUpperCase());
      const terms = [...q.matchAll(/(-?)in:(\w+)/g)].map((t) => ({ not: t[1] === "-", label: inLabel(t[2]) }));
      // Spam and trash only show when asked for (or named), like Gmail.
      const spamTrash = url.searchParams.get("includeSpamTrash") === "true" || label === "SPAM" || label === "TRASH";
      const size = Math.min(Number(url.searchParams.get("maxResults") ?? 100), state.config.pageSize);
      const all = [...state.messages.values()]
        .filter((m) => (!label || m.labelIds.includes(label)) && m.internalDate > after)
        .filter((m) => terms.every((t) => m.labelIds.includes(t.label) !== t.not))
        .filter(searchMatcher(q))
        .filter((m) => spamTrash || !m.labelIds.some((l) => l === "SPAM" || l === "TRASH"))
        .sort((a, b) => b.internalDate - a.internalDate);
      const start = Number(url.searchParams.get("pageToken") ?? 0);
      const page = all.slice(start, start + size);
      return reply(200, {
        ...(page.length ? { messages: page.map((m) => ({ id: m.id, threadId: m.threadId })) } : {}),
        ...(start + size < all.length ? { nextPageToken: String(start + size) } : {}),
        resultSizeEstimate: all.length,
      });
    }

    let m: RegExpMatchArray | null;
    if ((m = rest.match(/^\/threads\/([^/]+)$/)) && method === "GET") {
      const id = decodeURIComponent(m[1]);
      const messages = [...state.messages.values()].filter((x) => x.threadId === id).sort((a, b) => a.internalDate - b.internalDate);
      if (!messages.length) return reply(404, err(404, "Requested entity was not found."));
      return reply(200, { id, messages: messages.map((x) => resource(x, url.searchParams.get("format") ?? "full")) });
    }
    if ((m = rest.match(/^\/messages\/([^/]+)$/)) && method === "GET") {
      if ((state.config.failIds[m[1]] ?? 0) > 0) {
        state.config.failIds[m[1]]--;
        return reply(500, err(500, "Backend Error"));
      }
      const msg = state.messages.get(m[1]);
      if (!msg) return reply(404, err(404, "Requested entity was not found."));
      return reply(200, resource(msg, url.searchParams.get("format") ?? "full"));
    }
    if ((m = rest.match(/^\/messages\/([^/]+)\/attachments\/([^/]+)$/))) {
      const msg = state.messages.get(m[1]);
      const idx = Number(m[2].split("-").at(-2));
      const a = msg?.attachments?.[idx];
      if (!a) return reply(404, err(404, "Requested entity was not found."));
      return reply(200, { size: a.data.length, data: b64u(a.data) });
    }
    if ((m = rest.match(/^\/messages\/([^/]+)\/(modify|trash|untrash)$/)) && method === "POST") {
      const msg = state.messages.get(m[1]);
      if (!msg) return reply(404, err(404, "Requested entity was not found."));
      const b = await readBody(req);
      if (m[2] === "modify") changeLabels(msg, b.addLabelIds ?? [], b.removeLabelIds ?? []);
      if (m[2] === "trash") changeLabels(msg, ["TRASH"], ["INBOX"]);
      if (m[2] === "untrash") changeLabels(msg, ["INBOX"], ["TRASH"]);
      state.sent.push({ kind: m[2], id: msg.id, add: b.addLabelIds ?? [], remove: b.removeLabelIds ?? [] });
      return reply(200, resource(msg, "minimal"));
    }
    if (rest === "/messages/send" && method === "POST") {
      let raw: string;
      let threadId: string | undefined;
      if (uploadSend) {
        const chunks: Buffer[] = [];
        for await (const c of req) chunks.push(c as Buffer);
        const body = Buffer.concat(chunks).toString("latin1");
        const boundary = (req.headers["content-type"] ?? "").match(/boundary=([^;]+)/)?.[1];
        if (!boundary) return reply(400, err(400, "expected multipart/related"));
        const parts = body.split(`--${boundary}`).slice(1, -1).map((s) => s.replace(/^\r\n/, ""));
        const meta = JSON.parse(parts[0].slice(parts[0].indexOf("\r\n\r\n") + 4).trim() || "{}");
        threadId = meta.threadId;
        raw = parts[1].slice(parts[1].indexOf("\r\n\r\n") + 4).replace(/\r\n$/, "");
      } else {
        const b = await readBody(req);
        raw = Buffer.from(b.raw ?? "", "base64url").toString("latin1");
        threadId = b.threadId;
        if (raw.length > 5 * 1024 * 1024) return reply(413, err(413, "Request payload size exceeds the limit"));
      }
      const mime = parseMime(raw);
      const id = `g-sent-${state.messages.size + 1}`;
      const msg: FakeGmailMessage = {
        id, threadId: threadId ?? `g-thread-sent-${id}`, labelIds: ["SENT"], internalDate: Date.now(),
        from: mime.headers["from"] ?? GMAIL_ACCOUNT.email, to: mime.headers["to"] ?? "", cc: mime.headers["cc"], subject: mime.headers["subject"] ?? "",
        snippet: "", text: leaves(mime).find((l) => l.contentType === "text/plain")?.body.toString("utf8") ?? "", raw,
      };
      state.messages.set(id, msg);
      log({ messagesAdded: [{ message: { id, threadId: msg.threadId, labelIds: msg.labelIds } }] });
      state.sent.push({ kind: "send", id, threadId, upload: uploadSend, raw });
      return reply(200, { id, threadId: msg.threadId, labelIds: msg.labelIds });
    }
    // ----- drafts -----
    /** A draft's message from the request: {message: {raw, threadId}} as JSON, or the upload endpoint's multipart. */
    async function draftBody(): Promise<{ raw: string; threadId?: string }> {
      if (upload) {
        const chunks: Buffer[] = [];
        for await (const c of req) chunks.push(c as Buffer);
        const body = Buffer.concat(chunks).toString("latin1");
        const boundary = (req.headers["content-type"] ?? "").match(/boundary=([^;]+)/)?.[1] ?? "";
        const parts = body.split(`--${boundary}`).slice(1, -1).map((s) => s.replace(/^\r\n/, ""));
        const meta = JSON.parse(parts[0].slice(parts[0].indexOf("\r\n\r\n") + 4).trim() || "{}");
        return { raw: parts[1].slice(parts[1].indexOf("\r\n\r\n") + 4).replace(/\r\n$/, ""), threadId: meta.message?.threadId ?? undefined };
      }
      const b = await readBody(req);
      return { raw: Buffer.from(b.message?.raw ?? "", "base64url").toString("latin1"), threadId: b.message?.threadId };
    }
    if (rest === "/drafts" && method === "GET") {
      return reply(200, { drafts: [...state.drafts].map(([id, mid]) => ({ id, message: { id: mid, threadId: state.messages.get(mid)?.threadId } })) });
    }
    if (rest === "/drafts" && method === "POST") {
      const { raw, threadId } = await draftBody();
      const id = `r-${++draftSeq}`;
      const d = putDraft(id, fromRaw(raw, "", threadId ?? `g-thread-draft-${id}`, []));
      state.sent.push({ kind: "draft-create", id, threadId, upload, raw });
      return reply(200, d);
    }
    if (rest === "/drafts/send" && method === "POST") {
      const b = await readBody(req);
      const mid = state.drafts.get(b.id);
      const msg = mid && state.messages.get(mid);
      if (!msg) return reply(404, err(404, "Requested entity was not found."));
      state.drafts.delete(b.id);
      msg.labelIds = ["SENT"];
      log({ messagesAdded: [{ message: { id: msg.id, threadId: msg.threadId, labelIds: msg.labelIds } }] });
      state.sent.push({ kind: "send", id: msg.id, threadId: msg.threadId, raw: msg.raw, draftId: b.id });
      return reply(200, { id: msg.id, threadId: msg.threadId, labelIds: msg.labelIds });
    }
    if ((m = rest.match(/^\/drafts\/([^/]+)$/))) {
      const id = m[1];
      const mid = state.drafts.get(id);
      if (!mid) return reply(404, err(404, "Requested entity was not found."));
      if (method === "GET") return reply(200, { id, message: resource(state.messages.get(mid)!, url.searchParams.get("format") ?? "full") });
      if (method === "PUT") {
        const { raw, threadId } = await draftBody();
        const d = putDraft(id, fromRaw(raw, "", threadId ?? state.messages.get(mid)!.threadId, []));
        state.sent.push({ kind: "draft-update", id, threadId, upload, raw });
        return reply(200, d);
      }
      if (method === "DELETE") {
        state.messages.delete(mid);
        state.drafts.delete(id);
        state.sent.push({ kind: "draft-delete", id });
        return reply(204, "");
      }
    }
    if (rest === "/history") {
      const start = Number(url.searchParams.get("startHistoryId"));
      if (start < state.minHistory) return reply(404, err(404, "Requested entity was not found."));
      return reply(200, { history: state.history.filter((h) => Number(h.id) > start), historyId: String(state.historyId) });
    }
    if (rest === "/labels" && method === "GET") return reply(200, { labels: [{ id: "INBOX", name: "INBOX", type: "system" }, ...state.labels] });
    if (rest === "/labels" && method === "POST") {
      const b = await readBody(req);
      const label = { id: `Label_${state.labels.length + 1}`, name: b.name, type: "user" };
      state.labels.push(label);
      state.sent.push({ kind: "create-label", ...b });
      return reply(200, label);
    }
    reply(404, err(404, `No fake route for ${method} ${rest}`));
  }

  async function calendar(req: IncomingMessage, url: URL, reply: (s: number, b: unknown) => void) {
    const token = (req.headers.authorization ?? "").replace(/^Bearer /, "");
    if (!state.tokens.has(token)) return reply(401, err(401, "Request had invalid authentication credentials."));
    if (!state.grantCalendar) return reply(403, { error: { code: 403, message: "Request had insufficient authentication scopes.", errors: [{ reason: "insufficientPermissions" }], status: "PERMISSION_DENIED" } });
    const method = req.method ?? "GET";
    if (url.pathname === "/calendar/v3/calendars/primary" && method === "GET") {
      return reply(200, { kind: "calendar#calendar", id: "primary", conferenceProperties: { allowedConferenceSolutionTypes: state.config.conferenceTypes } });
    }
    const rest = url.pathname.slice("/calendar/v3/calendars/primary/events".length);
    const q = url.searchParams;
    if (rest === "" && method === "GET") {
      if (q.get("iCalUID")) {
        // Like Google: the event itself (the series master for recurring ones).
        const items = [...state.events.values()].filter((e) => e.iCalUID === q.get("iCalUID") && !e.recurringEventId);
        return reply(200, { kind: "calendar#events", items });
      }
      const sync = q.get("syncToken");
      if (sync !== null) {
        if (q.get("timeMin") || q.get("timeMax")) return reply(400, err(400, "syncToken can't be combined with timeMin/timeMax"));
        if (Number(sync) < state.minSyncToken) return reply(410, { error: { code: 410, message: "Sync token is no longer valid, a full sync is required.", errors: [{ reason: "fullSyncRequired" }] } });
        const items = [...state.eventChanged.entries()]
          .filter(([, seq]) => seq > Number(sync))
          .map(([id]) => state.events.get(id) ?? { kind: "calendar#event", id, status: "cancelled" })
          .filter((e) => !e.recurrence);
        return reply(200, { kind: "calendar#events", items, nextSyncToken: String(state.eventSeq) });
      }
      const min = q.get("timeMin") ?? "0000", max = q.get("timeMax") ?? "9999";
      const startOf = (e: GEvent) => e.start.dateTime ?? `${e.start.date}T00:00:00Z`;
      const endOf = (e: GEvent) => e.end.dateTime ?? `${e.end.date}T00:00:00Z`;
      const items = [...state.events.values()].filter((e) => !e.recurrence && e.status !== "cancelled" && startOf(e) < max && endOf(e) > min);
      return reply(200, { kind: "calendar#events", items, nextSyncToken: String(state.eventSeq) });
    }
    // Like Google: a conference request is only acted on with conferenceDataVersion=1, else dropped.
    const conference = (b: Json): Json => {
      if (!b.conferenceData) return b;
      const { conferenceData, ...rest } = b;
      if (q.get("conferenceDataVersion") !== "1") return rest;
      const link = `https://meet.google.com/fak-${String(conferenceData.createRequest?.requestId ?? "").slice(-4)}`;
      return { ...rest, hangoutLink: link, conferenceData: { ...conferenceData, entryPoints: [{ entryPointType: "video", uri: link }] } };
    };
    if (rest === "" && method === "POST") {
      const b = await readBody(req);
      const id = `gcal-created-${state.events.size + 1}`;
      const ev = { kind: "calendar#event", id, status: "confirmed", iCalUID: `${id}@google.com`, organizer: me, htmlLink: `https://calendar.google.com/${id}`, ...conference(b) };
      state.events.set(id, ev);
      touchEvent(id);
      state.sent.push({ kind: "create-event", id, body: b });
      return reply(200, ev);
    }
    const one = rest.match(/^\/([^/]+)$/);
    const ev = one ? state.events.get(decodeURIComponent(one[1])) : undefined;
    if (one && !ev) return reply(404, err(404, "Not Found"));
    if (one && method === "GET") return reply(200, ev);
    if (one && method === "PATCH") {
      const b = await readBody(req);
      Object.assign(ev!, conference(b));
      touchEvent(ev!.id);
      // Only your answer changed: an RSVP. Anything else: an edit.
      const rsvp = Object.keys(b).length === 1 && "attendees" in b;
      state.sent.push(rsvp ? { kind: "rsvp", id: ev!.id, sendUpdates: q.get("sendUpdates"), attendees: b.attendees } : { kind: "update-event", id: ev!.id, sendUpdates: q.get("sendUpdates"), body: b });
      return reply(200, ev);
    }
    if (one && method === "DELETE") {
      for (const x of [...state.events.values()]) {
        if (x.id === ev!.id || x.recurringEventId === ev!.id) {
          state.events.delete(x.id);
          touchEvent(x.id);
        }
      }
      state.sent.push({ kind: "delete-event", id: ev!.id, sendUpdates: q.get("sendUpdates") });
      return reply(204, "");
    }
    reply(404, err(404, `No fake calendar route for ${method} ${rest}`));
  }

  let delivered = 0;
  async function control(req: IncomingMessage, url: URL, reply: (s: number, b: unknown) => void) {
    const route = url.pathname.slice("/_test/".length);
    const b = req.method === "POST" ? await readBody(req) : {};
    switch (route) {
      case "deliver": {
        const id = b.id ?? `g-new-${++delivered}`;
        const msg: FakeGmailMessage = {
          id, threadId: b.threadId ?? `g-thread-new-${delivered}`, labelIds: b.labelIds ?? ["INBOX", "UNREAD", "CATEGORY_PERSONAL"], internalDate: b.internalDate ?? Date.now(),
          from: b.from ?? "Kim Park <kim@friends.example>", to: GMAIL_ACCOUNT.email, subject: b.subject ?? `New ${delivered}`, snippet: b.snippet ?? "Just arrived", text: b.text ?? "Just arrived",
          ...(b.attachmentBytes ? { attachments: [{ filename: "big.bin", mimeType: "application/octet-stream", data: Buffer.alloc(b.attachmentBytes, 7) }] } : {}),
        };
        state.messages.set(id, msg);
        log({ messagesAdded: [{ message: { id, threadId: msg.threadId, labelIds: msg.labelIds } }] });
        return reply(200, { id });
      }
      case "draft": {
        // A draft written in Gmail.
        const id = `r-${++draftSeq}`;
        const d = putDraft(id, {
          threadId: b.threadId ?? `g-thread-draft-${id}`, internalDate: Date.now(), from: `${GMAIL_ACCOUNT.name} <${GMAIL_ACCOUNT.email}>`,
          to: b.to ?? "", bcc: b.bcc, subject: b.subject ?? "", snippet: "", html: b.html, text: b.text, inReplyTo: b.inReplyTo,
          attachments: ((b.attachments ?? []) as { name: string; text: string }[]).map((a) => ({ filename: a.name, mimeType: "text/plain", data: Buffer.from(a.text) })),
        });
        return reply(200, d);
      }
      case "draft-update": {
        const mid = state.drafts.get(b.id);
        const old = mid && state.messages.get(mid);
        if (!old) return reply(404, { error: "no such draft" });
        return reply(200, putDraft(b.id, { ...old, ...(b.subject !== undefined ? { subject: b.subject } : {}), ...(b.html !== undefined ? { html: b.html } : {}) }));
      }
      case "draft-remove": {
        const mid = state.drafts.get(b.id);
        if (mid) state.messages.delete(mid);
        state.drafts.delete(b.id);
        return reply(200, { ok: true });
      }
      case "label": {
        const msg = state.messages.get(b.id);
        if (!msg) return reply(404, { error: "no such message" });
        changeLabels(msg, b.add ?? [], b.remove ?? []);
        return reply(200, { labelIds: msg.labelIds });
      }
      case "delete": {
        const msg = state.messages.get(b.id);
        state.messages.delete(b.id);
        if (msg) log({ messagesDeleted: [{ message: { id: msg.id, threadId: msg.threadId } }] });
        return reply(200, {});
      }
      case "gcal-add": {
        const e: GEvent = { kind: "calendar#event", status: "confirmed", organizer: me, ...b };
        state.events.set(e.id, e);
        touchEvent(e.id);
        return reply(200, e);
      }
      case "gcal-cancel": {
        state.events.delete(b.id);
        touchEvent(b.id);
        return reply(200, {});
      }
      case "gcal-expire-sync":
        state.minSyncToken = state.eventSeq + 1;
        return reply(200, {});
      case "gcal-access":
        state.grantCalendar = !!b.granted;
        return reply(200, {});
      case "event": {
        const e = state.events.get(url.searchParams.get("id") ?? "");
        return e ? reply(200, e) : reply(404, { error: "no such event" });
      }
      case "expire-history":
        state.minHistory = state.historyId + 1;
        return reply(200, {});
      case "config":
        Object.assign(state.config, b);
        return reply(200, state.config);
      default: {
        const id = url.searchParams.get("id");
        if (route.startsWith("message") && id) {
          const msg = state.messages.get(id);
          return msg ? reply(200, msg) : reply(404, { error: "no such message" });
        }
        reply(404, { error: `unknown control route ${route}` });
      }
    }
  }

  return {
    origin,
    gmailUrl: `${origin}/gmail/v1/users/me`,
    authUrl: `${origin}/oauth`,
    state,
    async control(route: string, body?: Json): Promise<any> {
      const res = await fetch(`${origin}/_test/${route}`, {
        method: body === undefined ? "GET" : "POST",
        headers: { "Content-Type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      if (!res.ok) throw new Error(`gmail control ${route}: ${res.status} ${await res.text()}`);
      return res.json();
    },
    close: () => new Promise<void>((r) => { server.closeAllConnections(); server.close(() => r()); }),
  };
}
