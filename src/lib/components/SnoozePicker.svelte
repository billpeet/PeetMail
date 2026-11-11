<script lang="ts">
  // Snooze (h): pick a preset with ↑/↓ or type a time ("fri 2pm", "in 3 hours"); Enter
  // snoozes. The conversation leaves the inbox and comes back on top when it's due.
  import { onMount } from "svelte";
  import { AlarmClock } from "@lucide/svelte";
  import { formatSendAt, mail } from "#lib/mail.svelte.js";
  import { parseSendTime } from "#lib/quickAdd.js";
  import { snoozePresets } from "#lib/snooze.js";

  let text = $state("");
  let active = $state(0);
  let input: HTMLInputElement | undefined = $state();
  const presets = snoozePresets();
  const typed = $derived(text.trim() ? parseSendTime(text) : null);
  const targets = mail.snoozeTargets;

  function choose(at: Date) {
    mail.snooze(at);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      active = (active + (e.key === "ArrowDown" ? 1 : presets.length - 1)) % presets.length;
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (typed?.ok) choose(typed.at);
      else if (!typed) choose(presets[active].at);
    }
  }

  onMount(() => input?.focus());
</script>

<div
  data-testid="snooze"
  onclick={(e) => e.target === e.currentTarget && (mail.overlay = null)}
  role="presentation"
  class="dialog-backdrop fixed inset-x-0 top-(--chrome-top) bottom-0 z-30 flex justify-center bg-black/50 pt-[15vh]"
>
  <div class="dialog-sheet h-fit w-[460px] overflow-hidden rounded-xl border border-ink-700 bg-ink-900 shadow-2xl">
    <div class="flex items-center gap-3 border-b border-ink-800 px-4">
      <AlarmClock size={18} class="shrink-0 text-accent" />
      <input
        data-testid="snooze-input"
        bind:this={input}
        bind:value={text}
        {onkeydown}
        placeholder="Snooze until… (fri 2pm, in 3 hours)"
        spellcheck="false"
        autocomplete="off"
        class="w-full bg-transparent py-3 text-sm text-ink-100 outline-none select-text"
      />
    </div>
    {#if targets.length > 1}
      <p data-testid="snooze-target" class="truncate border-b border-ink-800 px-4 py-2 text-xs text-ink-500">{targets.length} conversations</p>
    {:else if targets.length}
      <p data-testid="snooze-target" class="truncate border-b border-ink-800 px-4 py-2 text-xs text-ink-500">{targets[0].subject || "(no subject)"}</p>
    {/if}
    {#if typed}
      <div class="px-4 py-3 text-sm">
        {#if typed.ok}
          <p data-testid="snooze-when" class="text-accent">Until {formatSendAt(typed.at.toISOString())}</p>
        {:else}
          <p data-testid="snooze-hint" class="text-ink-500">{typed.reason}</p>
        {/if}
      </div>
    {:else}
      <ul class="py-1">
        {#each presets as p, i (p.id)}
          <li>
            <button
              data-testid="snooze-preset"
              data-id={p.id}
              onclick={() => choose(p.at)}
              onmousemove={() => (active = i)}
              class={["flex w-full justify-between px-4 py-2 text-left text-sm", i === active ? "bg-ink-800 text-ink-100" : "text-ink-300"]}
            >
              <span>{p.label}</span>
              <span class="text-ink-500">{formatSendAt(p.at.toISOString())}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
    <div class="key-hints border-t border-ink-800 px-4 py-2 text-xs text-ink-500">
      <kbd>↑</kbd> <kbd>↓</kbd> choose · <kbd>Enter</kbd> snooze · <kbd>Esc</kbd> cancel · <kbd>g</kbd> <kbd>h</kbd> shows snoozed mail
    </div>
  </div>
</div>
