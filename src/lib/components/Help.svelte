<script lang="ts">
  // Help: the user docs (docs/user, compiled in so they match this version and work
  // offline), the keyboard shortcuts (from the keymap), the changelog, and a way to report
  // a problem.
  import { onMount } from "svelte";
  import { BookOpen, Cloud, Smartphone, Keyboard, KeyRound, LifeBuoy, MessageSquareWarning, PenLine, RefreshCw, ShieldCheck, Sparkles } from "@lucide/svelte";
  import { api } from "#lib/api.js";
  import { DOC_SLUGS, parseDoc, resolveLink, type Doc, type HelpPage } from "#lib/docs.js";
  import { keymap } from "#lib/keys.svelte.js";
  import { isDesktop } from "#lib/platform.js";
  import { mail } from "#lib/mail.svelte.js";
  import { renderMarkdown } from "#lib/markdown.js";
  import { sectionLabel, whatsNew } from "#lib/whatsnew.svelte.js";
  import ReportProblem from "./ReportProblem.svelte";
  import ShortcutList from "./ShortcutList.svelte";
  import SidebarDialog, { type SidebarPage } from "./SidebarDialog.svelte";

  const files = import.meta.glob("../../../docs/user/*.md", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
  const docs: Record<string, Doc> = Object.fromEntries(
    DOC_SLUGS.map((slug) => [slug, parseDoc(slug, files[`../../../docs/user/${slug}.md`] ?? `# ${slug}`)]),
  );

  const pages: SidebarPage[] = [
    { id: "getting-started", label: docs["getting-started"].title, icon: BookOpen },
    // Only on a phone until the Android app is released (docs/ANDROID.md).
    ...(isDesktop() ? [] : [{ id: "android", label: docs.android.title, icon: Smartphone }]),
    { id: "shortcuts", label: "Keyboard shortcuts", icon: Keyboard },
    { id: "signatures", label: docs.signatures.title, icon: PenLine },
    { id: "sync", label: docs.sync.title, icon: Cloud },
    { id: "updates", label: docs.updates.title, icon: RefreshCw },
    { id: "privacy", label: docs.privacy.title, icon: ShieldCheck },
    { id: "troubleshooting", label: docs.troubleshooting.title, icon: LifeBuoy },
    { id: "report", label: "Report a problem", icon: MessageSquareWarning },
    { id: "whats-new", label: "What's new", icon: Sparkles },
    { id: "your-own-app", label: docs["your-own-app"].title, icon: KeyRound, separatorBefore: true },
  ];

  let page = $state<string>(mail.helpPage);
  $effect(() => {
    mail.helpPage = page as HelpPage;
  });

  let content: HTMLElement | undefined = $state();
  // A new page starts at the top.
  $effect(() => {
    void page;
    content?.closest(".overflow-y-auto")?.scrollTo({ top: 0 });
  });

  /** Links between docs switch pages; web links open in the browser; anything else does nothing. */
  function follow(e: MouseEvent) {
    const a = (e.target as Element | null)?.closest?.("a");
    if (!a) return;
    e.preventDefault();
    const target = resolveLink(a.getAttribute("href") ?? "");
    if (target.kind === "page") page = target.page;
    else if (target.kind === "external") api.openExternal(target.url);
  }

  onMount(() =>
    keymap.register([
      { id: "help.next", keys: "ctrl+PageDown", label: "Next page", group: "Help", when: () => mail.overlay === "docs", run: () => (page = pages[(pages.findIndex((p) => p.id === page) + 1) % pages.length].id) },
      { id: "help.prev", keys: "ctrl+PageUp", label: "Previous page", group: "Help", when: () => mail.overlay === "docs", run: () => (page = pages[(pages.findIndex((p) => p.id === page) + pages.length - 1) % pages.length].id) },
    ]),
  );
</script>

<SidebarDialog testid="help" title="Help" {pages} bind:current={page}>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <article bind:this={content} data-testid="help-content" onclick={follow} role="presentation">
    {#if page === "shortcuts"}
      <header class="mb-6">
        <h2 class="text-lg font-semibold text-ink-100">Keyboard shortcuts</h2>
        <p class="mt-1 text-sm text-ink-500">Everything PeetMail does has a key. Ctrl+K finds any command by name; ? shows this list anywhere.</p>
      </header>
      <ShortcutList />
    {:else if page === "report"}
      <ReportProblem />
    {:else if page === "whats-new"}
      <header class="mb-6">
        <h2 class="text-lg font-semibold text-ink-100">What's new</h2>
        <p class="mt-1 text-sm text-ink-500">Everything that's changed, newest first.</p>
      </header>
      <div class="doc space-y-6">
        {#each whatsNew.sections.filter((s) => s.entries.length) as s (s.title)}
          <section>
            <h3 class="!mt-0">{sectionLabel(s)}</h3>
            <ul>
              {#each s.entries as e (e.id)}
                <li data-testid="help-whats-new-entry">{@html renderMarkdown(e.text, { inline: true })}</li>
              {/each}
            </ul>
          </section>
        {/each}
      </div>
    {:else if docs[page]}
      <header class="mb-2">
        <h2 class="text-lg font-semibold text-ink-100">{docs[page].title}</h2>
      </header>
      <div class="doc">{@html renderMarkdown(docs[page].body)}</div>
    {/if}
  </article>
</SidebarDialog>
