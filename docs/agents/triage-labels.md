# Triage Labels

Ferrocull keeps a human in the loop on every issue — there is no AFK-agent flow. The triage vocabulary is just the three states an incoming issue can be in before it becomes regular open work.

| Label          | Meaning                                  |
| -------------- | ---------------------------------------- |
| `needs-triage` | Maintainer needs to evaluate this issue  |
| `needs-info`   | Waiting on reporter for more information |
| `wontfix`      | Will not be actioned                     |

Anything past triage is just an open issue — no label required. A human implements every accepted issue, so there is no `ready-for-agent` or `ready-for-human` distinction.

## Mapping for skills that speak the canonical vocabulary

When a Matt Pocock skill mentions one of its canonical roles, translate:

- `needs-triage` → `needs-triage`
- `needs-info` → `needs-info`
- `wontfix` → `wontfix`
- `ready-for-agent` → **does not apply**. If a skill would route an issue here, route it to a regular open issue instead and surface it to the maintainer.
- `ready-for-human` → **does not apply**. Every open issue is implicitly for a human; do not invent a label for the default state.
