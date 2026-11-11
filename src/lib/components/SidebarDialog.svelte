<script lang="ts" module>
  import type { Component } from "svelte";

  export type SidebarPage = { id: string; label: string; icon: Component<{ size?: number; class?: string }>; separatorBefore?: boolean };
</script>

<script lang="ts">
  // A dialog with its sections down the left (Settings, Help). Arrow keys move through the
  // sections; Ctrl+PageDown / Ctrl+PageUp work from anywhere in it (registered by the owner).
  import { untrack, type Snippet } from "svelte";
  import { ArrowLeft, X } from "@lucide/svelte";
  import { mail } from "#lib/mail.svelte.js";
  import { layout } from "#lib/layout.svelte.js";

  let {
    testid,
    title,
    pages,
    current = $bindable(),
    children,
  }: { testid: string; title: string; pages: SidebarPage[]; current: string; children: Snippet } = $props();

  let nav: HTMLElement | undefined = $state();

  // On a phone the sections are a page of their own: pick one to open it, back (or Esc,
  // or Android's back) returns to the list. Opened on a particular section, it starts there.
  let browsing = $state(untrack(() => current === pages[0]?.id));
  const label = $derived(pages.find((p) => p.id === current)?.label ?? title);

  function open(id: string) {
    current = id;
    browsing = false;
  }

  /** Esc inside a section goes back to the list first, before the keymap closes the dialog. */
  function backFirst(e: KeyboardEvent) {
    if (e.key !== "Escape" || !layout.compact || browsing || e.defaultPrevented) return;
    e.preventDefault();
    e.stopPropagation();
    browsing = true;
  }

  function navKeys(e: KeyboardEvent) {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const i = pages.findIndex((p) => p.id === current);
    const next = pages[(i + (e.key === "ArrowDown" ? 1 : pages.length - 1)) % pages.length];
    current = next.id;
    nav?.querySelector<HTMLButtonElement>(`[data-page="${next.id}"]`)?.focus();
  }

  // Open with the current section focused, so the arrow keys work straight away.
  $effect(() => {
    nav?.querySelector<HTMLButtonElement>(`[data-page="${current}"]`)?.focus();
  });
</script>

<svelte:window onkeydowncapture={backFirst} />

<div data-testid={testid} data-page={current} data-browsing={layout.compact && browsing} onclick={(e) => e.target === e.currentTarget && (mail.overlay = null)} role="presentation" class="dialog-backdrop fixed inset-x-0 top-(--chrome-top) bottom-0 z-20 grid place-items-center bg-black/60">
  <div class={["dialog-panel flex h-[min(760px,88vh)] w-[min(960px,94vw)] overflow-hidden rounded-xl border border-ink-700 bg-ink-900 shadow-2xl", layout.compact && "flex-col"]}>
    {#if layout.compact}
      <header class="flex h-14 shrink-0 items-center gap-1 border-b border-ink-800 px-2">
        {#if !browsing}
          <button data-testid="{testid}-back" aria-label="Back to {title}" onclick={() => (browsing = true)} class="icon-btn size-11"><ArrowLeft size={20} /></button>
        {/if}
        <h2 class="min-w-0 flex-1 truncate px-2 text-base font-semibold text-ink-100">{browsing ? title : label}</h2>
        <button data-testid="{testid}-close" aria-label="Close {title}" onclick={() => (mail.overlay = null)} class="icon-btn size-11"><X size={20} /></button>
      </header>
    {/if}
    <nav bind:this={nav} aria-label={title} class={["flex shrink-0 flex-col bg-ink-950/50 p-3", layout.compact ? (browsing ? "min-h-0 flex-1 overflow-y-auto" : "hidden") : "w-52 border-r border-ink-800"]}>
      {#if !layout.compact}<h2 class="px-2 pt-1 pb-4 text-sm font-semibold text-ink-100">{title}</h2>{/if}
      <div role="tablist" aria-orientation="vertical" tabindex="-1" onkeydown={navKeys} class="flex flex-col">
      {#each pages as p (p.id)}
        {#if p.separatorBefore}<div class="mx-2 my-2 border-t border-ink-800"></div>{/if}
        <button
          data-testid="{testid}-nav-{p.id}"
          data-page={p.id}
          role="tab"
          aria-selected={current === p.id}
          onclick={() => open(p.id)}
          class={[
            "flex items-center gap-2.5 rounded-md px-2 text-left text-sm outline-none focus-visible:ring-1 focus-visible:ring-accent",
            layout.compact ? "min-h-12 text-[15px]" : "py-1.5",
            current === p.id ? "bg-ink-800 text-ink-100" : "text-ink-500 hover:bg-ink-900 hover:text-ink-100",
          ]}
        >
          <p.icon size={15} class={current === p.id ? "text-accent" : ""} />
          {p.label}
        </button>
      {/each}
      </div>
      {#if !layout.compact}<p class="mt-auto px-2 pt-3 text-[11px] leading-relaxed text-ink-500"><kbd>Ctrl</kbd> <kbd>PgUp</kbd>/<kbd>PgDn</kbd> switch · <kbd>Esc</kbd> close</p>{/if}
    </nav>
    <div class={["min-w-0 flex-1 overflow-y-auto", layout.compact ? (browsing ? "hidden" : "min-h-0 px-4 py-5") : "px-8 py-7"]}>
      {@render children()}
    </div>
  </div>
</div>
