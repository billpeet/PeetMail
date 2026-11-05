# Troubleshooting

## Signing in

- **"Need admin approval"** or an `AADSTS` error mentioning consent: your organisation only allows apps an administrator has approved. Ask your IT team to approve PeetMail, or [use your own sign-in app](your-own-app.md).
- **"Google hasn't verified this app"**: expected. Choose **Advanced → Go to PeetMail**.
- **"Access blocked"** from Google: your Google Workspace administrator blocks unapproved apps. Ask them, or use your own Google app.
- **The browser shows an error and never comes back to PeetMail**: press **Esc** in PeetMail to cancel and try again.
- **An account shows "signed out"** or keeps failing: **Settings → Accounts → Sign in again** (or **Reconnect**). It keeps the account's mail and settings.

## Mail and calendar

- **"Sync error"** in the bottom bar: hover over it for the reason. **Ctrl+R** syncs again. A lost connection fixes itself once you're back online; anything you did offline goes out then.
- **Older mail is missing**: PeetMail keeps a set number of days on your PC (90 to start with). Search covers that; press **Ctrl+Enter** in a search to look on the server as well, or widen the window under **Settings → Storage**.
- **No notifications**: PeetMail only notifies while it isn't the window you're using, and Do not disturb (Windows' or your Linux desktop's) hides them. Check **Settings → Notifications & startup**.
- **Pictures in a message don't show**: they're blocked until you press **i**.

## PeetMail itself

- **It says it can't start because its data is from a newer version**: you've gone back to an older PeetMail. Install the latest from the [releases page](https://github.com/billpeet/peetmail/releases).
- **"Windows protected your PC"** when installing: choose **More info → Run anyway**. The installer isn't signed yet.
- **An update won't install**: **Settings → Updates → Check now** shows what went wrong. Installing the latest from the releases page over the top always works and keeps your data.

## Linux

- **Nothing happens when you double-click the AppImage**: it isn't marked executable. Right-click → **Properties → Allow executing file as program**, or run `chmod +x PeetMail_*.AppImage`.
- **"AppImages require FUSE to run"** (or an error about `libfuse.so.2`): install FUSE 2. On Ubuntu 24.04 and later that's `sudo apt install libfuse2t64`, on Ubuntu 22.04 and Debian `sudo apt install libfuse2`, on Fedora `sudo dnf install fuse-libs`, on Arch `sudo pacman -S fuse2`. Or start it without FUSE: `APPIMAGE_EXTRACT_AND_RUN=1 ./PeetMail_*.AppImage`.
- **"error while loading shared libraries"** (for example `libfontconfig.so.1` or `libGLESv2.so.2`): PeetMail brings most of what it needs, but takes fonts and graphics drivers from your system, as every app does. A normal desktop install has them; on a minimal one, install GTK 3 and Mesa (on Debian and Ubuntu: `sudo apt install libgtk-3-0 libgl1 libegl1 libgles2 libgbm1`).
- **The window opens but stays blank or flickers**: usually the graphics driver (often NVIDIA, or a virtual machine). Start PeetMail with `WEBKIT_DISABLE_DMABUF_RENDERER=1 ./PeetMail_*.AppImage`; if that fixes it, put the same variable in the launcher you use.
- **There's no tray icon**: your desktop doesn't show tray icons; see [Getting started](getting-started.md). Open PeetMail again to bring the window back, or turn off **Keep running in the tray** under **Settings → Notifications & startup**.
- **Your keyring asks for a password when PeetMail starts**: PeetMail keeps the key to your saved sign-ins in your desktop's keyring, and the keyring was locked (common with automatic login). Unlock it, or give the keyring your login password so it unlocks when you log in. If you dismiss it, PeetMail asks again in a few minutes, or straight away when you sign in to an account.
- **"PeetMail can't update itself from this folder"**: the AppImage has to be somewhere you can write to; move it into your home folder and run it from there.

## Warnings at the top of the window

PeetMail checks a few things outside itself when it starts, and every few hours after. Each warning says what's wrong; **×** hides it until PeetMail next starts.

- **Your disk is almost full**: PeetMail needs room to save new mail. Free up space (Windows' **Settings → System → Storage** shows what's using it), or keep fewer days of mail under PeetMail's **Settings → Storage**.
- **Your PC's clock is … slow / fast**: Microsoft and Google turn sign-ins away when the clock is more than a few minutes off, and meeting times look wrong. Turn on **Settings → Time & language → Date & time → Set time automatically** in Windows, then press **Ctrl+R** to sync.
- **PeetMail can't save to its data folder**: something is stopping PeetMail writing to `%APPDATA%\com.peetsoftware.peetmail` (on Linux, `~/.local/share/com.peetsoftware.peetmail`). Antivirus with ransomware protection ("Controlled folder access") is the usual cause: allow PeetMail in it. Otherwise check the folder isn't read-only.
- **Microsoft Edge WebView2 is out of date** (Windows only): PeetMail draws its window with WebView2, which normally updates itself. Run Windows Update, or install the **Evergreen** WebView2 Runtime from Microsoft's website. Your IT team may be holding it back.
- **PeetMail's mail cache may be damaged**: a check after an update found a problem in PeetMail's local copy of your mail. Your mail is safe on the server. First send a report (below) so it can be looked into. To start afresh: quit PeetMail (right-click the tray icon → **Quit PeetMail**), open `%APPDATA%\com.peetsoftware.peetmail` (on Linux, `~/.local/share/com.peetsoftware.peetmail`), move every file whose name starts with `peetmail.db` into a new folder there (say, `old`), and start PeetMail again. You'll sign in again and your mail downloads; settings start from scratch. Keep the `old` folder until you're sure: it has a copy from before the update (`peetmail.db.v<number>.bak`).

## Reporting a problem

Open **Help → Report a problem** (or **Report a problem** in the command palette, **Ctrl+K**). Say what happened and what you expected, and press **Send**. It goes straight to the developers, with your version and install ID; add your email if you'd like a reply. Leave **Include diagnostics** ticked unless you'd rather not: it adds the file described below.

Prefer email? **Write an email** on the same page starts one, addressed to the developers, with your version and install ID filled in.

### Diagnostics

**Export** under Diagnostics saves `PeetMail-diagnostics-….zip` to your Downloads folder. It has PeetMail's and your system's versions (on Linux, also which desktop you use and whether PeetMail runs as an AppImage), your settings (not the text of splits, snippets or signatures, and no passwords or tokens), how big the database is and whether it's healthy, which kinds of account you use (not their addresses), what's waiting to be sent, and PeetMail's recent logs with addresses, names and IDs removed. It has no mail, subjects or contacts. Open it and look if you like; it's plain text.

PeetMail's own log is in `%LOCALAPPDATA%\com.peetsoftware.peetmail\logs` on Windows and `~/.local/share/com.peetsoftware.peetmail/logs` on Linux, if you're asked for it.
