<script lang="ts">
  // Settings: one page per area, chosen in the sidebar. Most settings apply as soon as
  // they change; the offline cache has a Save button, because widening it resyncs.
  import { onMount } from "svelte";
  import { Bell, CalendarDays, Cloud, HardDrive, KeyRound, Mail, RefreshCw, ShieldCheck, Users } from "@lucide/svelte";
  import { api } from "#lib/api.js";
  import type { Privacy, UpdateChannel } from "#lib/api.js";
  import { keymap } from "#lib/keys.svelte.js";
  import { autostartLabel } from "#lib/platform.js";
  import { mail, type SettingsPage } from "#lib/mail.svelte.js";
  import { updates, updateText } from "#lib/updates.svelte.js";
  import AccountsSection from "./AccountsSection.svelte";
  import ClientIdForm from "./ClientIdForm.svelte";
  import GoogleClientForm from "./GoogleClientForm.svelte";
  import SettingRow from "./SettingRow.svelte";
  import SettingsCard from "./SettingsCard.svelte";
  import SidebarDialog, { type SidebarPage } from "./SidebarDialog.svelte";
  import SignatureEditor from "./SignatureEditor.svelte";
  import SnippetEditor from "./SnippetEditor.svelte";
  import SplitEditor from "./SplitEditor.svelte";
  import SyncSection from "./SyncSection.svelte";

  /** The build has PeetMail's own app for at least one provider; the user's own are an advanced option. */
  const builtinAny = $derived(!!mail.boot?.builtin.microsoft || !!mail.boot?.builtin.google);

  const pages: SidebarPage[] = $derived([
    { id: "accounts", label: "Accounts", icon: Users },
    { id: "mail", label: "Mail", icon: Mail },
    { id: "calendar", label: "Calendar", icon: CalendarDays },
    { id: "notifications", label: "Notifications & startup", icon: Bell },
    { id: "storage", label: "Storage", icon: HardDrive },
    { id: "updates", label: "Updates", icon: RefreshCw },
    { id: "privacy", label: "Privacy", icon: ShieldCheck },
    { id: "sync", label: "Sync", icon: Cloud },
    { id: "apps", label: builtinAny ? "Advanced" : "Sign-in apps", icon: KeyRound, separatorBefore: true },
  ]);

  let page = $state<string>(mail.settingsPage);
  $effect(() => {
    mail.settingsPage = page as SettingsPage;
  });

  onMount(() =>
    keymap.register([
      {
        id: "settings.next",
        keys: "ctrl+PageDown",
        label: "Next section",
        group: "Settings",
        when: () => mail.overlay === "settings",
        run: () => (page = pages[(pages.findIndex((p) => p.id === page) + 1) % pages.length].id),
      },
      {
        id: "settings.prev",
        keys: "ctrl+PageUp",
        label: "Previous section",
        group: "Settings",
        when: () => mail.overlay === "settings",
        run: () => (page = pages[(pages.findIndex((p) => p.id === page) + pages.length - 1) % pages.length].id),
      },
    ]),
  );

  let error = $state<string | null>(null);
  const fail = (err: unknown) => {
    const msg = String(err);
    if (!msg.endsWith("cancelled")) error = msg;
  };

  async function addWithOwnApp(provider: "microsoft" | "google") {
    error = null;
    try {
      await api.signIn(provider, "custom");
      await mail.init();
    } catch (err) {
      fail(err);
    }
  }

  let autostart = $state(false);
  api.getAutostart().then((v) => (autostart = v)).catch(() => {});
  async function setAutostart(on: boolean) {
    try {
      await api.setAutostart(on);
      autostart = on;
    } catch (err) {
      fail(err);
    }
  }

  let privacy = $state<Privacy | null>(null);
  api.privacy().then((p) => (privacy = p)).catch(() => {});
  let copied = $state(false);
  async function setCrashReports(on: boolean) {
    try {
      await api.setCrashReports(on);
      if (privacy) privacy.crashReports = on;
    } catch (err) {
      fail(err);
    }
  }
  async function setUsageData(on: boolean) {
    try {
      await api.setUsageData(on);
      if (privacy) privacy.usageData = on;
    } catch (err) {
      fail(err);
    }
  }
  async function copyInstallId() {
    if (!privacy) return;
    await navigator.clipboard.writeText(privacy.installId);
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }

  let cacheDays = $state(mail.boot?.cacheLimits.cacheDays ?? 90);
  let bodyCacheMb = $state(mail.boot?.cacheLimits.bodyCacheMb ?? 500);
  let cacheSaved = $state(false);
  async function saveCache(e: SubmitEvent) {
    e.preventDefault();
    error = null;
    try {
      await api.setCacheLimits({ cacheDays, bodyCacheMb });
      await mail.init();
      cacheSaved = true;
      setTimeout(() => (cacheSaved = false), 2000);
    } catch (err) {
      fail(err);
    }
  }

  const select = "rounded-md border border-ink-700 bg-ink-950 px-2 py-1 text-sm text-ink-100 outline-none focus:border-accent disabled:opacity-50";
  const button = "rounded-md border border-ink-700 px-3 py-1.5 text-sm text-ink-300 hover:text-ink-100 disabled:opacity-50";
  const number = "w-24 rounded-md border border-ink-700 bg-ink-950 px-2 py-1 text-right text-sm text-ink-100 outline-none focus:border-accent";
</script>

{#snippet heading(title: string, description: string)}
  <header class="mb-6">
    <h2 class="text-lg font-semibold text-ink-100">{title}</h2>
    <p class="mt-1 text-sm text-ink-500">{description}</p>
  </header>
{/snippet}

<SidebarDialog testid="settings" title="Settings" {pages} bind:current={page}>
  <div class="space-y-7">
    {#if page === "accounts"}
      {@render heading("Accounts", "The mailboxes and calendars in PeetMail. They all share one inbox.")}
      <SettingsCard>
        <SettingRow>
          {#snippet block()}<AccountsSection />{/snippet}
        </SettingRow>
      </SettingsCard>
    {:else if page === "mail"}
      {@render heading("Mail", "How your inbox is split, text you reuse, your signature, and sending.")}
      <SettingsCard title="Split inboxes" description="Tabs across the top of the inbox, each one a search.">
        <SettingRow>
          {#snippet block()}<SplitEditor />{/snippet}
        </SettingRow>
      </SettingsCard>
      <SettingsCard title="Snippets" description="Text you type often, inserted from the composer.">
        <SettingRow>
          {#snippet block()}<SnippetEditor />{/snippet}
        </SettingRow>
      </SettingsCard>
      <SettingsCard title="Signatures" description="Added under the messages you write. You can change it in the composer for one message.">
        <SettingRow>
          {#snippet block()}<SignatureEditor />{/snippet}
        </SettingRow>
      </SettingsCard>
      <SettingsCard title="Sending">
        <SettingRow label="Undo send" hint="How long a sent message waits, so z can take it back.">
          <select data-testid="undo-send-select" value={mail.boot?.preferences.undoSendSecs ?? 10} onchange={(e) => mail.setPreference("undoSendSecs", Number(e.currentTarget.value))} class={select}>
            {#each [0, 5, 10, 20, 30] as s (s)}
              <option value={s}>{s ? `${s} seconds` : "Off"}</option>
            {/each}
          </select>
        </SettingRow>
      </SettingsCard>
    {:else if page === "calendar"}
      {@render heading("Calendar", "How weeks are shown.")}
      <SettingsCard>
        <SettingRow label="Week starts on">
          <select data-testid="week-start-select" value={mail.boot?.preferences.weekStartsOn ?? 1} onchange={(e) => mail.setPreference("weekStartsOn", Number(e.currentTarget.value))} class={select}>
            {#each [1, 0, 6, 2, 3, 4, 5] as d (d)}
              <option value={d}>{new Date(2024, 0, 7 + d).toLocaleDateString(undefined, { weekday: "long" })}</option>
            {/each}
          </select>
        </SettingRow>
      </SettingsCard>
    {:else if page === "notifications"}
      {@render heading("Notifications & startup", "What PeetMail tells you about, and whether it keeps running in the background.")}
      <SettingsCard>
        <SettingRow label="New mail notifications" hint="Only while PeetMail isn't the window you're using.">
          <select data-testid="notify-select" value={mail.boot?.preferences.notify ?? "focused"} onchange={(e) => mail.setPreference("notify", e.currentTarget.value as "off" | "focused" | "all")} class={select}>
            <option value="focused">Focused mail</option>
            <option value="all">All mail</option>
            <option value="off">Off</option>
          </select>
        </SettingRow>
      </SettingsCard>
      <SettingsCard>
        <SettingRow label="Keep running in the tray" hint="Closing the window leaves PeetMail in the tray, still syncing and notifying. Ctrl+Q quits.">
          <input type="checkbox" data-testid="close-to-tray" checked={mail.boot?.preferences.closeToTray ?? true} onchange={(e) => mail.setPreference("closeToTray", e.currentTarget.checked)} class="size-4 accent-accent" />
        </SettingRow>
        <SettingRow label={autostartLabel().label} hint={autostartLabel().hint}>
          <input type="checkbox" data-testid="autostart" checked={autostart} onchange={(e) => setAutostart(e.currentTarget.checked)} class="size-4 accent-accent" />
        </SettingRow>
      </SettingsCard>
    {:else if page === "storage"}
      {@render heading("Storage", "How much mail PeetMail keeps on this PC, for instant search and reading offline.")}
      <form onsubmit={saveCache} class="space-y-4">
        <SettingsCard>
          <SettingRow label="Keep mail from the last" hint="Older mail stays on the server. Widening this resyncs.">
            <span class="flex items-center gap-2"><input type="number" min="7" max="3650" bind:value={cacheDays} class={number} /><span class="w-6 text-sm text-ink-500">days</span></span>
          </SettingRow>
          <SettingRow label="Message body cache" hint="The least recently read messages are removed first.">
            <span class="flex items-center gap-2"><input type="number" min="50" max="20000" step="50" bind:value={bodyCacheMb} class={number} /><span class="w-6 text-sm text-ink-500">MB</span></span>
          </SettingRow>
        </SettingsCard>
        <div class="flex items-center justify-end gap-3">
          {#if cacheSaved}<span data-testid="cache-saved" class="text-sm text-ink-500">Saved</span>{/if}
          <button data-testid="cache-save" class="rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-ink-950">Save</button>
        </div>
      </form>
    {:else if page === "updates" && updates.status}
      {@const u = updates.status}
      {@render heading("Updates", "PeetMail downloads new versions in the background and asks before restarting.")}
      <SettingsCard>
        <SettingRow label="Version" hint={updateText(u)}>
          <span class="flex items-center gap-3">
            <span data-testid="app-version" class="text-sm text-ink-300 tabular-nums">{u.currentVersion}</span>
            {#if u.state === "ready"}
              <button data-testid="update-install" type="button" onclick={() => updates.install()} class="rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-ink-950">Restart to update</button>
            {:else}
              <button data-testid="update-check" type="button" disabled={u.state === "disabled" || updates.busy} onclick={() => updates.check()} class={button}>Check now</button>
            {/if}
          </span>
        </SettingRow>
        <SettingRow label="Channel" hint="Canary gets every change as soon as it builds, so it can be rough. Moving back to Stable takes effect at the next stable release.">
          <select data-testid="update-channel" value={u.channel} disabled={u.state === "disabled"} onchange={(e) => updates.setChannel(e.currentTarget.value as UpdateChannel)} class={select}>
            <option value="stable">Stable</option>
            <option value="canary">Canary</option>
          </select>
        </SettingRow>
        <SettingRow label="What's new" hint="Everything that's changed, newest first.">
          <button data-testid="settings-whats-new" type="button" onclick={() => (mail.overlay = "changelog")} class={button}>Show</button>
        </SettingRow>
      </SettingsCard>
      <!-- The status (shown as the Version row's hint) as one element, for tests and screen readers. -->
      <p data-testid="update-status" data-state={u.state} class="sr-only">{updateText(u)}</p>
    {:else if page === "privacy" && privacy}
      {@render heading("Privacy", "What PeetMail sends to help find and fix problems. Mail, addresses, names, search words and sign-in details are never included.")}
      <SettingsCard>
        <SettingRow label="Send crash reports" hint={privacy.configured ? "What went wrong and where in the code, when PeetMail crashes or hits an error." : "This build doesn't send any."}>
          <input type="checkbox" data-testid="crash-reports" checked={privacy.configured && privacy.crashReports} disabled={!privacy.configured} onchange={(e) => setCrashReports(e.currentTarget.checked)} class="size-4 accent-accent disabled:opacity-50" />
        </SettingRow>
        <SettingRow label="Send usage and performance data" hint={privacy.usageConfigured ? "Timings and counts: how long syncs, searches and shortcuts take, and which fail. Turning it back on takes effect at the next start." : "This build doesn't send any."}>
          <input type="checkbox" data-testid="usage-data" checked={privacy.usageConfigured && privacy.usageData} disabled={!privacy.usageConfigured} onchange={(e) => setUsageData(e.currentTarget.checked)} class="size-4 accent-accent disabled:opacity-50" />
        </SettingRow>
        <SettingRow label="Install ID" hint="Identifies this install (not you) in reports. Quote it when you report a problem.">
          <span class="flex items-center gap-3">
            <span data-testid="install-id" class="font-mono text-xs text-ink-300 select-text">{privacy.installId}</span>
            <button data-testid="copy-install-id" type="button" onclick={copyInstallId} class={button}>{copied ? "Copied" : "Copy"}</button>
          </span>
        </SettingRow>
      </SettingsCard>
      {#if !privacy.configured}<span data-testid="crash-reports-off" class="sr-only">This build doesn't send crash reports.</span>{/if}
      {#if !privacy.usageConfigured}<span data-testid="usage-data-off" class="sr-only">This build doesn't send usage data.</span>{/if}
    {:else if page === "sync"}
      {@render heading("Sync", "Your settings, the same on every PC you use.")}
      <SyncSection />
    {:else if page === "apps"}
      <div data-testid="advanced-apps" class="space-y-7">
        {#if builtinAny}
          {@render heading("Advanced: your own sign-in apps", "PeetMail signs in with its own Microsoft and Google apps. If your organisation blocks them, register your own and add accounts with it here. Accounts keep the app they signed in with, so changing this only signs out the accounts that use it.")}
        {:else}
          {@render heading("Sign-in apps", "This build has no sign-in apps built in: it signs in with the Microsoft and Google apps you register.")}
        {/if}
        <SettingsCard title={builtinAny ? "Your own Microsoft app" : "Microsoft app"}>
          <SettingRow>
            {#snippet block()}
              <div class="space-y-3">
                <ClientIdForm onsaved={() => (mail.boot?.accounts.some((a) => a.signedIn) ? null : (mail.overlay = null))} />
                {#if builtinAny && mail.boot?.clientId}
                  <button data-testid="account-add-custom" onclick={() => addWithOwnApp("microsoft")} class={button}>Add Microsoft account with this app</button>
                {/if}
              </div>
            {/snippet}
          </SettingRow>
        </SettingsCard>
        <SettingsCard title={builtinAny ? "Your own Google app" : "Google app"}>
          <SettingRow>
            {#snippet block()}
              <div class="space-y-3">
                <GoogleClientForm />
                {#if builtinAny && mail.boot?.googleClientId}
                  <button data-testid="account-add-google-custom" onclick={() => addWithOwnApp("google")} class={button}>Add Google account with this app</button>
                {/if}
              </div>
            {/snippet}
          </SettingRow>
        </SettingsCard>
      </div>
    {/if}

    {#if error}<p data-testid="settings-error" class="rounded-md border border-red-900/60 bg-red-950/40 px-3 py-2 text-sm text-red-300 select-text">{error}</p>{/if}
  </div>
</SidebarDialog>
