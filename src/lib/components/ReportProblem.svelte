<script lang="ts" module>
  // The draft survives switching Help pages (and closing Help) until it's sent.
  let draft = $state({ message: "", email: "", diagnostics: true });
</script>

<script lang="ts">
  // Help → Report a problem: feedback to the developers (Sentry user feedback, with the
  // diagnostics zip if wanted), an email instead, or just the diagnostics zip.
  import { onMount } from "svelte";
  import { api, type FeedbackOptions } from "#lib/api.js";
  import { mail } from "#lib/mail.svelte.js";
  import { updates } from "#lib/updates.svelte.js";
  import { isDesktop } from "#lib/platform.js";
  import SettingRow from "./SettingRow.svelte";
  import SettingsCard from "./SettingsCard.svelte";

  let options = $state<FeedbackOptions | null>(null);
  let installId = $state("");
  let sending = $state(false);
  let status = $state<{ kind: "sent" | "error"; text: string } | null>(null);
  let exporting = $state(false);
  let exported = $state<{ kind: "saved" | "error"; text: string } | null>(null);

  onMount(async () => {
    [options, installId] = await Promise.all([api.feedbackOptions(), api.privacy().then((p) => p.installId)]);
  });

  const version = $derived(updates.status?.currentVersion ?? "");

  async function send(e: SubmitEvent) {
    e.preventDefault();
    if (!draft.message.trim() || sending) return;
    sending = true;
    status = null;
    try {
      await api.sendFeedback(draft.message, draft.email.trim() || null, draft.diagnostics);
      draft.message = "";
      status = { kind: "sent", text: "Sent. Thank you." };
    } catch (err) {
      status = { kind: "error", text: String(err) };
    } finally {
      sending = false;
    }
  }

  /** Writes it as an email instead: in PeetMail if an account is signed in, else the default mail app. */
  function email() {
    if (!options?.email) return;
    const subject = "PeetMail feedback";
    const body = `${draft.message.trim()}\n\n--\nPeetMail ${version}, install ID ${installId}\n`;
    if (mail.account) {
      mail.overlay = null;
      mail.compose({ to: options.email, subject, body });
    } else {
      api.openExternal(`mailto:${options.email}?subject=${encodeURIComponent(subject)}&body=${encodeURIComponent(body)}`);
    }
  }

  async function exportDiagnostics() {
    exporting = true;
    exported = null;
    try {
      const path = await api.exportDiagnostics();
      // Android hands the zip to the share sheet instead of saving it.
      exported = { kind: "saved", text: isDesktop() ? `Saved to ${path}` : `Shared ${path.split("/").pop()}` };
    } catch (err) {
      exported = { kind: "error", text: String(err) };
    } finally {
      exporting = false;
    }
  }

  const button = "rounded-md border border-ink-700 px-3 py-1.5 text-sm text-ink-300 hover:text-ink-100 disabled:opacity-50";
  const field = "w-full rounded-md border border-ink-700 bg-ink-950 px-2.5 py-1.5 text-sm text-ink-100 outline-none placeholder:text-ink-600 focus:border-accent";
</script>

<header class="mb-6">
  <h2 class="text-lg font-semibold text-ink-100">Report a problem</h2>
  <p class="mt-1 text-sm text-ink-500">Something broken, confusing or missing? Say what happened and what you expected.</p>
</header>

<div class="space-y-8" data-testid="report">
  {#if options?.sentry}
    <SettingsCard title="Send feedback" description="Goes straight to the developers. What you type is sent as written, so leave out anything private.">
      <SettingRow>
        {#snippet block()}
          <form class="space-y-3" onsubmit={send}>
            <textarea data-testid="feedback-message" bind:value={draft.message} onkeydown={(e) => e.ctrlKey && e.key === "Enter" && e.currentTarget.form?.requestSubmit()} rows="6" placeholder="What happened? What did you expect?" class={[field, "resize-y"]}></textarea>
            <input data-testid="feedback-contact" type="email" bind:value={draft.email} placeholder="Your email, if you'd like a reply (optional)" class={field} />
            <label class="flex items-center gap-2 text-sm text-ink-300">
              <input data-testid="feedback-diagnostics" type="checkbox" bind:checked={draft.diagnostics} />
              Include diagnostics (versions, settings and scrubbed logs; no mail)
            </label>
            <div class="flex items-center gap-3">
              <button data-testid="feedback-send" type="submit" disabled={sending || !draft.message.trim()} class="rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-ink-950 disabled:opacity-50">
                {sending ? "Sending…" : "Send"} <kbd class="ml-1 opacity-70">Ctrl+Enter</kbd>
              </button>
              {#if status}
                <span data-testid="feedback-status" data-kind={status.kind} class={["text-sm", status.kind === "sent" ? "text-emerald-400" : "text-red-400"]}>{status.text}</span>
              {/if}
            </div>
          </form>
        {/snippet}
      </SettingRow>
    </SettingsCard>
  {/if}

  {#if options?.email}
    <SettingsCard title={options.sentry ? "Prefer email?" : "Email the developers"}>
      <SettingRow label={options.email} hint="Opens a new message with your version and install ID filled in. Attach the diagnostics file below if it helps.">
        <button data-testid="feedback-email" class={button} onclick={email}>Write an email</button>
      </SettingRow>
    </SettingsCard>
  {/if}

  {#if options && !options.sentry && !options.email}
    <p data-testid="report-fallback" class="text-sm text-ink-400">This copy of PeetMail can't send feedback itself. Export the diagnostics below and send them, with what happened, to whoever gave you PeetMail.</p>
  {/if}

  <SettingsCard title="Diagnostics" description="A zip of versions, settings, the database's health and recent logs, with addresses, names and tokens removed. No mail, subjects or contacts.">
    <SettingRow label="Export diagnostics" hint={isDesktop() ? "Saves the zip to Downloads, to attach to an email or a GitHub issue." : "Makes the zip and opens the share sheet, to send it by email or attach it to a GitHub issue."}>
      <button data-testid="diagnostics-export" class={button} disabled={exporting} onclick={exportDiagnostics}>{exporting ? "Exporting…" : "Export"}</button>
    </SettingRow>
    {#if exported}
      <div data-testid="diagnostics-status" data-kind={exported.kind} class={["pb-3 text-xs break-all", exported.kind === "saved" ? "text-ink-400" : "text-red-400"]}>{exported.text}</div>
    {/if}
    <SettingRow label="Install ID" hint="Identifies this install's reports without saying who you are.">
      <span data-testid="report-install-id" class="font-mono text-xs text-ink-300 select-text">{installId}</span>
    </SettingRow>
  </SettingsCard>
</div>
