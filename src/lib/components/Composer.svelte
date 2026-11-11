<script lang="ts">
  // New message, reply, reply all, forward. The text is rich text (Tiptap): Ctrl+B/I/U,
  // lists, links (Ctrl+K in the text), and Markdown shortcuts as you type (**bold**,
  // *italic*, "- " and "1. " lists, "> " quotes). It's sent as HTML (sanitized in Rust)
  // plus a plain-text version. Ctrl+Enter sends, Ctrl+Shift+Enter sends later. It's saved
  // as a draft as you type, so Esc just closes it (Drafts, g d, has it); Ctrl+Shift+D
  // throws it away. Files come from Ctrl+Shift+A (a file picker),
  // dropping them on the composer, or pasting; a forward lists the original's attachments,
  // each removable. The account's signature is a block at the end of the text (the
  // Signature node), edited like the rest; Ctrl+Shift+S takes it off or puts it back.
  // `inline`: a reply or forward written in the reader, under the conversation it answers
  // (like Outlook). Otherwise it's a dialog over everything.
  import { onDestroy, onMount } from "svelte";
  import { Editor } from "@tiptap/core";
  import StarterKit from "@tiptap/starter-kit";
  import Placeholder from "@tiptap/extension-placeholder";
  import { Fragment, Slice, type Node as PMNode } from "@tiptap/pm/model";
  import { docToText, textToHtml } from "#lib/richtext.js";
  import { api, type Attachment, type Contact } from "#lib/api.js";
  import { keymap } from "#lib/keys.svelte.js";
  import { mail } from "#lib/mail.svelte.js";
  import { Bold, Italic, Link2, List, ListOrdered, Paperclip, Trash2, Underline, X } from "@lucide/svelte";
  import { attachmentRoom, fileSize } from "#lib/format.js";
  import { latestOnly, STALE } from "#lib/latest.js";
  import { narrow } from "#lib/contacts.js";
  import { parseSendTime } from "#lib/quickAdd.js";
  import { formatSendAt } from "#lib/mail.svelte.js";
  import { expandSnippet, matchSnippets, snippetTrigger, type Snippet } from "#lib/snippets.js";
  import { SIGNATURE_CLASS } from "#lib/signatures.js";
  import { SignatureNode } from "#lib/signatureNode.js";
  import { layout } from "#lib/layout.svelte.js";

  let { inline = false }: { inline?: boolean } = $props();

  const c = mail.composer!;
  let root: HTMLDivElement | undefined = $state();
  let to = $state(c.to);
  let cc = $state(c.cc);
  let bcc = $state(c.bcc);
  let subject = $state(c.subject);
  /** The text as plain text and as the editor's HTML, kept in step by the editor. */
  let body = $state(c.body);
  let html = $state(c.html || textToHtml(c.body));
  let showCc = $state(c.cc.length > 0);
  let showBcc = $state(c.bcc.length > 0);
  let bccInput: HTMLInputElement | undefined = $state();
  let from = $state(c.accountId);

  // ----- saved as you type -----
  const AUTOSAVE_MS = 800;
  let saveTimer: ReturnType<typeof setTimeout> | undefined;
  let primed = false;
  $effect(() => {
    // Everything a draft keeps.
    void [to, cc, bcc, subject, html, from, c.attachments.length, c.droppedAttachments.length];
    if (!primed) {
      primed = true; // opening it isn't a change
      return;
    }
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => mail.autosave(c), AUTOSAVE_MS);
  });
  onDestroy(() => clearTimeout(saveTimer));

  // ----- attachments -----
  let fileInput: HTMLInputElement | undefined = $state();
  /** Files still on their way to Rust (sending waits for them). */
  const attaching = $derived(mail.pendingFiles);
  /** A file is being dragged over the composer. */
  let dragging = $state(false);
  /** The original's attachments, when forwarding. */
  let forwarded = $state<Attachment[]>([]);
  const keptForwarded = $derived(forwarded.filter((a) => !c.droppedAttachments.includes(a.name)));

  if (c.kind === "forward" && c.referenceMessageId !== null) {
    api.listAttachments(c.referenceMessageId).then((a) => (forwarded = a), () => {});
  }

  async function addFiles(files: File[]) {
    if (!files.length) return;
    const tooMuch = attachmentRoom(c.attachments.map((f) => f.size), files.map((f) => f.size));
    if (tooMuch) {
      mail.showToast({ text: tooMuch, error: true });
      return;
    }
    mail.pendingFiles += files.length;
    for (const file of files) {
      try {
        const added = await api.addDraftFile(file);
        // The composer may have been closed or replaced while the bytes went over.
        if (mail.composer === c) c.attachments.push(added);
        else api.removeDraftFiles([added.id]).catch(() => {});
      } catch (e) {
        mail.showToast({ text: `Couldn't attach ${file.name}: ${e}`, error: true });
      } finally {
        mail.pendingFiles--;
      }
    }
  }

  function removeFile(id: number) {
    const i = c.attachments.findIndex((f) => f.id === id);
    if (i < 0) return;
    c.attachments.splice(i, 1);
    api.removeDraftFiles([id]).catch(() => {});
  }

  function pickFiles() {
    fileInput?.click();
  }

  const hasFiles = (e: DragEvent) => !!e.dataTransfer?.types.includes("Files");

  function ondragover(e: DragEvent) {
    if (!hasFiles(e)) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
    dragging = true;
  }

  function ondragleave(e: DragEvent) {
    // Only when leaving the composer itself, not moving between its children.
    if (!(e.currentTarget as Node).contains(e.relatedTarget as Node | null)) dragging = false;
  }

  function ondrop(e: DragEvent) {
    if (!hasFiles(e)) return;
    e.preventDefault();
    dragging = false;
    addFiles([...(e.dataTransfer?.files ?? [])]);
  }

  /** Pasted files become attachments. True if it took the paste. */
  function pasteFiles(e: ClipboardEvent): boolean {
    const files = [...(e.clipboardData?.files ?? [])];
    // Text copied with a picture of itself (Word, Excel) pastes as text.
    if (!files.length || e.clipboardData?.types.includes("text/plain")) return false;
    addFiles(files);
    return true;
  }

  // ----- send later -----
  /** Text of the send-later picker; null while it's closed. */
  let later = $state<string | null>(null);
  let laterInput: HTMLInputElement | undefined = $state();
  const laterParsed = $derived(later?.trim() ? parseSendTime(later) : null);

  function nextWeekday(day: number, hour: number): Date {
    const now = new Date();
    const diff = (day - now.getDay() + 7) % 7 || 7;
    return new Date(now.getFullYear(), now.getMonth(), now.getDate() + diff, hour);
  }
  const presets = $derived.by(() => {
    const now = new Date();
    const list: { label: string; at: Date }[] = [{ label: "In 1 hour", at: new Date(now.getTime() + 3_600_000) }];
    if (now.getHours() < 16) list.push({ label: "This evening", at: new Date(now.getFullYear(), now.getMonth(), now.getDate(), 18) });
    list.push({ label: "Tomorrow morning", at: new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1, 8) });
    list.push({ label: "Monday morning", at: nextWeekday(1, 8) });
    return list;
  });

  function openLater() {
    later = "";
    queueMicrotask(() => laterInput?.focus());
  }

  function closeLater() {
    later = null;
    editor?.commands.focus();
  }

  function laterKeys(e: KeyboardEvent) {
    if (e.key !== "Enter" || e.ctrlKey) return;
    e.preventDefault();
    if (laterParsed?.ok) mail.send(laterParsed.at);
  }

  // ----- snippets: ";name" in the body, or Ctrl+; -----
  /** Where the ";query" starts and what's typed after it; null while the list is closed. */
  let snip = $state<{ start: number; query: string } | null>(null);
  let snipActive = $state(0);
  const snipMatches = $derived(snip ? matchSnippets(mail.snippets, snip.query) : []);
  // Typing ";" shows the list only when there's something to pick; Ctrl+; always does.
  let snipForced = $state(false);
  const snipOpen = $derived(!!snip && (snipMatches.length > 0 || snipForced));
  /** Where the list goes: under the ";" (or above it near the bottom), as CSS. */
  let snipStyle = $state("");
  const SNIP_LIST_W = 384; // w-96
  const SNIP_LIST_H = 260; // roughly the tallest it gets

  /** The ";query" just before the caret in its paragraph: its document position and the query. */
  function triggerAtCaret(): { start: number; query: string } | null {
    if (!editor) return null;
    const { $from: pos, empty } = editor.state.selection;
    if (!empty) return null;
    const before = pos.parent.textBetween(0, pos.parentOffset, undefined, "\ufffc");
    const t = snippetTrigger(before, before.length);
    return t && { start: pos.start() + t.start, query: t.query };
  }

  function placeSnip(start: number) {
    if (!editor || !bodyBox) return;
    const at = editor.view.coordsAtPos(start);
    const box = bodyBox.getBoundingClientRect();
    const top = at.top - box.top;
    const left = Math.max(8, Math.min(at.left - box.left, box.width - SNIP_LIST_W - 8));
    const below = at.bottom - box.top + 4;
    snipStyle =
      below + SNIP_LIST_H > box.height && top > SNIP_LIST_H
        ? `left:${left}px; bottom:${box.height - top + 4}px`
        : `left:${left}px; top:${below}px`;
  }

  function updateSnip() {
    const t = editor?.isFocused || snipForced ? triggerAtCaret() : null;
    if (!t) {
      snip = null;
      snipForced = false;
    } else {
      if (snip?.query !== t.query) snipActive = 0;
      if (snip?.start !== t.start) placeSnip(t.start);
      snip = t;
    }
  }

  function openSnippets() {
    if (!editor) return;
    const { $from: pos } = editor.state.selection;
    const prev = pos.parentOffset > 0 ? pos.parent.textBetween(pos.parentOffset - 1, pos.parentOffset, undefined, "\ufffc") : "";
    // Start a ";" word at the caret (with a space before it if needed).
    const pad = prev && !/\s/.test(prev) ? " " : "";
    snipForced = true;
    editor.chain().focus().insertContent({ type: "text", text: pad + ";" }).run();
    updateSnip();
  }

  /** Text as editor nodes: a paragraph per line. */
  function paragraphsOf(text: string): PMNode[] {
    const schema = editor!.schema;
    return text.split("\n").map((line) => schema.nodes.paragraph.create(null, line ? schema.text(line) : null));
  }

  async function insertSnippet(s: Snippet) {
    if (!snip || !editor) return;
    const { start } = snip;
    snip = null;
    snipForced = false;
    const first = to.split(/[,;]/).map((x) => x.trim()).find(Boolean) ?? null;
    let person = first ? { name: null as string | null, address: first } : null;
    if (person) {
      const known = (await api.suggestContacts(person.address).catch(() => [])).find((c) => c.address.toLowerCase() === person!.address.toLowerCase());
      if (known?.name) person = { ...person, name: known.name };
    }
    const acct = mail.accountOf(from);
    const me = acct ? { name: acct.displayName, address: acct.email } : null;
    const text = expandSnippet(s.body, { to: person, me });
    // Open at both ends, so the first paragraph joins the text before the ";" and the
    // last one the text after the caret, instead of splitting them off.
    const slice = new Slice(Fragment.from(paragraphsOf(text)), 1, 1);
    const end = editor.state.selection.from;
    editor.view.dispatch(editor.state.tr.replaceRange(start, end, slice).scrollIntoView());
    editor.commands.focus();
  }

  /** Keys for the snippet list while it's open. True if the key was used. */
  function snippetKeys(e: KeyboardEvent): boolean {
    if (!snipOpen) return false;
    const n = snipMatches.length;
    if (e.key === "ArrowDown" && n) snipActive = (snipActive + 1) % n;
    else if (e.key === "ArrowUp" && n) snipActive = (snipActive - 1 + n) % n;
    else if ((e.key === "Enter" || e.key === "Tab") && n && !e.ctrlKey) insertSnippet(snipMatches[snipActive]);
    else if (e.key === "Escape") {
      // Close the list, not the composer.
      snip = null;
      snipForced = false;
    } else return false;
    return true;
  }

  // ----- signature -----
  const sigSetting = $derived(mail.signatureOf(from));
  /** Taken off by hand: switching From then doesn't put one back. */
  let sigRemoved = false;

  /** The signature block in the text, with its position, if there is one. */
  function findSignature(): { pos: number; size: number } | null {
    let found: { pos: number; size: number } | null = null;
    editor?.state.doc.forEach((node, pos) => {
      if (!found && node.type.name === "signature") found = { pos, size: node.nodeSize };
    });
    return found;
  }
  /** Reads `tick` so the markup follows the text. */
  const hasSignature = () => tick >= 0 && !!findSignature();

  /** Where your own text ends: just before the signature. */
  function textEnd(): number | "end" {
    const sig = findSignature();
    return sig && sig.pos > 0 ? sig.pos - 1 : "end";
  }

  /** Swaps the signature block for `html` ("" removes it). */
  function setSignature(html: string) {
    if (!editor) return;
    const sig = findSignature();
    let chain = editor.chain();
    if (sig) chain = chain.deleteRange({ from: sig.pos, to: sig.pos + sig.size });
    if (html) chain = chain.insertContentAt(sig ? sig.pos : editor.state.doc.content.size, `<div class="${SIGNATURE_CLASS}">${html}</div>`);
    chain.run();
  }

  /** Ctrl+Shift+S: take the signature off this message, or put the account's back. */
  function toggleSignature() {
    if (findSignature()) {
      sigRemoved = true;
      setSignature("");
    } else if (sigSetting?.html && !sigSetting.managed) {
      sigRemoved = false;
      setSignature(sigSetting.html);
    }
    // Back in the text (from the Signature button too, so the keys go on working).
    editor?.commands.focus(textEnd());
  }

  /** A new message moved to another account takes that account's signature. */
  function fromChanged() {
    if (!sigRemoved) setSignature(mail.startingSignature(from, c.kind));
  }

  // ----- the editor -----
  let host: HTMLDivElement | undefined = $state();
  /** The text's box, which the snippet list is placed in. */
  let bodyBox: HTMLDivElement | undefined = $state();
  let editor: Editor | null = null;
  /** Bumped on every editor transaction, so the toolbar's active states re-render. */
  let tick = $state(0);
  let linkUrl = $state<string | null>(null);
  let linkInput: HTMLInputElement | undefined = $state();

  /** Reads `tick` so the toolbar follows the selection. */
  const isActive = (name: string) => tick >= 0 && (editor?.isActive(name) ?? false);
  const inText = () => mail.composerActive && !!editor?.isFocused;

  function format(run: (e: Editor) => void) {
    if (editor) run(editor);
  }

  function openLink() {
    if (!editor) return;
    linkUrl = editor.getAttributes("link").href ?? "";
    queueMicrotask(() => linkInput?.focus());
  }

  function applyLink(e?: Event) {
    e?.preventDefault();
    if (!editor || linkUrl === null) return;
    const url = linkUrl.trim();
    const chain = editor.chain().focus().extendMarkRange("link");
    if (!url) chain.unsetLink().run();
    else {
      const href = /^(https?:|mailto:)/i.test(url) ? url : url.includes("@") && !url.includes("/") ? `mailto:${url}` : `https://${url}`;
      // With nothing selected, the address itself becomes the link's text.
      if (editor.state.selection.empty && !editor.isActive("link")) chain.insertContent({ type: "text", text: url, marks: [{ type: "link", attrs: { href } }] }).run();
      else chain.setLink({ href }).run();
    }
    linkUrl = null;
  }

  function closeLink() {
    linkUrl = null;
    editor?.commands.focus();
  }

  const tools = [
    { id: "bold", label: "Bold (Ctrl+B)", icon: Bold, on: "bold", run: () => format((e) => e.chain().focus().toggleBold().run()) },
    { id: "italic", label: "Italic (Ctrl+I)", icon: Italic, on: "italic", run: () => format((e) => e.chain().focus().toggleItalic().run()) },
    { id: "underline", label: "Underline (Ctrl+U)", icon: Underline, on: "underline", run: () => format((e) => e.chain().focus().toggleUnderline().run()) },
    { id: "bullets", label: "Bulleted list (Ctrl+Shift+8, or type - and a space)", icon: List, on: "bulletList", run: () => format((e) => e.chain().focus().toggleBulletList().run()) },
    { id: "numbers", label: "Numbered list (Ctrl+Shift+7, or type 1. and a space)", icon: ListOrdered, on: "orderedList", run: () => format((e) => e.chain().focus().toggleOrderedList().run()) },
    { id: "link", label: "Link (Ctrl+K in the text)", icon: Link2, on: "link", run: openLink },
  ];

  function createEditor(element: HTMLElement) {
    editor = new Editor({
      element,
      extensions: [
        StarterKit.configure({ heading: { levels: [1, 2, 3] }, trailingNode: false, link: { openOnClick: false, autolink: true, defaultProtocol: "https" } }),
        SignatureNode,
        Placeholder.configure({ placeholder: isReply ? "Write your reply… (;name inserts a snippet)" : "Type ;name to insert a snippet" }),
      ],
      content: html,
      editorProps: {
        attributes: (state) => ({
          "data-testid": "compose-body",
          // The plain text, as sent: for tests and anything else that reads the field.
          "data-text": docToText(state.doc.toJSON()),
          "aria-label": "Message",
          class: ["compose-rich px-4 py-3 text-sm leading-relaxed text-ink-100 outline-none select-text", inline ? "min-h-48" : "min-h-full"].join(" "),
        }),
        handleKeyDown: (_view, e) => {
          if (snippetKeys(e)) return true;
          // Esc closes the composer (or what's open in it); nothing in the editor wants it.
          if (e.key === "Escape") {
            keymap.handle(e);
            return e.defaultPrevented;
          }
          if (e.ctrlKey && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "k") {
            openLink();
            return true;
          }
          // Ctrl chords are PeetMail's first (Ctrl+Enter sends, where Tiptap would add a line
          // break; Ctrl+Shift+B shows Bcc, not a quote). The rest (Ctrl+Z...) are the editor's.
          if (e.ctrlKey) {
            keymap.handle(e);
            return e.defaultPrevented;
          }
          return false;
        },
        handlePaste: (_view, e) => pasteFiles(e),
        // Files dropped on the text are attached by the composer's own drop handler.
        handleDrop: (_view, e) => !!(e as DragEvent).dataTransfer?.types.includes("Files"),
      },
      onTransaction: ({ transaction: tr }) => {
        // A bare focus or blur changes nothing shown here. Skipping it matters: removing the
        // focused text (closing the composer) blurs it while Svelte is updating the page,
        // and writing state then throws (state_unsafe_mutation).
        if (!tr.docChanged && !tr.selectionSet && (tr.getMeta("focus") || tr.getMeta("blur"))) return;
        tick++;
        updateSnip();
      },
      onUpdate: ({ editor: ed }) => {
        html = ed.isEmpty ? "" : ed.getHTML();
        body = docToText(ed.getJSON());
      },
      onBlur: () =>
        setTimeout(() => {
          if (!editor?.isFocused) snip = null;
        }, 150),
    });
  }

  onDestroy(() => {
    editor?.destroy();
    mail.composerFocused = false;
  });

  // r on a conversation whose draft is showing puts the cursor in it.
  let focusSeen = mail.composerFocusTick;
  $effect(() => {
    if (mail.composerFocusTick === focusSeen) return;
    focusSeen = mail.composerFocusTick;
    editor?.commands.focus(textEnd());
    root?.scrollIntoView({ block: "nearest" });
  });

  let toInput: HTMLInputElement | undefined = $state();

  const isReply = c.kind === "reply" || c.kind === "replyAll";
  const title = { new: "New message", reply: "Reply", replyAll: "Reply all", forward: "Forward" }[c.kind];

  // Keep the store in sync so send() sees the latest text.
  $effect(() => {
    if (mail.composer) Object.assign(mail.composer, { to, cc, bcc, subject, body, html, accountId: from });
  });

  // ----- contact suggestions for the field being typed in -----
  let suggestions = $state<Contact[]>([]);
  let active = $state(0);
  type Field = "to" | "cc" | "bcc";
  let field = $state<Field | null>(null);

  // One lookup per keystroke; only the latest may update the list, or a slow reply for
  // "p" could replace the one for "priya" and Enter would pick the wrong person.
  const lookup = latestOnly(async (prefix: string) => (prefix ? api.suggestContacts(prefix) : []));

  async function suggest(which: Field, value: string) {
    field = which;
    const text = value.split(/[,;]/).pop()?.trim() ?? "";
    // Instantly keep only what still matches; the fresh lookup replaces it when it lands.
    suggestions = text ? narrow(suggestions, text) : [];
    active = 0;
    const found = await lookup(text);
    if (found === STALE) return;
    suggestions = found;
    active = 0;
  }

  function accept(which: Field, contact: Contact) {
    const parts = { to, cc, bcc }[which].split(/[,;]/).map((s) => s.trim()).filter(Boolean);
    parts.pop();
    const next = [...parts, contact.address].join(", ") + ", ";
    if (which === "to") to = next;
    else if (which === "cc") cc = next;
    else bcc = next;
    lookup.cancel(); // a lookup still in flight mustn't reopen the list
    suggestions = [];
  }

  function fieldKeys(which: Field, e: KeyboardEvent) {
    if (!suggestions.length || field !== which) return;
    // Ctrl+Enter sends, even while suggestions are up (they arrive a moment after typing,
    // so taking it as a pick made a quick Ctrl+Enter silently not send).
    if (e.key === "Enter" && e.ctrlKey) {
      lookup.cancel();
      suggestions = [];
      return;
    }
    if (e.key === "ArrowDown") active = (active + 1) % suggestions.length;
    else if (e.key === "ArrowUp") active = (active - 1 + suggestions.length) % suggestions.length;
    else if (e.key === "Enter" || e.key === "Tab") accept(which, suggestions[active]);
    // Esc closes the list first (the next Esc closes the composer).
    else if (e.key === "Escape") closeSuggestions();
    else return;
    e.preventDefault();
  }

  /** Leaving an address field: its list goes, and a lookup still on its way mustn't bring it
   * back (an open list would hold up Esc in the text). */
  function closeSuggestions() {
    lookup.cancel();
    suggestions = [];
  }

  function revealBcc() {
    showBcc = true;
    queueMicrotask(() => bccInput?.focus());
  }

  /** Closes it; what's in it stays in Drafts. */
  function close() {
    mail.composer = null;
  }

  onMount(() => {
    createEditor(host!);
    // A draft shown again under its conversation waits for r (or a click).
    if (c.focus !== false) {
      if (isReply) editor?.commands.focus(textEnd(), { scrollIntoView: !inline });
      else toInput?.focus({ preventScroll: inline });
      // Bring the whole reply into view, its fields included, with the thread above it.
      if (inline) root?.scrollIntoView({ block: "nearest" });
    }
    return keymap.register([
      { id: "compose.send", keys: "ctrl+Enter", label: "Send", group: "Compose", when: () => mail.composerActive, run: () => mail.send() },
      { id: "compose.later", keys: "ctrl+shift+Enter", label: "Send later", group: "Compose", when: () => mail.composerActive, run: () => (later === null ? openLater() : closeLater()) },
      { id: "compose.later.close", keys: "Escape", label: "Close send later", group: "Compose", when: () => mail.composerActive && later !== null, run: closeLater },
      { id: "compose.snippet", keys: "ctrl+;", label: "Insert snippet", group: "Compose", when: () => mail.composerActive && later === null, run: openSnippets },
      { id: "compose.close", keys: "Escape", label: "Close composer", group: "Compose", when: () => mail.composerActive && !suggestions.length && later === null && linkUrl === null && !snipOpen, run: close },
      { id: "compose.discard", keys: "ctrl+shift+d", label: "Discard draft", group: "Compose", when: () => mail.composerActive, run: () => mail.discardComposer() },
      { id: "compose.cc", keys: "ctrl+shift+c", label: "Show Cc", group: "Compose", when: () => mail.composerActive, run: () => (showCc = true) },
      { id: "compose.bcc", keys: "ctrl+shift+b", label: "Show Bcc", group: "Compose", when: () => mail.composerActive, run: revealBcc },
      { id: "compose.signature", keys: "ctrl+shift+s", label: "Add or remove signature", group: "Compose", when: () => mail.composerActive, run: toggleSignature },
      { id: "compose.bold", keys: "ctrl+b", label: "Bold", group: "Compose", when: inText, run: tools[0].run },
      { id: "compose.italic", keys: "ctrl+i", label: "Italic", group: "Compose", when: inText, run: tools[1].run },
      { id: "compose.underline", keys: "ctrl+u", label: "Underline", group: "Compose", when: inText, run: tools[2].run },
      // Shift+8 arrives as "*", so the editor itself handles these keys; listed for the overlay and palette.
      { id: "compose.bullets", keys: "ctrl+shift+8", label: "Bulleted list", group: "Compose", when: inText, run: tools[3].run },
      { id: "compose.numbers", keys: "ctrl+shift+7", label: "Numbered list", group: "Compose", when: inText, run: tools[4].run },
      // Taken by the text itself (Ctrl+K is the command palette everywhere else).
      { id: "compose.link", keys: "ctrl+k", label: "Insert link (in the message text)", group: "Compose", when: () => false, run: openLink },
      { id: "compose.attach", keys: "ctrl+shift+a", label: "Attach files", group: "Compose", when: () => mail.composerActive, run: pickFiles },
    ]);
  });
</script>

{#snippet chip(name: string, size: number, testid: string, remove: () => void)}
  <span data-testid={testid} class="flex items-center gap-2 rounded-md border border-ink-700 bg-ink-800 py-0.5 pr-0.5 pl-2.5 text-xs text-ink-300">
    <Paperclip size={12} class="shrink-0 text-ink-500" />
    <span class="max-w-64 truncate">{name}</span>
    <span class="text-ink-500">{fileSize(size)}</span>
    <button data-testid="{testid}-remove" title="Remove {name}" aria-label="Remove {name}" onclick={remove} class="icon-btn size-6"><X size={12} /></button>
  </span>
{/snippet}

{#snippet suggestionList(which: Field)}
  {#if field === which && suggestions.length}
    <ul data-testid="contact-suggestions" class="absolute top-full left-0 z-10 mt-1 w-80 overflow-hidden rounded-md border border-ink-700 bg-ink-900 shadow-xl">
      {#each suggestions as s, i (s.address)}
        <li>
          <button
            type="button"
            onmousedown={(e) => { e.preventDefault(); accept(which, s); }}
            class={["block w-full px-3 py-1.5 text-left text-sm", i === active ? "bg-ink-800 text-ink-100" : "text-ink-300"]}
          >
            {s.name ?? s.address} <span class="text-ink-500">{s.name ? s.address : ""}</span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
{/snippet}

<div
  data-testid="composer"
  data-inline={inline}
  data-active={mail.composerActive}
  bind:this={root}
  onfocusin={() => (mail.composerFocused = true)}
  onfocusout={(e) => {
    if (!root?.contains(e.relatedTarget as Node | null)) mail.composerFocused = false;
  }}
  class={inline ? "mt-4 scroll-mb-6" : "dialog-backdrop fixed inset-x-0 top-(--chrome-top) bottom-0 z-20 flex items-end justify-center bg-black/40 p-6"}
>
  <div
    role="group"
    aria-label={title}
    {ondragover}
    {ondragleave}
    {ondrop}
    class={["@container relative flex flex-col rounded-xl border bg-ink-900", inline ? "border-ink-600" : "dialog-panel h-[70vh] w-full max-w-3xl border-ink-700 shadow-2xl"]}
  >
    {#if dragging}
      <div data-testid="compose-drop" class="pointer-events-none absolute inset-0 z-30 flex items-center justify-center rounded-xl border-2 border-dashed border-accent bg-ink-950/80 text-sm text-ink-100">
        Drop to attach
      </div>
    {/if}
    <div class="flex items-center justify-between border-b border-ink-800 px-4 py-2.5 text-sm">
      <span class="font-semibold text-ink-100">{title}</span>
      <span class="flex items-center gap-2 text-xs whitespace-nowrap text-ink-500">
        <!-- The hints need room; a narrow reading pane drops them (the buttons say it too). -->
        <span class="hidden items-center gap-2 @xl:flex">
          <kbd>Ctrl</kbd> <kbd>Enter</kbd> send · <kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>Enter</kbd> later · <kbd>Esc</kbd> close
        </span>
        <button data-testid="compose-close" title="Close (Esc)" aria-label="Close" onclick={close} class="icon-btn size-7"><X size={15} /></button>
      </span>
    </div>

    <div class="space-y-0 border-b border-ink-800 text-sm">
      {#if mail.accounts.length > 1}
        <label class="flex items-center gap-3 border-b border-ink-800 px-4 py-2">
          <span class="w-14 shrink-0 text-ink-500">From</span>
          <span class={["size-2 shrink-0 rounded-full", mail.accountColor(from)]}></span>
          <select
            data-testid="compose-from"
            bind:value={from}
            onchange={fromChanged}
            disabled={c.kind !== "new"}
            title={c.kind !== "new" ? "Replies and forwards go from the account the mail came to" : ""}
            class="min-w-0 flex-1 bg-transparent text-ink-100 outline-none disabled:text-ink-300"
          >
            {#each mail.accounts as a (a.id)}
              <option value={a.id} class="bg-ink-900">{a.displayName ? `${a.displayName} <${a.email}>` : a.email}</option>
            {/each}
          </select>
        </label>
      {/if}
      <label class="relative flex items-center gap-3 px-4 py-2">
        <span class="w-14 shrink-0 text-ink-500">To</span>
        <input
          data-testid="compose-to"
          bind:this={toInput}
          bind:value={to}
          oninput={() => suggest("to", to)}
          onkeydown={(e) => fieldKeys("to", e)}
          onblur={closeSuggestions}
          spellcheck="false"
          autocomplete="off"
          class="min-w-0 flex-1 bg-transparent text-ink-100 outline-none select-text"
        />
        {#if !showCc}
          <button type="button" data-testid="compose-show-cc" title="Cc (Ctrl+Shift+C)" onclick={() => (showCc = true)} class="text-xs text-ink-500 hover:text-ink-100">Cc</button>
        {/if}
        {#if !showBcc}
          <button type="button" data-testid="compose-show-bcc" title="Bcc (Ctrl+Shift+B)" onclick={revealBcc} class="text-xs text-ink-500 hover:text-ink-100">Bcc</button>
        {/if}
        {@render suggestionList("to")}
      </label>
      {#if showCc}
        <label class="relative flex items-center gap-3 border-t border-ink-800 px-4 py-2">
          <span class="w-14 shrink-0 text-ink-500">Cc</span>
          <input
            data-testid="compose-cc"
            bind:value={cc}
            oninput={() => suggest("cc", cc)}
            onkeydown={(e) => fieldKeys("cc", e)}
            onblur={closeSuggestions}
            spellcheck="false"
            autocomplete="off"
            class="min-w-0 flex-1 bg-transparent text-ink-100 outline-none select-text"
          />
          {@render suggestionList("cc")}
        </label>
      {/if}
      {#if showBcc}
        <label class="relative flex items-center gap-3 border-t border-ink-800 px-4 py-2">
          <span class="w-14 shrink-0 text-ink-500" title="Blind copy: the other recipients don't see these addresses">Bcc</span>
          <input
            data-testid="compose-bcc"
            bind:this={bccInput}
            bind:value={bcc}
            oninput={() => suggest("bcc", bcc)}
            onkeydown={(e) => fieldKeys("bcc", e)}
            onblur={closeSuggestions}
            spellcheck="false"
            autocomplete="off"
            class="min-w-0 flex-1 bg-transparent text-ink-100 outline-none select-text"
          />
          {@render suggestionList("bcc")}
        </label>
      {/if}
      <label class="flex items-center gap-3 border-t border-ink-800 px-4 py-2">
        <span class="w-14 shrink-0 text-ink-500">Subject</span>
        <input
          data-testid="compose-subject"
          bind:value={subject}
          disabled={isReply}
          class="min-w-0 flex-1 bg-transparent text-ink-100 outline-none select-text disabled:text-ink-500"
        />
      </label>
    </div>

    <div bind:this={bodyBox} class={["relative flex flex-col", !inline && "min-h-0 flex-1"]}>
      <!-- Inline it grows with what's typed (the reader scrolls); in the dialog it scrolls itself. -->
      <div bind:this={host} class={inline ? "" : "min-h-0 flex-1 overflow-y-auto"}></div>
      {#if sigSetting?.managed}
        <p data-testid="compose-signature-managed" class="px-4 pb-2 text-xs text-ink-500">Your organisation adds your signature when this is sent.</p>
      {/if}
      {#if snipOpen}
        <div data-testid="snippet-list" style={snipStyle} class="absolute z-10 w-96 overflow-hidden rounded-md border border-ink-700 bg-ink-900 shadow-xl">
          {#if snipMatches.length}
            <ul class="max-h-64 overflow-y-auto py-1">
              {#each snipMatches as s, i (s.name)}
                <li>
                  <button
                    type="button"
                    data-testid="snippet-option"
                    onmousedown={(e) => { e.preventDefault(); insertSnippet(s); }}
                    onmousemove={() => (snipActive = i)}
                    class={["block w-full px-3 py-1.5 text-left text-sm", i === snipActive ? "bg-ink-800" : ""]}
                  >
                    <span class="font-mono text-accent">;{s.name}</span>
                    <span class="ml-2 truncate text-ink-500">{s.body.split("\n")[0]}</span>
                  </button>
                </li>
              {/each}
            </ul>
          {:else}
            <p data-testid="snippet-none" class="px-3 py-2 text-xs text-ink-500">
              {mail.snippets.length ? "No snippet matches." : "No snippets yet. Add them in Settings (Ctrl+,)."}
            </p>
          {/if}
          <p class="border-t border-ink-800 px-3 py-1 text-xs text-ink-500"><kbd>Enter</kbd> insert · <kbd>Esc</kbd> dismiss</p>
        </div>
      {/if}
    </div>

    {#if c.attachments.length || keptForwarded.length || attaching}
      <div data-testid="compose-attachments" class="flex flex-wrap gap-2 border-t border-ink-800 px-4 py-2.5">
        {#each keptForwarded as a (a.id)}
          {@render chip(a.name, a.size, "compose-forwarded", () => c.droppedAttachments.push(a.name))}
        {/each}
        {#each c.attachments as f (f.id)}
          {@render chip(f.name, f.size, "compose-attachment", () => removeFile(f.id))}
        {/each}
        {#if attaching}
          <span data-testid="compose-attaching" class="px-1 py-1 text-xs text-ink-500">Attaching…</span>
        {/if}
      </div>
    {/if}

    {#if later !== null}
      <div data-testid="compose-later" class="space-y-2 border-t border-ink-800 px-4 py-3 text-sm">
        <div class="flex items-center gap-3">
          <span class="shrink-0 text-ink-500">Send</span>
          <input
            data-testid="compose-later-input"
            bind:this={laterInput}
            bind:value={later}
            onkeydown={laterKeys}
            placeholder="tomorrow 9am, mon, in 2 hours…"
            spellcheck="false"
            autocomplete="off"
            class="min-w-0 flex-1 bg-transparent text-ink-100 outline-none select-text"
          />
          {#if laterParsed?.ok}
            <span data-testid="compose-later-when" class="shrink-0 text-accent">{formatSendAt(laterParsed.at.toISOString())}</span>
          {:else if laterParsed}
            <span data-testid="compose-later-hint" class="shrink-0 text-ink-500">{laterParsed.reason}</span>
          {/if}
        </div>
        <div class="flex flex-wrap gap-2">
          {#each presets as p (p.label)}
            <button
              data-testid="compose-later-preset"
              type="button"
              onclick={() => mail.send(p.at)}
              class="rounded-md border border-ink-700 px-2 py-1 text-xs text-ink-300 hover:border-accent hover:text-ink-100"
            >
              {p.label} <span class="text-ink-500">{formatSendAt(p.at.toISOString())}</span>
            </button>
          {/each}
        </div>
      </div>
    {/if}

    {#if linkUrl !== null}
      <form data-testid="compose-link" onsubmit={applyLink} class="flex items-center gap-3 border-t border-ink-800 px-4 py-2 text-sm">
        <span class="shrink-0 text-ink-500">Link</span>
        <input
          data-testid="compose-link-input"
          bind:this={linkInput}
          bind:value={linkUrl}
          onkeydown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              closeLink();
            }
          }}
          placeholder="https://… or an email address (empty removes the link)"
          spellcheck="false"
          autocomplete="off"
          class="min-w-0 flex-1 bg-transparent text-ink-100 outline-none select-text"
        />
        <button data-testid="compose-link-apply" class="rounded-md border border-ink-700 px-2 py-0.5 text-xs text-ink-300 hover:text-ink-100">Set</button>
      </form>
    {/if}

    <!-- A phone: the formatting tools on one row, the buttons that send on the next. -->
    <div class={["flex items-center justify-between border-t border-ink-800", layout.compact ? "flex-wrap gap-x-3 gap-y-1 px-3 py-2" : "gap-3 px-4 py-2.5"]}>
      <!-- Out of the tab order: Tab goes from the text to the buttons that send. -->
      <span class="flex shrink-0 items-center gap-0.5">
        {#each tools as t (t.id)}
          <button
            type="button"
            data-testid="compose-format-{t.id}"
            title={t.label}
            aria-label={t.label}
            aria-pressed={isActive(t.on)}
            tabindex="-1"
            onmousedown={(e) => e.preventDefault()}
            onclick={t.run}
            class={["icon-btn size-7", isActive(t.on) && "bg-ink-800 text-ink-100"]}
          >
            <t.icon size={14} />
          </button>
        {/each}
      </span>
      <span data-testid="compose-hint" class={["min-w-0 flex-1 truncate text-xs text-ink-500", layout.compact && "hidden"]}>
        {c.draftId !== null ? "Draft saved" : isReply ? (inline ? "The message above is quoted below your reply." : "The original message is quoted below your reply.") : ""}
      </span>
      <span class={["flex shrink-0 items-center gap-2 whitespace-nowrap", layout.compact && "ml-auto"]}>
        <input
          data-testid="compose-file-input"
          bind:this={fileInput}
          type="file"
          multiple
          class="hidden"
          onchange={(e) => {
            const input = e.currentTarget;
            addFiles([...(input.files ?? [])]);
            input.value = ""; // picking the same file again still fires
            editor?.commands.focus();
          }}
        />
        <button data-testid="compose-discard" title="Discard draft (Ctrl+Shift+D)" aria-label="Discard draft" onclick={() => mail.discardComposer()} class="icon-btn size-8">
          <Trash2 size={15} />
        </button>
        <button data-testid="compose-attach" title="Attach files (Ctrl+Shift+A)" aria-label="Attach files" onclick={pickFiles} class="icon-btn size-8">
          <Paperclip size={15} />
        </button>
        {#if !hasSignature() && sigSetting?.html && !sigSetting.managed}
          <button data-testid="compose-signature-add" title="Add signature (Ctrl+Shift+S)" onmousedown={(e) => e.preventDefault()} onclick={toggleSignature} class="px-2 py-1.5 text-sm text-ink-500 hover:text-ink-100">
            Signature
          </button>
        {/if}
        <button
          data-testid="compose-later-toggle"
          title="Send later (Ctrl+Shift+Enter)"
          onclick={() => (later === null ? openLater() : closeLater())}
          class="rounded-md border border-ink-700 px-3 py-1.5 text-sm text-ink-300 hover:text-ink-100"
        >
          Send later
        </button>
        <button data-testid="compose-send" title="Send (Ctrl+Enter)" onclick={() => mail.send()} class="rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-ink-950">
          Send
        </button>
      </span>
    </div>
  </div>
</div>

<style>
  /* A paragraph is a line (Enter starts the next), as it's sent: no gaps of its own. */
  :global(.compose-rich p) { margin: 0; min-height: 1lh; }
  :global(.compose-rich ul) { list-style: disc; padding-left: 1.4em; margin: 0; }
  :global(.compose-rich ol) { list-style: decimal; padding-left: 1.4em; margin: 0; }
  :global(.compose-rich blockquote) { border-left: 2px solid var(--color-ink-500); padding-left: 0.8em; margin: 0; color: var(--color-ink-300); }
  :global(.compose-rich h1) { font-size: 1.4em; font-weight: 600; margin: 0; }
  :global(.compose-rich h2) { font-size: 1.2em; font-weight: 600; margin: 0; }
  :global(.compose-rich h3) { font-weight: 600; margin: 0; }
  :global(.compose-rich code) { font-family: ui-monospace, monospace; font-size: 0.9em; background: var(--color-ink-800); padding: 0 0.25em; border-radius: 3px; }
  :global(.compose-rich pre) { background: var(--color-ink-800); padding: 0.5em 0.75em; border-radius: 4px; margin: 0; white-space: pre-wrap; }
  :global(.compose-rich pre code) { background: none; padding: 0; }
  :global(.compose-rich a) { color: var(--color-accent); text-decoration: underline; }
  :global(.compose-rich hr) { border-color: var(--color-ink-700); margin: 0.75em 0; }
  /* The signature, a line below the text, as it's sent. */
  :global(.compose-rich .peetmail_signature) { margin-top: 1lh; }
  /* The placeholder still shows with only the signature below it. */
  :global(.compose-rich p.is-editor-empty:first-child::before),
  :global(.compose-rich > p.is-empty:first-child:has(+ .peetmail_signature:last-child)::before) {
    content: attr(data-placeholder);
    color: var(--color-ink-500);
    float: left;
    height: 0;
    pointer-events: none;
  }
</style>
