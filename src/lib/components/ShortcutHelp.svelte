<script lang="ts">
  import { mail } from "#lib/mail.svelte.js";
  import ShortcutList from "./ShortcutList.svelte";
</script>

<div data-testid="shortcut-help" onclick={(e) => e.target === e.currentTarget && (mail.overlay = null)} role="presentation" class="dialog-backdrop fixed inset-x-0 top-(--chrome-top) bottom-0 z-20 grid place-items-center bg-black/60">
  <div class="dialog-panel w-[560px] overflow-y-auto rounded-xl border border-ink-700 bg-ink-900 p-6 shadow-2xl">
    <h2 class="mb-4 text-sm font-semibold text-ink-100">Keyboard shortcuts</h2>
    <ShortcutList />
  </div>
</div>
