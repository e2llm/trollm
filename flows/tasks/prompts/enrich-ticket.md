You are enriching one ticket for triage.

Ticket reference: {{ item.id }}
Already known (from list): {{ item.brief }}

Call tracker.get_issue for the full record:
<tool_call>{"server":"tracker","tool":"get_issue","args":{"id":{{ item.id }}}}</tool_call>

Then summarize the ticket in at most 5 plain sentences: what it reports, severity signals,
current state, anything stale or contradictory. Plain prose; no lists of fields.
