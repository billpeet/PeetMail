<script lang="ts">
  // First run (or every account signed out). Builds with built-in apps just offer "Sign in";
  // the user's own app is an advanced option. Builds without them (local and dev builds)
  // need a client ID first, as before.
  import { onMount } from "svelte";
  import { api } from "#lib/api.js";
  import { keymap } from "#lib/keys.svelte.js";
  import { mail } from "#lib/mail.svelte.js";
  import ClientIdForm from "./ClientIdForm.svelte";
  import GoogleClientForm from "./GoogleClientForm.svelte";
  import type { OAuthApp, Provider } from "#lib/api.js";

  let editing = $state(false);
  /** Showing the Google client form. */
  let googleForm = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);

  const builtin = $derived(mail.boot?.builtin ?? { microsoft: false, google: false });
  /** The user's own Microsoft app, if set. */
  const clientId = $derived(mail.boot?.clientId ?? null);
  const googleClientId = $derived(mail.boot?.googleClientId ?? null);
  const showForm = $derived(editing || (!builtin.microsoft && !clientId));
  /** Mailbox whose cache is waiting for a fresh sign-in (e.g. after a client ID change). */
  const returning = $derived(mail.boot?.accounts.find((a) => !a.signedIn) ?? null);

  async function signIn(provider: Provider = "microsoft", app?: OAuthApp) {
    busy = true;
    error = null;
    try {
      // A returning mailbox signs in with the app it used before.
      const chosen = app ?? (provider === "microsoft" && returning?.provider === "microsoft" ? returning.oauthApp : undefined);
      await api.signIn(provider, chosen);
      await mail.init();
    } catch (err) {
      const msg = String(err);
      if (!msg.endsWith("cancelled")) error = msg;
    } finally {
      busy = false;
    }
  }

  function edit() {
    if (busy) api.cancelSignIn();
    error = null;
    editing = true;
  }

  onMount(() =>
    keymap.register([
      { id: "setup.signin", keys: "Enter", label: "Sign in", group: "Setup", when: () => !showForm && !busy && !googleForm, run: () => signIn() },
      { id: "setup.cancel", keys: "Escape", label: "Cancel sign-in", group: "Setup", when: () => !showForm && busy, run: () => api.cancelSignIn() },
      {
        id: "setup.clientid",
        keys: "c",
        label: builtin.microsoft ? "Use your own app" : "Change client ID",
        group: "Setup",
        when: () => !showForm,
        run: edit,
      },
    ]),
  );
</script>

<div data-testid="setup" data-builtin={builtin.microsoft} class="grid h-full place-items-center overflow-y-auto" style:padding-top="var(--safe-top)" style:padding-bottom="var(--safe-bottom)">
  <div class="w-full max-w-[460px] space-y-6 px-6">
    <div>
      <h1 class="text-2xl font-semibold tracking-tight text-ink-100">PeetMail</h1>
      <p class="mt-1 text-sm text-ink-500">Fast, keyboard-first mail for Microsoft 365, Outlook.com and Gmail.</p>
    </div>

    {#if showForm}
      {#if builtin.microsoft}
        <p class="text-sm text-ink-300">
          Your own Entra app, instead of PeetMail's. Only needed if your organisation doesn't allow PeetMail's app.
        </p>
      {/if}
      <ClientIdForm
        autofocus
        oncancel={clientId || builtin.microsoft ? () => (editing = false) : undefined}
        onsaved={() => (editing = false)}
      />
    {:else}
      <div class="space-y-3">
        {#if busy}
          <div data-testid="sign-in-waiting" class="flex w-full items-center justify-between rounded-md bg-ink-800 px-4 py-2.5 text-sm text-ink-300">
            <span>Waiting for the browser…</span>
            <button onclick={() => api.cancelSignIn()} class="flex items-center gap-1.5 text-ink-500 hover:text-ink-100">
              Cancel <kbd>Esc</kbd>
            </button>
          </div>
          {#if !builtin.microsoft}
            <p class="text-xs text-ink-500">
              If the browser shows an error about the application, the client ID is probably wrong. Press <kbd>c</kbd> to change it.
            </p>
          {/if}
        {:else}
          <button
            data-testid="sign-in"
            onclick={() => signIn()}
            class="flex w-full items-center justify-between rounded-md bg-accent px-4 py-2.5 text-sm font-medium text-ink-950"
          >
            <span>{returning ? `Sign in again as ${returning.email}` : "Sign in with Microsoft"}</span>
            <kbd>Enter</kbd>
          </button>
          {#if builtin.microsoft && clientId}
            <button
              data-testid="sign-in-custom"
              onclick={() => signIn("microsoft", "custom")}
              class="flex w-full items-center justify-between rounded-md border border-ink-700 px-4 py-2.5 text-sm text-ink-100 hover:border-accent"
            >
              <span>Sign in with your own app</span>
            </button>
          {/if}
        {/if}

        {#if builtin.microsoft}
          <div class="flex items-center justify-between text-xs text-ink-500">
            <span>Work, school or personal Microsoft account</span>
            <button data-testid="setup-own-app" onclick={edit} class="flex shrink-0 items-center gap-1.5 hover:text-ink-100">
              {clientId ? "Your own app" : "Use your own app"} <kbd>c</kbd>
            </button>
          </div>
        {:else}
          <div class="flex items-center justify-between text-xs text-ink-500">
            <span class="truncate">Client ID <span class="font-mono text-ink-300 select-text">{clientId}</span></span>
            <button data-testid="change-client-id" onclick={edit} class="flex shrink-0 items-center gap-1.5 hover:text-ink-100">Change <kbd>c</kbd></button>
          </div>
        {/if}
      </div>
    {/if}

    {#if !busy}
      <div class="space-y-2 border-t border-ink-800 pt-4">
        {#if googleForm || (!builtin.google && !googleClientId && !clientId && !builtin.microsoft)}
          <p class="text-sm text-ink-300">Gmail: {builtin.google ? "your own Google OAuth client" : "your Google OAuth client"}</p>
          <GoogleClientForm onsaved={() => (googleForm = false)} />
        {:else if builtin.google || googleClientId}
          <button
            data-testid="sign-in-google"
            onclick={() => signIn("google")}
            class="flex w-full items-center justify-between rounded-md border border-ink-700 px-4 py-2.5 text-sm text-ink-100 hover:border-accent"
          >
            <span>Sign in with Google (Gmail)</span>
          </button>
          {#if builtin.google && googleClientId}
            <button data-testid="sign-in-google-custom" onclick={() => signIn("google", "custom")} class="text-xs text-ink-500 hover:text-ink-100">
              Sign in with your own Google app
            </button>
          {/if}
          <button onclick={() => (googleForm = true)} class="block text-xs text-ink-500 hover:text-ink-100">
            {builtin.google ? "Use your own Google app" : "Change Google client"}
          </button>
        {:else}
          <button data-testid="setup-gmail" onclick={() => (googleForm = true)} class="text-xs text-ink-500 hover:text-ink-100">Use Gmail instead…</button>
        {/if}
      </div>
    {/if}

    {#if error}
      <p data-testid="setup-error" class="rounded-md border border-red-900/60 bg-red-950/40 px-3 py-2 text-sm text-red-300 select-text">{error}</p>
    {/if}
  </div>
</div>
