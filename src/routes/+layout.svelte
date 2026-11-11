<script lang="ts">
  import "../app.css";
  import { keymap } from "#lib/keys.svelte.js";
  import TitleBar from "#lib/components/TitleBar.svelte";
  import { api } from "#lib/api.js";
  import { installErrorReporting } from "#lib/errors.js";
  import { observePage, UiTimings } from "#lib/perf.js";
  import { layout } from "#lib/layout.svelte.js";

  // As early as possible, so errors while the app starts are reported too.
  installErrorReporting((r) => api.reportError(r.name, r.message, r.stack, r.mechanism));

  // Usage timings (Rust drops them when usage data is off or not configured).
  const timings = new UiTimings((events) => api.uiEvents(events));
  observePage(timings);
  keymap.onAction = (id, ms) => timings.add({ kind: "action", binding: id, view: document.body.dataset.view, ms });

  let { children } = $props();

  // CSS keys off this (app.css): dialogs become full-screen pages, safe areas pad the edges.
  $effect(() => {
    document.documentElement.toggleAttribute("data-compact", layout.compact);
  });
</script>

<svelte:window onkeydown={keymap.handle} />

<div class="flex h-full flex-col">
  <!-- A phone has no window to drag, minimise or close. -->
  {#if !layout.compact}<TitleBar />{/if}
  <div class="min-h-0 flex-1">{@render children()}</div>
</div>
