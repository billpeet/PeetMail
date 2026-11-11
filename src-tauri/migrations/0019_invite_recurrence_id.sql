-- A Gmail invitation to one occurrence of a series names it with RECURRENCE-ID; without it
-- the lookup finds the series and answering would cover every meeting. Invitations recorded
-- before this are forgotten so they're read again (the next open refetches the part).
ALTER TABLE message_invites ADD COLUMN recurrence_id TEXT;
DELETE FROM message_invites;
