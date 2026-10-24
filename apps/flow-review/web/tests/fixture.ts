import type { Inspection } from "../src/model";
export function fixture(): Inspection {
  return {
    format: "trollm.flow-review.v1",
    packageId: "revision-1",
    name: "Example",
    entryFlow: "main.flow.yaml",
    taskRegistry: "main.tasks.yaml",
    candidates: {
      entryFlows: ["main.flow.yaml"],
      taskRegistries: ["main.tasks.yaml"],
    },
    valid: true,
    diagnostics: [],
    files: [
      {
        path: "prompts/shared.md",
        content: "Read {{ user_input }} <script>alert(1)</script>",
        sha256: "a",
        sizeBytes: 50,
      },
    ],
    plans: [
      {
        path: "main.flow.yaml",
        id: "main",
        entryStep: "start",
        limits: { maxSteps: 20 },
        nodes: {
          start: {
            kind: "task",
            taskRef: "shared",
            routes: {
              again: "start",
              wait: { status: "paused", resumeAt: "done", reason: "Wait" },
            },
            onError: { retry: 2, then: "done" },
          },
          done: { kind: "end", outcome: "ok" },
          unreachable: { kind: "task", taskRef: "shared" },
        },
      },
    ],
    registry: {
      path: "main.tasks.yaml",
      id: "tasks",
      tasks: {
        shared: {
          model: "coder",
          prompt: "prompts/shared.md",
          tools: ["flow.choose"],
          completion: "flow-route",
        },
      },
    },
    setup: null,
    presentation: {
      format: "trollm.flow-presentation.v1",
      title: "Review example",
      purpose: "Inspect the example",
      phases: [
        {
          id: "phase",
          title: "First phase",
          description: "Human description",
          nodes: [{ flow: "main", node: "start" }],
        },
      ],
    },
    unassignedNodes: [
      { flow: "main", node: "done" },
      { flow: "main", node: "unreachable" },
    ],
  };
}
