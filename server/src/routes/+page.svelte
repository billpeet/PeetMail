<script lang="ts">
  // The only page: says what this is. The sync API lives under /v1.
</script>

<svelte:head><title>PeetMail sync</title></svelte:head>

<main class="mx-auto flex min-h-screen max-w-xl flex-col justify-center gap-4 px-4 py-16">
  <h1 class="text-2xl font-semibold text-ink-50">PeetMail sync server</h1>
  <p class="text-ink-300">
    This server keeps PeetMail's settings in step between your devices. Everything it stores is encrypted on your devices first,
    so it can't read any of it.
  </p>
  <p class="text-ink-300">
    To use it, open PeetMail and go to <b class="text-ink-100">Settings → Sync</b>, then enter this page's address.
  </p>
</main>
