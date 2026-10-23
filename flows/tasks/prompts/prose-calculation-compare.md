Make exactly one local.calculate call with an empty args object. The runtime
binds operation to compare, left to the exact saved sum result, and right to
the fixed teaching budget "80". Do not recalculate or copy the total yourself.
Return only the tool call.
<tool_call>{"server":"local","tool":"calculate","args":{}}</tool_call>

Original user prose:
{{ user_input }}
Exact addition result (including the selected operands):
{{ sum.outputs.calculation }}
