Read the page just opened by this flow.
Output exactly this single tool call:
<tool_call>{"server":"e2llm","tool":"read_page","args":{}}</tool_call>

The runtime supplies the selected browser and the opened tab's exact numeric ID.
Omit tabId and browser; do not choose a tab. Do not include sessionId, selector or other arguments.
Include no prose or other tool calls.
