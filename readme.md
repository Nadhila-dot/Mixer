# Mixer

> A hosted AI agent that takes a business request, spins up a sandboxed workspace, writes real files, runs real commands, recovers from failures, and streams its progress back live.

Built for the **AI Agent Olympics Hackathon 2026** — Vultr submission.

## How it works

```mermaid
sequenceDiagram
    autonumber
    participant U as User
    participant M as Mixer Agent
    participant T as Tool Layer
    participant W as Workspace<br/>(sandboxed FS)

    U->>M: send message
    loop agent loop
        M->>M: think (pick next action)
        M->>T: emit tool call<br/>CreateFile · Command · ReadFile · …
        T->>W: read / write / run
        W-->>T: file tree + stdout
        T-->>M: tool result
    end
    M-->>U: final reply (streamed)
```
