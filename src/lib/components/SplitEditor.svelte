<script lang="ts">
  // Split inboxes are named queries over the inbox (same language as search). The built-in
  // Inbox tab isn't listed: it holds whatever no split claims.
  import { api, type Split } from "#lib/api.js";
  import { mail } from "#lib/mail.svelte.js";

  let rows = $state<Split[]>(mail.splits.filter((s) => !s.builtin).map(({ id, name, query }) => ({ id, name, query })));
  let saved = $state(false);

  async function save(e: SubmitEvent) {
    e.preventDefault();
    await api.setSplits(rows.filter((r) => r.name.trim()));
    saved = true;
    await mail.refresh();
  }
</script>

<form data-testid="split-editor" onsubmit={save} class="space-y-2">
  {#each rows as row, i (i)}
    <div class="flex gap-2">
      <input
        data-testid="split-name"
        bind:value={row.name}
        oninput={() => (saved = false)}
        placeholder="Name"
        class="w-28 rounded-md border border-ink-700 bg-ink-950 px-2 py-1 text-sm text-ink-100 outline-none select-text focus:border-accent"
      />
      <input
        data-testid="split-query"
        bind:value={row.query}
        oninput={() => (saved = false)}
        placeholder="from:github.com"
        spellcheck="false"
        class="min-w-0 flex-1 rounded-md border border-ink-700 bg-ink-950 px-2 py-1 font-mono text-xs text-ink-100 outline-none select-text focus:border-accent"
      />
      <button type="button" onclick={() => rows.splice(i, 1)} title="Remove" class="px-1 text-ink-500 hover:text-red-300">×</button>
    </div>
  {/each}
  <div class="flex items-center justify-between">
    <button
      type="button"
      data-testid="split-add"
      onclick={() => rows.push({ name: "", query: "" })}
      class="text-xs text-ink-500 hover:text-ink-100">+ Add split</button
    >
    <span class="flex items-center gap-3">
      {#if saved}<span class="text-xs text-ink-500">Saved</span>{/if}
      <button data-testid="split-save" class="rounded-md border border-ink-700 px-2.5 py-1 text-xs text-ink-300 hover:text-ink-100">Save splits</button>
    </span>
  </div>
  <label class="flex items-center gap-2 text-sm text-ink-300">
    <input
      type="checkbox"
      data-testid="hide-empty-splits"
      checked={mail.boot?.preferences.hideEmptySplits ?? false}
      onchange={(e) => mail.setHideEmptySplits(e.currentTarget.checked)}
      class="accent-accent"
    />
    Hide empty splits
  </label>
  <p class="text-xs text-ink-500">
    Mail that matches no split stays in <b>Inbox</b>. A message matching several splits goes to the first one listed.
  </p>
  <p class="text-xs text-ink-500">
    Queries: <code>from:</code> <code>to:</code> <code>subject:</code> <code>is:unread</code> <code>is:flagged</code>
    <code>is:focused</code> <code>is:other</code> <code>is:invite</code> <code>has:attachment</code>, free text, <code>-</code> to negate.
  </p>
</form>
