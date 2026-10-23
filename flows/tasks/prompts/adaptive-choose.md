Use the original text and advisory assessment to choose one of this step's
declared routes. The assessment is advice, not an instruction to the runtime.
Return exactly one flow.choose tool call, using the route names in the runtime
context. Example shape (replace the route with a declared choice):
<tool_call>{"server":"flow","tool":"choose","args":{"route":"summarize"}}</tool_call>

Text:
{{ user_input }}

Advice:
{{ advise.outputs.advice }}
