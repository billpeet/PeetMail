<script lang="ts">
  // Command palette over the keymap registry: every action, searchable by name.
  import { onMount } from "svelte";
  import { displayKeys, keymap, type Binding } from "#lib/keys.svelte.js";
  import { mail } from "#lib/mail.svelte.js";

  let query = $state("");
  let active = $state(0);
  let input: HTMLInputElement | undefined = $state();

  // Snapshot what's runnable from the screen the palette was opened over.
  const available = (() => {
    const was = mail.overlay;
    mail.overlay = null;
    const list = keymap.active().filter((b) => !b.id.startsWith("palette.") && b.id !== "app.close");
    mail.overlay = was;
    // One entry per action (arrow-key aliases share labels).
    return list.filter((b, i) => list.findIndex((o) => o.label === b.label) === i);
  })();

  /** Subsequence match; tighter and earlier matches score higher. */
  function score(label: string, q: string): number {
    if (!q) return 1;
    const l = label.toLowerCase();
    let pos = -1;
    let gaps = 0;
    for (const ch of q.toLowerCase()) {
      const next = l.indexOf(ch, pos + 1);
      if (next < 0) return 0;
      gaps += next - pos - 1;
      pos = next;
    }
    return 100 - gaps - (l.startsWith(q.toLowerCase()) ? 0 : 10);
  }

  const results = $derived(
    available
      .map((b) => ({ b, s: score(`${b.label} ${b.group}`, query.trim()) }))
      .filter((r) => r.s > 0)
      .sort((a, z) => z.s - a.s)
      .map((r) => r.b),
  );

  function run(b: Binding | undefined) {
    if (!b) return;
    mail.overlay = null;
    b.run(new KeyboardEvent("keydown"));
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "ArrowDown") active = Math.min(active + 1, results.length - 1);
    else if (e.key === "ArrowUp") active = Math.max(active - 1, 0);
    else if (e.key === "Enter") run(results[active]);
    else return;
    e.preventDefault();
  }

  onMount(() => input?.focus());
</script>

<div data-testid="palette" onclick={(e) => e.target === e.currentTarget && (mail.overlay = null)} role="presentation" class="dialog-backdrop fixed inset-x-0 top-(--chrome-top) bottom-0 z-30 flex justify-center bg-black/50 pt-[15vh]">
  <div class="dialog-sheet h-fit w-[560px] overflow-hidden rounded-xl border border-ink-700 bg-ink-900 shadow-2xl">
    <input
      data-testid="palette-input"
      bind:this={input}
      bind:value={query}
      oninput={() => (active = 0)}
      {onkeydown}
      placeholder="Type a command…"
      spellcheck="false"
      autocomplete="off"
      class="w-full border-b border-ink-800 bg-transparent px-4 py-3 text-sm text-ink-100 outline-none select-text"
    />
    <ul class="max-h-[50vh] overflow-y-auto py-1">
      {#each results as b, i (b.id)}
        <li>
          <button
            data-testid="palette-item"
            onclick={() => run(b)}
            onmousemove={() => (active = i)}
            class={["flex w-full items-center justify-between px-4 py-2 text-left text-sm", i === active ? "bg-ink-800 text-ink-100" : "text-ink-300"]}
          >
            <span>{b.label} <span class="ml-2 text-xs text-ink-500">{b.group}</span></span>
            <span class="flex gap-1">
              {#each displayKeys(b.keys) as step, si}
                {#if si > 0}<span class="text-xs text-ink-500">then</span>{/if}
                {#each step as k}<kbd>{k}</kbd>{/each}
              {/each}
            </span>
          </button>
        </li>
      {:else}
        <li class="px-4 py-3 text-sm text-ink-500">No matching command</li>
      {/each}
    </ul>
  </div>
</div>
