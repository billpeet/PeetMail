// What the webview runs on, for the few labels and controls that differ (WebView2 on
// Windows, WebKitGTK on Linux, Android System WebView; all put the OS in the user agent).

export type Platform = "windows" | "linux" | "android";

export function platformOf(userAgent: string): Platform {
  // Android's user agent says "Linux; Android 17", so it's checked first.
  if (/Android/.test(userAgent)) return "android";
  return /Linux|X11/.test(userAgent) && !/Windows/.test(userAgent) ? "linux" : "windows";
}

export const platform: Platform = typeof navigator === "undefined" ? "windows" : platformOf(navigator.userAgent);

/** A desktop has a tray, starts at sign-in and quits; a phone does none of those. */
export const isDesktop = (p: Platform = platform) => p !== "android";

/** The Settings label for starting PeetMail at sign-in. */
export function autostartLabel(p: Platform = platform) {
  return p === "linux"
    ? { label: "Start when you log in", hint: "Starts in the tray when you log in to your desktop." }
    : { label: "Start with Windows", hint: "Starts in the tray when you sign in to Windows." };
}
