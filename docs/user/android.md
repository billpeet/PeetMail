# PeetMail on Android

PeetMail runs on Android phones from Android 17. It's the same app as on your PC, with your mail on one screen at a time and controls you can reach with your thumb.

## Install

1. On your phone, download `PeetMail_<version>.apk` from the [releases page](https://github.com/billpeet/peetmail/releases).
2. Open it. Android asks to allow installing apps from your browser the first time: allow it, then go back and choose **Install**.

## Signing in

Choose your account type and sign in on the page that opens. With a Microsoft account, PeetMail comes back on its own when you're done. With Gmail or [your own sign-in app](your-own-app.md), switch back to PeetMail after signing in.

The quickest way to set up a phone is [settings sync](sync.md): join with your passphrase and your signatures, snippets, splits and the list of your accounts arrive, then sign in to each account. Sign-ins never sync, so the phone signs in for itself.

Saved sign-ins are encrypted with a key kept in the phone's secure hardware (the Android Keystore). They aren't included in phone backups, so a new phone signs in again.

## Getting around

- **The bottom bar** switches between mail, the calendar, search and settings. The **☰** button at the top lists your mailboxes.
- **Tap** a conversation to read it. The arrows at the top go to the next or previous one, and so does a flick left or right across it.
- **Back** closes whatever is open, then returns to the inbox. Back from the inbox leaves PeetMail running in the background.
- **Pull down** at the top of the list to check for mail.

## Gestures

- **Swipe a conversation right** to archive it, **left** to snooze it. Let go before the colour changes to cancel.
- **Long press** a conversation to select it. Then tap others to add them, and use the bar at the top to archive, delete, mark or snooze them all at once.

## Attachments

- **Tap** an attachment to open it in an app on your phone. Use the download button beside it to save it to **Downloads**.
- The paperclip in a new message attaches files from your phone.
- **Share → PeetMail** from another app (Photos, Files, a web page) starts a new message with the file or link in it.

## Notifications

PeetMail asks to send notifications after you sign in. If you said no, **Settings → Notifications & startup** has a button to open Android's settings and allow them. Tap a notification to open that conversation.

## Checking for mail with PeetMail closed

PeetMail checks for mail every 15 minutes while it's closed, and as soon as you switch back to it. Each check also sends anything waiting to go (including messages scheduled to send later) and brings back snoozed conversations whose time has come, with a reminder.

Change how often under **Settings → Notifications & startup → Check for mail**: every 15 minutes, every hour, or only when PeetMail is open.

Android decides when the checks actually run, and holds them back to save battery, especially when the phone has been lying still for a while. So mail, sends and snooze reminders can arrive later than the setting says. For checks on time, set PeetMail's **App battery usage** to **Unrestricted**: the **Battery** button under the setting opens PeetMail's page in Android's settings.

## Not on Android (yet)

- PeetMail can't update itself on a phone yet: download the new APK from the releases page and open it.
- Starting when you sign in to your computer, the tray, and the window settings are desktop only.
- Keyboard shortcuts work with a hardware keyboard, but most of them are about the desktop layout.
