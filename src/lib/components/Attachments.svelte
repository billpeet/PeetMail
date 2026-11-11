<script lang="ts">
  import { api, type Attachment } from "#lib/api.js";
  import { mail } from "#lib/mail.svelte.js";
  import { fileSize } from "#lib/format.js";
  import { layout } from "#lib/layout.svelte.js";
  import { Download } from "@lucide/svelte";

  let { messageId }: { messageId: number } = $props();

  let items = $state<Attachment[] | null>(null);

  $effect(() => {
    const id = messageId;
    items = null;
    api.listAttachments(id).then(
      (a) => id === messageId && (items = a),
      () => id === messageId && (items = []),
    );
  });

  // A click saves to Downloads and a double-click opens. The click waits out the
  // double-click time, so a double-click doesn't save a copy as well.
  const DOUBLE_CLICK_MS = 500; // Windows' default double-click time
  let pendingClick: ReturnType<typeof setTimeout> | undefined;

  function onclick(a: Attachment) {
    clearTimeout(pendingClick);
    pendingClick = setTimeout(() => download(a), DOUBLE_CLICK_MS);
  }

  async function open(a: Attachment) {
    clearTimeout(pendingClick);
    try {
      await api.openAttachment(messageId, a.id, a.name);
      mail.showToast({ text: `Opening ${a.name}` });
    } catch (e) {
      mail.showToast({ text: String(e), error: true });
    }
  }

  async function download(a: Attachment) {
    try {
      const path = await api.downloadAttachment(messageId, a.id, a.name);
      mail.showToast({ text: `Saved ${path.split(/[\\/]/).pop()}${layout.compact ? " to Downloads" : ""}` });
    } catch (e) {
      mail.showToast({ text: String(e), error: true });
    }
  }
</script>

{#if items?.length}
  <div data-testid="attachments" class="flex flex-wrap gap-2 px-2 pb-2">
    {#each items as a (a.id)}
      {#if layout.compact}
        <!-- A phone has no double-click: a tap opens it, the button beside saves it. -->
        <span class="flex items-center rounded-md border border-ink-700 bg-ink-800 text-sm text-ink-300">
          <button data-testid="attachment" onclick={() => open(a)} class="flex min-h-10 items-center gap-2 py-1 pr-1 pl-3">
            <span class="max-w-52 truncate">{a.name}</span>
            <span class="text-xs text-ink-500">{fileSize(a.size)}</span>
          </button>
          <button data-testid="attachment-save" aria-label="Save {a.name} to Downloads" onclick={() => download(a)} class="grid size-10 place-items-center text-ink-500"><Download size={16} /></button>
        </span>
      {:else}
      <button
        data-testid="attachment"
        onclick={() => onclick(a)}
        ondblclick={() => open(a)}
        title="Click to save to Downloads, double-click to open"
        class="flex items-center gap-2 rounded-md border border-ink-700 bg-ink-800 px-2.5 py-1 text-xs text-ink-300 hover:border-ink-500 hover:text-ink-100"
      >
        <span>{a.name}</span>
        <span class="text-ink-500">{fileSize(a.size)}</span>
      </button>
      {/if}
    {/each}
  </div>
{/if}
