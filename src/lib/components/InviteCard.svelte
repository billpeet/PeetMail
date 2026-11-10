<script lang="ts">
  // RSVP card shown above a conversation containing a meeting invite. y / m / n answer it.
  import { CalendarClock, Check, HelpCircle, MapPin, Repeat, X } from "@lucide/svelte";
  import { calendar, eventTimes } from "#lib/calendar.svelte.js";
  import { mail } from "#lib/mail.svelte.js";
  import { describeRecurrence } from "#lib/recurrence.js";
  import type { Invite, Rsvp } from "#lib/api.js";

  let { invite }: { invite: Invite } = $props();

  const e = $derived(invite.event);

  const fmtRange = (startIso: string, endIso: string | null) => {
    const s = new Date(startIso);
    const date = s.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" });
    const t = (d: Date) => d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
    return endIso ? `${date} · ${t(s)} – ${t(new Date(endIso))}` : `${date} · ${t(s)}`;
  };

  /** Set when this is an update that moved the meeting. */
  const timeChanged = $derived(
    !!invite.previousStart && !e.isAllDay && (invite.previousStart !== e.startAt || (invite.previousEnd ?? e.endAt) !== e.endAt),
  );
  const locationChanged = $derived(!!invite.previousLocation && invite.previousLocation !== (e.location ?? ""));
  const isUpdate = $derived(timeChanged || locationChanged || invite.meetingRequestType === "fullUpdate");

  /** You answered an earlier version of this meeting in this thread. */
  const earlierAnswer = $derived.by(() => {
    const me = mail.accountOf(mail.invite?.event.accountId ?? -1)?.email.toLowerCase() ?? mail.account?.email.toLowerCase();
    const requests = mail.thread.filter((m) => m.meetingMessageType === "meetingRequest");
    const latest = requests.at(-1)?.receivedAt ?? "";
    return [...mail.thread]
      .reverse()
      .find(
        (m) =>
          m.receivedAt < latest &&
          (m.folderRole === "sentitems" || m.fromAddress?.toLowerCase() === me) &&
          /^meeting(Accepted|Tenatively|Tentatively|Declined)/.test(m.meetingMessageType ?? ""),
      )?.meetingMessageType;
  });

  const cancelled = $derived(invite.meetingMessageType === "meetingCancelled" || e.isCancelled);
  const answerable = $derived(!cancelled && !e.isOrganizer && invite.meetingMessageType !== "meetingAccepted");

  /** The meeting the card's time is for: a series' next one when it's known, else its first. */
  const shown = $derived(invite.next ?? e);
  const when = $derived.by(() => {
    const { start, end } = eventTimes(shown);
    const date = start.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" });
    if (e.isAllDay) return `${date} · all day`;
    const t = (d: Date) => d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
    return `${date} · ${t(start)} – ${t(end)}`;
  });

  const STATUS: Record<string, string> = {
    accepted: "You're going",
    tentativelyAccepted: "You said maybe",
    declined: "You declined",
    organizer: "You're the organiser",
  };

  const buttons: { r: Rsvp; status: string; label: string; key: string; Icon: typeof Check }[] = [
    { r: "accept", status: "accepted", label: "Accept", key: "Y", Icon: Check },
    { r: "tentative", status: "tentativelyAccepted", label: "Maybe", key: "M", Icon: HelpCircle },
    { r: "decline", status: "declined", label: "Decline", key: "N", Icon: X },
  ];

  function showInCalendar() {
    calendar.showWeekOf(eventTimes(shown).start);
    mail.view = "calendar";
    calendar.selected = e;
    mail.overlay = "event";
  }
</script>

<section data-testid="invite-card" data-response={e.response ?? "none"} class="mb-4 rounded-lg border border-accent/40 bg-accent/10 p-4">
  <div class="flex items-start gap-3">
    <CalendarClock size={20} class="mt-0.5 shrink-0 text-accent" />
    <div class="min-w-0 flex-1">
      <p class={["font-semibold text-ink-100", cancelled && "line-through"]}>
        {#if isUpdate && !cancelled}<span class="mr-1 rounded bg-amber-400/15 px-1.5 py-0.5 text-[11px] font-medium text-amber-300">Updated</span>{/if}
        {e.subject || "(no title)"}
      </p>
      <p class="mt-0.5 text-sm text-ink-300">{invite.recurrence ? `${invite.next ? "Next" : "First"}: ${when}` : when}</p>
      {#if invite.recurrence}
        <p data-testid="invite-recurrence" class="mt-0.5 flex items-center gap-1 text-sm text-ink-500"><Repeat size={13} />{describeRecurrence(invite.recurrence)}</p>
      {/if}
      {#if e.location}<p class="mt-0.5 flex items-center gap-1 text-sm text-ink-500"><MapPin size={13} />{e.location}</p>{/if}
      {#if timeChanged}
        <p data-testid="invite-changed" class="mt-1.5 text-sm">
          <span class="font-medium text-amber-300">Time changed</span>
          <span class="ml-1 text-ink-500 line-through">{fmtRange(invite.previousStart!, invite.previousEnd)}</span>
        </p>
      {/if}
      {#if locationChanged}
        <p data-testid="invite-location-changed" class="mt-0.5 text-sm">
          <span class="font-medium text-amber-300">Location changed</span>
          <span class="ml-1 text-ink-500 line-through">{invite.previousLocation}</span>
        </p>
      {/if}
      <p data-testid="invite-status" class="mt-1 text-xs text-ink-500">
        {#if cancelled}
          This meeting was cancelled
        {:else if STATUS[e.response ?? ""]}
          {STATUS[e.response ?? ""]}
        {:else if isUpdate && earlierAnswer}
          {earlierAnswer === "meetingAccepted" ? "You accepted" : earlierAnswer === "meetingDeclined" ? "You declined" : "You said maybe to"}
          the earlier {timeChanged ? "time" : "version"}. This update needs your response.
        {:else}
          You haven't responded yet
        {/if}
      </p>
    </div>
    <button data-testid="invite-show" onclick={showInCalendar} class="shrink-0 text-xs text-accent hover:underline">Show in calendar</button>
  </div>
  {#if answerable}
    <div class="mt-3 flex gap-2">
      {#each buttons as b (b.r)}
        {@const active = e.response === b.status}
        <button
          data-testid="rsvp-{b.r}"
          aria-pressed={active}
          title="{b.label} ({b.key})"
          onclick={() => mail.rsvp(b.r)}
          class={[
            "flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm",
            active ? "bg-accent font-medium text-chrome" : "border border-ink-700 text-ink-300 hover:border-ink-500 hover:text-ink-100",
          ]}
        >
          <b.Icon size={14} />{b.label}
          <kbd class="ml-1">{b.key.toLowerCase()}</kbd>
        </button>
      {/each}
    </div>
  {/if}
</section>
