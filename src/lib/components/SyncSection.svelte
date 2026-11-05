<script lang="ts">
  // Settings → Sync (docs/user/sync.md): turning settings sync on (connect, then join with the
  // passphrase or create a sync account), and once on, its status, linked accounts, devices,
  // passphrase and turning it off.
  import { settingsSyncApi, type SyncConnected, type SyncDevice, type SyncIdentity } from "#lib/api.js";
  import { mail } from "#lib/mail.svelte.js";
  import { settingsSync } from "#lib/settingsSync.svelte.js";
  import SettingRow from "./SettingRow.svelte";
  import SettingsCard from "./SettingsCard.svelte";

  const MIN_PASSPHRASE = 10;

  const status = $derived(settingsSync.status);
  const pending = $derived(status?.pending ?? null);

  let busy = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);

  async function act(fn: () => Promise<unknown>) {
    busy = true;
    error = null;
    notice = null;
    try {
      await fn();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  // ---------- turning it on ----------

  let server = $state("");
  let connected = $state<SyncConnected | null>(null);
  let secret = $state("");
  let accountId = $state<number | null>(null);
  let passphrase = $state("");
  let confirm = $state("");
  let recoveryKey = $state<string | null>(null);

  function connect(e: SubmitEvent) {
    e.preventDefault();
    act(async () => {
      connected = await settingsSyncApi.connect(server);
      accountId = connected.accounts[0]?.id ?? null;
    });
  }

  function join(e: SubmitEvent) {
    e.preventDefault();
    act(async () => {
      settingsSync.status = await settingsSyncApi.join(secret);
      secret = "";
      connected = null;
    });
  }

  function create(e: SubmitEvent) {
    e.preventDefault();
    if (passphrase.length < MIN_PASSPHRASE) return void (error = `Use a passphrase of at least ${MIN_PASSPHRASE} characters.`);
    if (passphrase !== confirm) return void (error = "The two passphrases don't match.");
    if (accountId === null) return;
    const id = accountId;
    act(async () => {
      recoveryKey = (await settingsSyncApi.create(id, passphrase)).recoveryKey;
      passphrase = confirm = "";
      connected = null;
    });
  }

  function cancel() {
    act(async () => {
      await settingsSyncApi.cancel();
      connected = null;
      secret = passphrase = confirm = "";
    });
  }

  // ---------- once on ----------

  let devices = $state<SyncDevice[]>([]);
  let identities = $state<SyncIdentity[]>([]);
  let loadedFor = "";
  $effect(() => {
    // Load the lists when sync is on (and again if it's turned on for another server).
    const key = status?.state !== "off" && status?.server ? status.server : "";
    if (key && key !== loadedFor && !recoveryKey) {
      loadedFor = key;
      loadLists();
    }
    if (!key) loadedFor = "";
  });

  async function loadLists() {
    try {
      [devices, identities] = await Promise.all([settingsSyncApi.devices(), settingsSyncApi.identities()]);
    } catch (e) {
      error = String(e);
    }
  }

  function syncNow() {
    act(async () => {
      await settingsSync.syncNow();
      await loadLists();
    });
  }

  function revoke(d: SyncDevice) {
    act(async () => {
      await settingsSyncApi.revoke(d.id);
      if (!d.current) await loadLists();
    });
  }

  function unlink(i: SyncIdentity) {
    act(async () => {
      await settingsSyncApi.unlink(i.id);
      await loadLists();
    });
  }

  let newPassphrase = $state("");
  function changePassphrase(e: SubmitEvent) {
    e.preventDefault();
    if (newPassphrase.length < MIN_PASSPHRASE) return void (error = `Use a passphrase of at least ${MIN_PASSPHRASE} characters.`);
    act(async () => {
      await settingsSyncApi.changePassphrase(newPassphrase);
      newPassphrase = "";
      notice = "Passphrase changed. Use the new one on your next device.";
    });
  }

  let shownKey = $state<string | null>(null);
  function showRecoveryKey() {
    act(async () => (shownKey = await settingsSyncApi.recoveryKey()));
  }

  let confirmDelete = $state(false);
  function disable() {
    act(async () => {
      await settingsSyncApi.disable();
      devices = identities = [];
      await mail.refreshElsewhere();
    });
  }
  function deleteAccount() {
    act(async () => {
      await settingsSyncApi.deleteAccount();
      confirmDelete = false;
      devices = identities = [];
      await mail.refreshElsewhere();
    });
  }

  const when = (iso: string | null) => (iso ? new Date(iso).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : "");
  const linkText: Record<string, string> = {
    linked: "Linked",
    elsewhere: "Linked to a different sync account",
    unlinked: "Unlinked",
    failed: "Couldn't link yet; trying again later",
    pending: "Linking…",
  };

  const input = "min-w-0 flex-1 rounded-md border border-ink-700 bg-ink-950 px-2 py-1 text-sm text-ink-100 outline-none select-text focus:border-accent";
  const button = "rounded-md border border-ink-700 px-3 py-1.5 text-sm text-ink-300 hover:text-ink-100 disabled:opacity-50";
  const primary = "rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-ink-950 hover:opacity-90 disabled:opacity-50";
</script>

<div class="space-y-7" data-testid="sync-section" data-state={status?.state ?? "loading"}>
  {#if error}
    <p data-testid="sync-error" class="rounded-md border border-red-900/60 bg-red-950/40 px-3 py-2 text-sm text-red-200">{error}</p>
  {/if}
  {#if notice}
    <p data-testid="sync-notice" class="rounded-md border border-ink-700 px-3 py-2 text-sm text-ink-300">{notice}</p>
  {/if}

  {#if recoveryKey}
    <SettingsCard title="Your recovery key">
      <SettingRow>
        {#snippet block()}
          <p class="text-sm text-ink-300">
            If you forget your passphrase, this key is the only other way to turn sync on for a new device. Keep it somewhere safe, like a password
            manager. It won't be shown again here (you can see it later from a device that syncs).
          </p>
          <p data-testid="sync-recovery-key" class="mt-3 rounded-md bg-ink-950 px-3 py-2 font-mono text-sm tracking-wide break-all text-ink-100 select-all">{recoveryKey}</p>
          <div class="mt-3 flex justify-end">
            <button data-testid="sync-recovery-done" class={primary} onclick={() => (recoveryKey = null)}>I've saved it</button>
          </div>
        {/snippet}
      </SettingRow>
    </SettingsCard>
  {:else if !status}
    <p class="text-sm text-ink-500">Loading…</p>
  {:else if status.state === "off" && pending === "join"}
    <SettingsCard title="Unlock your synced settings">
      <SettingRow>
        {#snippet block()}
          <form onsubmit={join} class="space-y-3">
            <p class="text-sm text-ink-300">
              {connected?.accounts[0]?.email ?? "Your account"} already has settings on this server. Enter your sync passphrase, or your recovery key.
            </p>
            <input data-testid="sync-secret" type="password" bind:value={secret} placeholder="Passphrase or recovery key" autocomplete="off" class="{input} w-full" />
            <div class="flex justify-end gap-2">
              <button type="button" data-testid="sync-cancel" class={button} disabled={busy} onclick={cancel}>Cancel</button>
              <button data-testid="sync-join" class={primary} disabled={busy || !secret}>{busy ? "Unlocking…" : "Turn on sync"}</button>
            </div>
          </form>
        {/snippet}
      </SettingRow>
    </SettingsCard>
  {:else if status.state === "off" && pending === "create"}
    <SettingsCard title="Start syncing">
      <SettingRow>
        {#snippet block()}
          <form onsubmit={create} class="space-y-3">
            <p class="text-sm text-ink-300">
              None of your accounts has synced settings on this server yet. Choose the account that signs you in, and a passphrase. The passphrase
              encrypts your settings before they leave this PC: the server can't read them, and nobody can reset it for you.
            </p>
            <label class="flex items-center gap-3 text-sm text-ink-300">
              <span class="w-28 shrink-0">Sign in with</span>
              <select data-testid="sync-account" bind:value={accountId} class={input}>
                {#each connected?.accounts ?? status.accounts.map((a) => ({ id: a.accountId, email: a.email })) as a (a.id)}
                  <option value={a.id}>{a.email}</option>
                {/each}
              </select>
            </label>
            <label class="flex items-center gap-3 text-sm text-ink-300">
              <span class="w-28 shrink-0">Passphrase</span>
              <input data-testid="sync-passphrase" type="password" bind:value={passphrase} autocomplete="new-password" class={input} />
            </label>
            <label class="flex items-center gap-3 text-sm text-ink-300">
              <span class="w-28 shrink-0">Again</span>
              <input data-testid="sync-passphrase-confirm" type="password" bind:value={confirm} autocomplete="new-password" class={input} />
            </label>
            <p class="text-xs text-ink-500">At least {MIN_PASSPHRASE} characters. A few unrelated words make a good one.</p>
            <div class="flex justify-end gap-2">
              <button type="button" data-testid="sync-cancel" class={button} disabled={busy} onclick={cancel}>Cancel</button>
              <button data-testid="sync-create" class={primary} disabled={busy}>{busy ? "Setting up…" : "Turn on sync"}</button>
            </div>
          </form>
        {/snippet}
      </SettingRow>
    </SettingsCard>
  {:else if status.state === "off"}
    {#if status.error}
      <p data-testid="sync-off-reason" class="rounded-md border border-amber-900/60 bg-amber-950/30 px-3 py-2 text-sm text-amber-200">{status.error}</p>
    {/if}
    <SettingsCard title="Sync settings between your devices">
      <SettingRow>
        {#snippet block()}
          <form onsubmit={connect} class="space-y-3">
            <p class="text-sm text-ink-300">
              Your splits, snippets, signatures and preferences, the same on every PC you use. They go through a sync server that you or your organisation
              runs, encrypted first so it can't read them. You sign in with one of your mail accounts.
            </p>
            <div class="flex gap-2">
              <input data-testid="sync-server" bind:value={server} placeholder="https://sync.example.com" spellcheck="false" class={input} />
              <button data-testid="sync-connect" class={primary} disabled={busy || !server.trim()}>{busy ? "Connecting…" : "Connect"}</button>
            </div>
          </form>
        {/snippet}
      </SettingRow>
    </SettingsCard>
  {:else}
    <SettingsCard>
      <SettingRow
        label={status.state === "error" ? "Sync isn't working" : status.state === "syncing" ? "Syncing…" : "Settings are syncing"}
        hint={status.state === "error" ? (status.error ?? "") : `With ${status.server}${status.lastSyncedAt ? ` · last synced ${when(status.lastSyncedAt)}` : ""}`}
      >
        <button data-testid="sync-now" class={button} disabled={busy || status.state === "syncing"} onclick={syncNow}>Sync now</button>
      </SettingRow>
    </SettingsCard>

    <SettingsCard title="Accounts" description="Any linked account can sign a new device in to sync. Accounts on this PC are linked automatically.">
      <SettingRow>
        {#snippet block()}
          <ul class="divide-y divide-ink-800 text-sm">
            {#each identities as i (i.id)}
              <li data-testid="sync-identity" class="flex items-center justify-between gap-3 py-2">
                <span class="min-w-0 truncate text-ink-100">{i.email ?? (i.provider === "google" ? "A Google account" : "A Microsoft account")}</span>
                {#if identities.length > 1}
                  <button data-testid="sync-unlink" class="text-xs text-ink-500 hover:text-red-300 disabled:opacity-50" disabled={busy} onclick={() => unlink(i)}>Unlink</button>
                {/if}
              </li>
            {/each}
          </ul>
          {#each status.accounts.filter((a) => a.state !== "linked") as a (a.accountId)}
            <p data-testid="sync-account-state" data-state={a.state} class="mt-2 text-xs text-ink-500">{a.email}: {linkText[a.state]}</p>
          {/each}
        {/snippet}
      </SettingRow>
    </SettingsCard>

    <SettingsCard title="Devices" description="PCs that sync with this account. Signing one out stops it syncing; its settings stay as they are.">
      <SettingRow>
        {#snippet block()}
          <ul class="divide-y divide-ink-800 text-sm">
            {#each devices as d (d.id)}
              <li data-testid="sync-device" data-current={d.current} class="flex items-center justify-between gap-3 py-2">
                <span class="min-w-0">
                  <span class="text-ink-100">{d.name}</span>{#if d.current}<span class="ml-2 text-xs text-ink-500">This PC</span>{/if}
                  <span class="block text-xs text-ink-500">Last seen {when(d.lastSeenAt)}</span>
                </span>
                <button data-testid="sync-device-revoke" class="text-xs text-ink-500 hover:text-red-300 disabled:opacity-50" disabled={busy} onclick={() => revoke(d)}>Sign out</button>
              </li>
            {/each}
          </ul>
        {/snippet}
      </SettingRow>
    </SettingsCard>

    <SettingsCard title="Passphrase and recovery key">
      <SettingRow>
        {#snippet block()}
          <form onsubmit={changePassphrase} class="flex gap-2">
            <input data-testid="sync-new-passphrase" type="password" bind:value={newPassphrase} placeholder="New passphrase" autocomplete="new-password" class={input} />
            <button data-testid="sync-change-passphrase" class={button} disabled={busy || !newPassphrase}>Change</button>
          </form>
          <div class="mt-3">
            {#if shownKey}
              <p data-testid="sync-shown-recovery-key" class="rounded-md bg-ink-950 px-3 py-2 font-mono text-sm break-all text-ink-100 select-all">{shownKey}</p>
            {:else}
              <button data-testid="sync-show-recovery" class={button} disabled={busy} onclick={showRecoveryKey}>Show recovery key</button>
            {/if}
          </div>
        {/snippet}
      </SettingRow>
    </SettingsCard>

    <SettingsCard>
      <SettingRow label="Turn off sync on this PC" hint="Settings here stay as they are. Your other devices keep syncing.">
        <button data-testid="sync-disable" class={button} disabled={busy} onclick={disable}>Turn off</button>
      </SettingRow>
      <SettingRow label="Delete the sync account" hint="Removes your synced settings from the server and signs every device out of sync. Each device keeps its own settings.">
        {#if confirmDelete}
          <span class="flex gap-2">
            <button class={button} onclick={() => (confirmDelete = false)}>Keep it</button>
            <button data-testid="sync-delete-confirm" class="rounded-md border border-red-800 px-3 py-1.5 text-sm text-red-200 hover:bg-red-950/50" disabled={busy} onclick={deleteAccount}>Delete</button>
          </span>
        {:else}
          <button data-testid="sync-delete" class={button} disabled={busy} onclick={() => (confirmDelete = true)}>Delete…</button>
        {/if}
      </SettingRow>
    </SettingsCard>
  {/if}
</div>
