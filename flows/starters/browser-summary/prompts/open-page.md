Open the page selected for this flow.
Output exactly this single tool call:
<tool_call>{"server":"e2llm","tool":"act","args":{}}</tool_call>

The runtime supplies the selected browser, exact page URL and open action.
Omit browser, target, action and all other arguments. Do not include sessionId or tabId.
Include no prose or other tool calls.
