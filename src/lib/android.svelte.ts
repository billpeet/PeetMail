// What Android hands PeetMail from outside (docs/ANDROID.md Stage 4): "Share → PeetMail",
// taps on its notifications, and the notification permission. PeetPlugin.kt keeps what
// arrives until the page asks (`api.takePending`), and fires `peetmail-pending` to say so.
import { api, type NotificationPermission, type Pending, type Shared } from "./api.js";
import { mail } from "./mail.svelte.js";
import { platform } from "./platform.js";

/** What a share puts in a new message. A shared link's page title arrives as the subject;
 *  some apps send the title in the text as well, which isn't repeated. */
export function composeFromShare(s: Shared): { subject: string; body: string } {
  const subject = s.subject?.trim() ?? "";
  let body = s.text?.trim() ?? "";
  if (subject && body.startsWith(subject)) body = body.slice(subject.length).trim();
  return { subject, body };
}

export const android = $state({ notifications: "granted" as NotificationPermission });

async function handle(p: Pending) {
  if (p.shared) {
    if (!mail.account) {
      if (p.shared.files.length) api.removeDraftFiles(p.shared.files.map((f) => f.id)).catch(() => {});
      mail.showToast({ text: "Sign in first, then share to PeetMail again.", error: true });
    } else {
      const { subject, body } = composeFromShare(p.shared);
      // Only failures (an app that shared a file PeetMail can't read): just say so.
      if (subject || body || p.shared.files.length) {
        mail.showMail();
        mail.compose({ subject, body, attachments: p.shared.files });
      }
    }
    for (const f of p.shared.failed) mail.showToast({ text: `Couldn't attach ${f}`, error: true });
  }
  if (p.open) {
    mail.overlay = null;
    mail.showMail();
    await mail.openRecent(p.open);
  }
}

async function takePending() {
  try {
    await handle(await api.takePending());
  } catch (e) {
    mail.showToast({ text: String(e), error: true });
  }
}

const ASKED_KEY = "peetmail.notificationsAsked";

/** Asks for the notification permission the first time it matters: notifications on, and an
 *  account signed in (not at first launch, before PeetMail has anything to say). */
export async function checkNotifications(ask: boolean) {
  if (platform !== "android") return;
  try {
    android.notifications = await api.notificationPermission(ask);
  } catch {
    // Not worth a toast; Settings shows the state when it's known.
  }
}

/** Asks once, ever, when notifications are wanted (an account and notifications on). */
async function askOnce() {
  let asked = false;
  try {
    asked = localStorage.getItem(ASKED_KEY) === "1";
  } catch {}
  const wanted = mail.accounts.length > 0 && (mail.boot?.preferences.notify ?? "focused") !== "off";
  await checkNotifications(wanted && !asked);
  if (wanted && !asked) {
    try {
      localStorage.setItem(ASKED_KEY, "1");
    } catch {}
  }
}

/** Starts listening once startup has loaded the accounts (a share needs one to compose
 *  from); returns the cleanup. Does nothing off Android. */
export function startAndroid(): () => void {
  if (platform !== "android") return () => {};
  const onPending = () => void takePending();
  window.addEventListener("peetmail-pending", onPending);
  const stop = $effect.root(() => {
    let started = false;
    $effect(() => {
      if (!mail.boot || started) return;
      started = true;
      takePending();
    });
    // Signing in the first account is when notifications start to matter.
    $effect(() => {
      if (mail.accounts.length) askOnce();
    });
  });
  return () => {
    stop();
    window.removeEventListener("peetmail-pending", onPending);
  };
}
