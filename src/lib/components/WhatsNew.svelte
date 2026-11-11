<script lang="ts">
  // "What's new": the changelog lines not seen yet (after an update), or the whole changelog
  // (palette, Settings → Updates). Either way, closing it marks every line as seen.
  import { onDestroy, untrack } from "svelte";
  import { mail } from "#lib/mail.svelte.js";
  import { openLinksExternally, renderMarkdown } from "#lib/markdown.js";
  import { sectionLabel, whatsNew } from "#lib/whatsnew.svelte.js";

  let { mode }: { mode: "new" | "all" } = $props();

  // Fixed when the panel opens, so lines don't vanish from it as they're marked seen.
  const sections = untrack(() => (mode === "new" ? whatsNew.fresh : whatsNew.sections.filter((s) => s.entries.length)));
  const isNew = untrack(() => new Set(whatsNew.fresh.flatMap((s) => s.entries.map((e) => e.id))));

  let ok: HTMLButtonElement | undefined = $state();
  $effect(() => ok?.focus());

  onDestroy(() => {
    whatsNew.markAllSeen();
  });
</script>

<div data-testid="whats-new" data-mode={mode} onclick={(e) => e.target === e.currentTarget && (mail.overlay = null)} role="presentation" class="dialog-backdrop fixed inset-x-0 top-(--chrome-top) bottom-0 z-30 grid place-items-center bg-black/60">
  <div class="dialog-panel flex max-h-[80vh] w-[560px] flex-col rounded-xl border border-ink-700 bg-ink-900 shadow-2xl">
    <div class="border-b border-ink-800 px-6 py-4">
      <h2 class="text-sm font-semibold text-ink-100">{mode === "new" ? "What's new in PeetMail" : "What's new"}</h2>
      {#if mode === "new"}<p class="mt-1 text-xs text-ink-500">Since you last looked. The full list is in the command palette: What's new.</p>{/if}
    </div>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="min-h-0 flex-1 space-y-5 overflow-y-auto px-6 py-4" onclick={openLinksExternally} role="presentation">
      {#each sections as s (s.title)}
        <section>
          <h3 class="mb-2 text-xs tracking-wide text-ink-500 uppercase">{sectionLabel(s)}</h3>
          <ul class="space-y-2">
            {#each s.entries as e (e.id)}
              <li data-testid="whats-new-entry" data-new={isNew.has(e.id)} class="flex gap-2 text-sm text-ink-300 [&_a]:text-accent [&_a]:underline [&_code]:rounded [&_code]:bg-ink-800 [&_code]:px-1 [&_strong]:text-ink-100">
                <span class={["mt-1.5 size-1.5 shrink-0 rounded-full", isNew.has(e.id) && mode === "all" ? "bg-accent" : "bg-ink-700"]}></span>
                <span>{@html renderMarkdown(e.text, { inline: true })}</span>
              </li>
            {/each}
          </ul>
        </section>
      {:else}
        <p class="text-sm text-ink-500">Nothing yet.</p>
      {/each}
    </div>
    <div class="flex justify-end border-t border-ink-800 px-6 py-3">
      <button bind:this={ok} data-testid="whats-new-ok" onclick={() => (mail.overlay = null)} class="rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-ink-950">
        Got it <kbd class="ml-1">Esc</kbd>
      </button>
    </div>
  </div>
</div>
