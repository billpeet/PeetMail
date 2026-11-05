# What's new in PeetMail

Changes you can see, newest first. PeetMail shows the lines you haven't seen after it updates, and the whole list under Help.

<!--
For whoever changes the app: every change a user can notice adds one line under
"## Unreleased" in the same commit. Write what changed for them, not how ("Ctrl+Enter
sends even while address suggestions show", not "fix keydown handler"). One line per
change; Markdown is fine (`code`, **bold**, links). `pnpm version:set <version>` turns
Unreleased into that version's section when cutting a stable release.
-->

## Unreleased

## 0.2.1 — 2026-11-05

- Act on several conversations at once: **x** selects one, **Shift+J** / **Shift+K** select as you move, and **Ctrl+A** selects everything in the view. **e**, **#**, **u**, **s** and **h** then archive, delete, mark read, flag or snooze them all, and one **z** undoes the lot. **Esc** clears the selection. With the mouse, Ctrl+click and Shift+click.
- Undo now always reverses an action on the server too. Before, pressing **z** soon after another change could leave the undone action in place in Outlook or Gmail and drop that other change.
- Search older mail: in a search, **Ctrl+Enter** (or **Search the server for older mail** under the results) asks Microsoft or Gmail for matches from before your mail window. They show among the other results and open, reply and file like any mail, without filling up your PC.
- **Junk**: a new mailbox (**g !**) with the mail your provider caught as spam, including Gmail's Spam, so a real message caught by mistake isn't lost. **!** marks a conversation as spam; in Junk, **!** or **e** moves it back to your inbox. **z** undoes either.
- PeetMail runs on Linux: download the AppImage from the releases page, and it works on Ubuntu, Debian, Fedora, Arch and other mainstream distributions, and updates itself like the Windows version. Saved sign-ins are kept encrypted with your desktop's keyring. See Help → Getting started.

## 0.2.0 — 2026-10-30

- **Signatures**: set one for each account in **Settings → Mail**. It goes under new messages, and under replies and forwards if you choose. Change it for one message in the composer, or take it off with **Ctrl+Shift+S**. If your organisation adds signatures itself (CodeTwo, Exclaimer), tick that option and PeetMail won't add a second one.
- **Esc** in an address field closes the contact suggestions; press it again to close the message.
- Drafts: what you write is saved as you type, and kept in step with your drafts in Outlook and Gmail (the last change wins). **Esc** closes a message and keeps it as a draft, and so does starting another one; nothing is lost if PeetMail closes unexpectedly. New drafts show at the top of your Inbox. A reply stays with its conversation: move to another one mid-reply and it waits there, under the messages, for when you come back (**r** carries on with it). **g d** lists every draft; **Ctrl+Shift+D** throws one away.
- Pressing **f** (or Reply all) while writing a reply now switches to the new message instead of leaving the reply on screen.
- Format your messages: bold, italic, underline, lists, quotes and links, with **Ctrl+B**, **Ctrl+I**, **Ctrl+U**, **Ctrl+K** (in the text), the buttons at the bottom of the message, or Markdown as you type (a dash and a space starts a list, for one). Formatting pasted from documents and web pages is kept.
- Bcc: **Ctrl+Shift+B** or the Bcc button adds blind copies to new messages, replies and forwards. Forwards also keep their Cc now.
- Double-click an attachment to open it in its app. A single click still saves it to Downloads. Files that can run programs (`.exe`, `.js` and the like) aren't opened this way.
- Attach files to new messages, replies and forwards: **Ctrl+Shift+A** or the paperclip picks them, or drag them onto the message, or paste them (screenshots too). Forwards list the original's attachments, and you can remove any you don't want to send on.
- Changes to a recurring meeting and to one of its occurrences (accept the series, then decline one day) reach the server in the order you made them, even when the connection drops in between.
- Deleting an event you've only just created, while it's still being saved, now deletes it, instead of it coming back a moment later.
- In the event editor, room and guest suggestions no longer close under you when you click back into the field straight after removing a room or guest.
- A message whose send hits a brief network hiccup goes out a second later, instead of waiting up to a minute. Quick changes to one message (flag, then unflag) still reach the server in the order you made them when a connection drops.
- Snoozed (**g h**) and Scheduled (**g s**) open as lists in the main window instead of pop-ups. In Snoozed, **e** and **#** archive and delete too.
- The left sidebar lists your mailboxes: Inbox, Done, Sent, Snoozed, Scheduled, All mail and Trash, across all your accounts. **g e**, **g t**, **g a** and **g #** open Done, Sent, All mail and Trash. In Done, **e** puts a conversation back in the inbox; in Trash, **#** restores it.
- Search no longer shows mail from accounts you've signed out of.
- Mail you archive or delete anywhere (Outlook, Gmail, your phone) is now kept in PeetMail too, within your mail window: search finds archived mail and conversations show it. The first sync after updating takes a little longer while it fetches.
- Replies and forwards open under the conversation you're reading, like Outlook, so the earlier messages stay in view while you write. New messages still open on their own.
- PeetMail warns you, with what to do, when your disk is nearly full, your PC's clock is wrong, it can't save to its folder, WebView2 is out of date, or its mail cache looks damaged after an update.
- **Help → Report a problem**: send feedback to the developers from PeetMail (with diagnostics if you like), write an email instead, or save a diagnostics file with no personal details in it.
- **Help** (**F1**): getting started, updates, privacy, troubleshooting and every keyboard shortcut, built into PeetMail.
- Keyboard shortcut lists show arrow keys as arrows (← →) and Page Up/Down as PgUp/PgDn.
- Settings is organised into pages (Accounts, Mail, Calendar, Notifications, Storage, Updates, Privacy) with a sidebar. **Ctrl+PageUp** and **Ctrl+PageDown** move between them, and **Ctrl+,** opens the one you used last.
- After an update, **What's new** shows what changed since you last looked. The full list is in the command palette (What's new) and under Settings → Updates.
- PeetMail has its own icon: a **P** whose bowl is a key with an envelope on it, in the taskbar, Start menu, tray and title bar.
- Sign in with **Microsoft** or **Google** straight away, with no app registration to set up. Using your own sign-in app is still possible under Settings → Advanced.
- PeetMail updates itself. A new version downloads in the background; **Ctrl+Shift+U** (or the footer) restarts into it. Choose Stable or Canary under Settings → Updates.
- Crash reports and usage data help find and fix problems. Mail, addresses, names and search words are never included; turn either off under Settings → Privacy.
- **Ctrl+Enter** sends even while address suggestions are showing.
- Events can repeat: set a schedule when creating an event, and edit or end a series.
- Create and edit events in full (attendees, location, description, online meeting), and delete or cancel them.
- Room suggestions when choosing a location for a Microsoft 365 event.
- Join links are found in an event's location or description too, with a **Join** button on the calendar.
- Quick add and the event editor work from the keyboard: Tab moves through the fields without stopping on the formatting buttons.
