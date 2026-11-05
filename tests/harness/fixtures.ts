// Deterministic mailbox fixtures for the fake Graph server.
// Dates are relative to `now` so "today" rows always exist; content is seeded so
// screenshots stay stable between runs.

export type Sender = [name: string, address: string];

/** Graph folder ids for the well-known folders the fake knows about. */
export const FOLDER_IDS = {
  inbox: "AAMkAGI2-inbox",
  sentitems: "AAMkAGI2-sent",
  archive: "AAMkAGI2-archive",
  deleteditems: "AAMkAGI2-deleted",
  junkemail: "AAMkAGI2-junk",
} as const;
export type FolderName = keyof typeof FOLDER_IDS;

export type FakeAttachment = {
  id: string;
  name: string;
  contentType: string;
  size: number;
  isInline: boolean;
  contentId?: string;
  contentBytes: string;
};

/** 1x1 PNG, used for inline and remote image fixtures. */
export const PIXEL_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

export type GraphMessage = {
  "@odata.type"?: string;
  id: string;
  parentFolderId: string;
  conversationId: string;
  subject: string;
  from: { emailAddress: { name: string; address: string } };
  toRecipients: { emailAddress: { name: string; address: string } }[];
  ccRecipients: { emailAddress: { name: string; address: string } }[];
  receivedDateTime: string;
  bodyPreview: string;
  isRead: boolean;
  flag: { flagStatus: "flagged" | "notFlagged" | "complete" };
  hasAttachments: boolean;
  importance: string;
  inferenceClassification: string;
  body: { contentType: "html" | "text"; content: string };
  /** Not part of Graph's message payload; served from /attachments. */
  attachments?: FakeAttachment[];
  /** Meeting invites: the calendar event behind the message (served via $expand). */
  eventId?: string;
  meetingMessageType?: string;
  /** eventMessageRequest properties (updated invites). */
  meetingRequestType?: string;
  previousStartDateTime?: { dateTime: string; timeZone: string } | null;
  previousEndDateTime?: { dateTime: string; timeZone: string } | null;
  previousLocation?: { displayName: string } | null;
};

type Variant = "default" | "newsletter" | "hostile" | "quoted" | "plain";

const SENDERS: Sender[] = [
  ["Sam Carter", "sam.carter@contoso.com"],
  ["Priya Nair", "priya@fabrikam.io"],
  ["GitHub", "noreply@github.com"],
  ["Linear", "notifications@linear.app"],
  ["Mum", "mum@outlook.com"],
  ["Jordan Lee", "jordan.lee@contoso.com"],
  ["Stripe", "receipts@stripe.com"],
  ["Hannah Okafor", "hannah@northwind.dev"],
  ["Calendar", "calendar-noreply@contoso.com"],
  ["The Beetroot Weekly", "hello@beetroot.news"],
];

const SUBJECTS = [
  "Q4 planning doc, first pass",
  "Re: Beetroot supplier contract",
  "[peetmail] CI failed on main",
  "Your receipt from Stripe #4471-2210",
  "Lunch Friday?",
  "Design review notes",
  "Invoice overdue: October",
  "Weekend plans",
  "Offsite agenda (draft)",
  "This week in beetroot: 12 recipes",
  "Can you review my PR?",
  "Flight itinerary: SYD to MEL",
  "Quarterly security training reminder",
  "Photos from Saturday",
  "Re: Re: API rate limits",
];

const SNIPPETS = [
  "Had a look at the numbers and I think we can make the timeline work if we drop the second milestone.",
  "Quick one: are you free for 15 minutes this afternoon to go through the open questions?",
  "Attached is the latest version. Main changes are in section 3, everything else is formatting.",
  "Thanks for sending this over. I've left a few comments inline, nothing blocking.",
  "Reminder that this is due by end of week. Let me know if anything is unclear.",
  "Here's the summary from today's call, plus the action items we agreed on.",
];

/** Small seeded PRNG (mulberry32) so fixtures are identical on every run. */
function rng(seed: number): () => number {
  return () => {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const OTHER_SENDERS = new Set(["noreply@github.com", "notifications@linear.app", "receipts@stripe.com", "hello@beetroot.news"]);

const pick = <T,>(r: () => number, list: T[]): T => list[Math.floor(r() * list.length)];
const addr = ([name, address]: Sender) => ({ emailAddress: { name, address } });
const ME: Sender = ["Boris Dragon", "boris@peetmail.test"];

function htmlBody(subject: string, snippet: string, variant: Variant): string {
  switch (variant) {
    case "newsletter":
      return `<table width="100%" bgcolor="#f4efe6" cellpadding="24"><tr><td align="center">
        <h1 style="color:#7a1f3d;font-family:Georgia,serif">The Beetroot Weekly</h1>
        <img src="https://tracker.example/pixel.gif" width="1" height="1" alt="">
        <img src="https://images.example/hero.jpg" alt="Roasted beetroot" width="480">
        <p style="font-size:16px">${snippet}</p>
        <a href="https://beetroot.news/recipes" style="background:#7a1f3d;color:#fff;padding:10px 18px;text-decoration:none">Read the recipes</a>
      </td></tr></table>`;
    case "hostile":
      return `<p>${snippet}</p>
        <script>window.__pwned = true; alert("xss")</script>
        <img src="x" onerror="window.__pwned = true">
        <a href="javascript:window.__pwned=true">click me</a>
        <iframe src="https://evil.example"></iframe>`;
    case "quoted":
      return `<p>${snippet}</p><p>Cheers,<br>Sam</p>
        <blockquote><p>On Tue, Boris wrote:</p><p>Sounds good, let's lock it in.</p></blockquote>`;
    default:
      return `<div style="font-family:Segoe UI,sans-serif"><p>Hi Boris,</p><p>${snippet}</p>
        <p>More detail at <a href="https://example.com/doc/${encodeURIComponent(subject)}">the doc</a>.</p>
        <p>Thanks!</p></div>`;
  }
}

/**
 * Builds the default mailbox: ~120 conversations over ~120 days, some multi-message
 * threads, unread, flagged and attachment variety, plus a few special messages
 * tests can target by id:
 *   msg-hostile     HTML with scripts/handlers that must be stripped
 *   msg-newsletter  remote images that must be blocked
 *   msg-plain       text/plain body
 *   conv-thread-*   conversation with 4 messages
 */
export function defaultMailbox({ now = Date.now(), count = 160, seed = 42 }: { now?: number; count?: number; seed?: number } = {}): GraphMessage[] {
  const r = rng(seed);
  const messages: GraphMessage[] = [];
  const HOUR = 3600_000;

  const special: { id: string; conv: string; sender: Sender; subject: string; variant: Variant; ageH: number; isRead: boolean }[] = [
    { id: "msg-hostile", conv: "conv-hostile", sender: SENDERS[7], subject: "Weird formatting in this one", variant: "hostile", ageH: 0.5, isRead: false },
    { id: "msg-newsletter", conv: "conv-newsletter", sender: SENDERS[9], subject: "This week in beetroot: 12 recipes", variant: "newsletter", ageH: 2, isRead: false },
    { id: "msg-plain", conv: "conv-plain", sender: SENDERS[4], subject: "Weekend plans", variant: "plain", ageH: 5, isRead: true },
  ];
  for (const s of special) {
    messages.push(makeMessage({
      id: s.id, conversationId: s.conv, sender: s.sender, subject: s.subject,
      receivedAt: now - s.ageH * HOUR, isRead: s.isRead, variant: s.variant, snippet: SNIPPETS[0],
    }));
  }

  // One long thread so the reader's collapse/expand logic has something to chew on.
  for (let i = 0; i < 4; i++) {
    messages.push(makeMessage({
      id: `msg-thread-${i}`, conversationId: "conv-thread-1", sender: i % 2 ? SENDERS[0] : SENDERS[5],
      subject: i ? "Re: Offsite agenda (draft)" : "Offsite agenda (draft)",
      receivedAt: now - (30 - i * 6) * HOUR, isRead: i < 3, variant: i === 3 ? "quoted" : "default",
      snippet: SNIPPETS[i % SNIPPETS.length],
    }));
  }

  // Your reply in the offsite thread, so threads show Sent copies.
  messages.push(makeMessage({
    id: "msg-thread-sent", conversationId: "conv-thread-1", sender: ME, to: [SENDERS[0]],
    subject: "Re: Offsite agenda (draft)", receivedAt: now - 9 * HOUR, isRead: true, folder: "sentitems",
    snippet: "Works for me. I'll book the room.",
  }));
  // Mail you sent, so contact suggestions have something to rank.
  for (let i = 0; i < 3; i++) {
    messages.push(makeMessage({
      id: `msg-sent-${i}`, conversationId: `conv-sent-${i}`, sender: ME, to: [SENDERS[1]],
      subject: `Quick question ${i + 1}`, receivedAt: now - (50 + i * 24) * HOUR, isRead: true, folder: "sentitems",
    }));
  }
  // Archived and deleted mail (done elsewhere, before PeetMail), for Done and Trash.
  messages.push(makeMessage({
    id: "msg-archived", conversationId: "conv-archived", sender: SENDERS[2], subject: "Invoice paid, thanks",
    receivedAt: now - 26 * HOUR, isRead: true, folder: "archive", snippet: "Got it, all settled.",
  }));
  messages.push(makeMessage({
    id: "msg-deleted", conversationId: "conv-deleted", sender: SENDERS[3], subject: "Last chance: webinar seats",
    receivedAt: now - 28 * HOUR, isRead: true, folder: "deleteditems", snippet: "Only a few seats left.",
  }));
  // Caught by the junk filter: one real (for Not spam), for the Junk view.
  messages.push(makeMessage({
    id: "msg-junk", conversationId: "conv-junk", sender: SENDERS[5], subject: "Your contract renewal",
    receivedAt: now - 27 * HOUR, isRead: false, folder: "junkemail", snippet: "Caught by the filter, but it's real.",
  }));
  // Inline image (cid:) plus a regular attachment.
  const attach = makeMessage({
    id: "msg-attach", conversationId: "conv-attach", sender: SENDERS[1], subject: "Q4 plan attached",
    receivedAt: now - 3 * HOUR, isRead: true, hasAttachments: true,
  });
  attach.body = { contentType: "html", content: `<p>Chart below, full plan attached.</p><img src="cid:chart@peetmail" alt="chart" width="40">` };
  attach.attachments = [
    { id: "att-chart", name: "chart.png", contentType: "image/png", size: 68, isInline: true, contentId: "<chart@peetmail>", contentBytes: PIXEL_PNG },
    { id: "att-pdf", name: "Q4 plan.pdf", contentType: "application/pdf", size: 18, isInline: false, contentBytes: Buffer.from("%PDF-1.4 fake plan").toString("base64") },
  ];
  messages.push(attach);
  // A meeting invite.
  messages.push({
    ...makeMessage({
      id: "msg-invite", conversationId: "conv-invite", sender: SENDERS[8], subject: "Invitation: Design review (Thu 10am)",
      receivedAt: now - 4 * HOUR, isRead: false,
    }),
    "@odata.type": "#microsoft.graph.eventMessage",
    eventId: "evt-invite",
    meetingMessageType: "meetingRequest",
  });

  // A meeting that moved after you accepted (the reported case): the original request,
  // your acceptance in Sent, then the organiser's update with the new time.
  const utc = (d: Date) => ({ dateTime: d.toISOString().replace(/\.\d{3}Z$/, ".0000000"), timeZone: "UTC" });
  const oldStart = new Date(now + 48 * HOUR);
  oldStart.setHours(14, 0, 0, 0);
  messages.push({
    ...makeMessage({
      id: "msg-update-req1", conversationId: "conv-update", sender: SENDERS[0], subject: "Team health: conflict & commitment",
      receivedAt: now - 6 * 24 * HOUR, isRead: true, snippet: "Pre-work required, about 10 minutes.",
    }),
    "@odata.type": "#microsoft.graph.eventMessageRequest", eventId: "evt-update", meetingMessageType: "meetingRequest",
    meetingRequestType: "newMeetingRequest",
  });
  messages.push({
    ...makeMessage({
      id: "msg-update-accept", conversationId: "conv-update", sender: ME, to: [SENDERS[0]], folder: "sentitems",
      subject: "Accepted: Team health: conflict & commitment", receivedAt: now - 6 * 24 * HOUR + 20 * 60_000, isRead: true, snippet: "",
    }),
    "@odata.type": "#microsoft.graph.eventMessageResponse", meetingMessageType: "meetingAccepted",
  });
  messages.push({
    ...makeMessage({
      id: "msg-update-req2", conversationId: "conv-update", sender: SENDERS[0], subject: "Team health: conflict & commitment",
      receivedAt: now - 4.5 * HOUR, isRead: false, snippet: "The time has changed. Pre-work required, about 10 minutes.",
    }),
    "@odata.type": "#microsoft.graph.eventMessageRequest", eventId: "evt-update", meetingMessageType: "meetingRequest",
    meetingRequestType: "fullUpdate", previousStartDateTime: utc(oldStart), previousEndDateTime: utc(new Date(oldStart.getTime() + 45 * 60_000)),
    previousLocation: { displayName: "Room 1" },
  });

  for (let i = 0; messages.length < count; i++) {
    const sender = pick(r, SENDERS);
    const subject = pick(r, SUBJECTS);
    // Skewed towards recent mail, like a real inbox.
    const ageH = 8 + Math.pow(r(), 2) * 24 * 120;
    messages.push(makeMessage({
      id: `msg-${String(i).padStart(4, "0")}`,
      conversationId: r() < 0.15 && i > 0 ? `conv-${String(i - 1).padStart(4, "0")}` : `conv-${String(i).padStart(4, "0")}`,
      sender, subject, receivedAt: now - ageH * HOUR,
      isRead: r() > 0.25 || ageH > 24 * 14,
      isFlagged: r() < 0.06,
      hasAttachments: r() < 0.12,
      importance: r() < 0.05 ? "high" : "normal",
      variant: sender[0].includes("Weekly") ? "newsletter" : r() < 0.2 ? "quoted" : "default",
      snippet: pick(r, SNIPPETS),
    }));
  }
  return messages;
}

export type MessageSpec = {
  id: string;
  conversationId?: string;
  sender: Sender;
  /** Recipients; defaults to you. */
  to?: Sender[];
  folder?: FolderName;
  subject: string;
  receivedAt: number;
  isRead?: boolean;
  isFlagged?: boolean;
  hasAttachments?: boolean;
  importance?: string;
  variant?: Variant;
  snippet?: string;
};

export function makeMessage({
  id, conversationId, sender, to = [ME], folder = "inbox", subject, receivedAt, isRead = false, isFlagged = false,
  hasAttachments = false, importance = "normal", variant = "default", snippet = SNIPPETS[1],
}: MessageSpec): GraphMessage {
  const plain = variant === "plain";
  return {
    id,
    parentFolderId: FOLDER_IDS[folder],
    conversationId: conversationId ?? `conv-${id}`,
    subject,
    from: addr(sender),
    toRecipients: to.map(addr),
    ccRecipients: [],
    receivedDateTime: new Date(receivedAt).toISOString().replace(/\.\d{3}Z$/, "Z"),
    bodyPreview: snippet.slice(0, 120),
    isRead,
    flag: { flagStatus: isFlagged ? "flagged" : "notFlagged" },
    hasAttachments,
    importance,
    inferenceClassification: OTHER_SENDERS.has(sender[1]) ? "other" : "focused",
    body: plain
      ? { contentType: "text", content: `Hey love,\n\n${snippet}\n\n<not a tag> & see you Sunday.\nMum` }
      : { contentType: "html", content: htmlBody(subject, snippet, variant) },
  };
}

export const ACCOUNT = { displayName: ME[0], mail: ME[1] };
export { ME };
