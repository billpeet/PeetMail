# Syncing settings

If you use PeetMail on more than one PC, settings sync keeps your settings the same on all of them. It works through a sync server that you or your organisation runs. Your settings are encrypted on your PC before they're sent, so the server can't read them.

## What syncs

- Split inboxes
- Snippets
- Signatures, including "My organisation adds my signature"
- Hide empty splits, the first day of the week, the undo-send delay and notifications
- Which accounts you use: a PC that doesn't have one yet offers to sign in to it

Some settings stay on each PC, because they depend on that PC: the layout and pane widths, close to tray, starting with Windows, how much mail is kept offline, the update channel, and crash report and usage data choices. Your own sign-in app (Settings → Advanced) doesn't sync yet either.

Your mail and sign-ins don't sync. Your mail is already on Microsoft's or Google's servers, and each PC signs in to your accounts itself.

## Turning it on

You need the address of a sync server. If someone runs one for you, ask them for it. To run your own, see [the server's guide](https://github.com/billpeet/peetmail/blob/master/server/README.md).

On your first PC:

1. Open **Settings → Sync** (**Ctrl+,**).
2. Enter the server's address and choose **Connect**.
3. Choose the account that signs you in, and a passphrase of at least 10 characters. A few unrelated words make a good one.
4. PeetMail shows your **recovery key**. Keep it somewhere safe, like a password manager. If you forget your passphrase, the recovery key is the only other way in.

On each of your other PCs, open **Settings → Sync**, enter the same address and choose **Connect**. PeetMail recognises your account and asks for your passphrase. Your recovery key works there too.

## Your passphrase

Your passphrase encrypts your settings, and nobody can reset it for you: not the server, not whoever runs it. If you lose both your passphrase and your recovery key, turn sync off on every PC and delete the sync account from one of them. Then start again with a new passphrase. Your settings stay on each PC, so nothing is lost.

To change your passphrase, go to **Settings → Sync** on a PC that syncs. Your other PCs keep syncing; use the new passphrase the next time you add one. **Show recovery key** shows the key again.

## Several accounts

Any of your accounts can sign a new PC in. Each PC links its other accounts automatically once sync is on. An account that's already linked to someone else's sync account isn't linked to yours.

The list under **Settings → Sync → Accounts** shows what's linked. **Unlink** removes one; the last account can't be unlinked. If one of your accounts is from work, link a personal one too, so you can still sign in to sync if you leave.

## Your accounts on every PC

When you add an account on one PC, your other PCs hear about it. A message says so, and **Settings → Accounts** lists it under **On your other PCs**. **Sign in** there signs in to it on this PC, with the account already filled in on the sign-in page. **Hide** stops suggesting it on this PC. An account that signs in with your own sign-in app needs that app set up on this PC first (Settings → Advanced).

With sync on, **Sign out** asks which you mean:

- **This PC only**: the account leaves this PC (with its cached mail) and stays on your others. This PC stops suggesting it.
- **Everywhere**: the account leaves every PC that syncs, the next time each one syncs.

Signing in to an account again, anywhere, puts it back on the list.

## When two PCs change the same thing

The most recent change wins. Each setting counts separately: change a snippet on one PC and a signature on another, and both changes stay.

PeetMail syncs a moment after you change something, when you come back to the window, and every minute. **Sync now** in Settings, or **Sync settings now** in the command palette (**Ctrl+K**), syncs straight away.

## Devices and turning it off

**Settings → Sync → Devices** lists the PCs that sync. **Sign out** stops one syncing; it keeps the settings it has. Use it for a PC you no longer have.

**Turn off** stops sync on this PC only. **Delete** removes your settings from the server and stops sync on every PC. Each PC keeps its own copy of your settings either way.

## What the server sees

The server stores your encrypted settings, the address of each account you link (so whoever runs it can tell people apart), and your devices' names, such as your PC's name. It can't see which settings you have or what's in them.
