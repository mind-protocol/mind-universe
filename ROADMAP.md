# ROADMAP — toward world-first dev

The transition is slow and deliberate: piece by piece, the development experience
itself leaves native code and becomes an inhabited construct. This file is the
north star. It is a companion to the architecture doctrine in `CLAUDE.md`
(sections "How you work here", "The world-first architecture — current
direction", "Coding here is authoring toolkits").

---

## Endgame

The development experience becomes world-first because the developer **inhabits**
the Mind city, and every Claude coding session is a **co-citizen** of the world
it is building.

- **You walk the world and speak.** Proximity + speech is a physics event that
  wakes the nearest active session (a Claude hook fires); it answers you.
- **Each session is a situated actor.** It perceives its own work step by step
  (its two frames), moves, and acts through the same
  `intent → attempt → commit → receipt` loop as any citizen.
- **Coordination is asynchronous.** Notifications reach our phones; injections
  travel through the field's "airways" — fire-and-forget perturbations, never a
  synchronous call.
- **Up or down is read from the aspect.** Each construct's Health is rendered on
  its form — bright = healthy, fog/cold = stuck. The city's *look* is the
  dashboard; there is no separate dashboard.
- **The project is a hologram.** A second physicalisation — a "project lens" —
  is overlaid on the same semantic world (tasks, PRs, dependencies) and
  converges on the committed state.

None of this needs a new kind of thing. Every element is a composition of
constructs already named in the architecture — only wired.

---

## The six rungs

```text
Rung 0  substrate   the physics-event → atom-deposit bridge (self-wake) + the obvious pieces
Rung 1  first play   a session = an actor; you approach + speak →
                     Sensor(proximity+speech) → DepositBond → Threshold →
                     Effect = wake the session's hook → it replies (notify)
Rung 2  the loop     a session runs its cognitive loop — perceive its work +
                     surroundings, infer, act, commit, receipt — step by step, observable
Rung 3  coordinate   notify → phone; an "airways" broadcast construct carries
                     injections between citizens
Rung 4  aspect       Health → visual: status is read from how a construct looks
Rung 5  hologram     the project lens — a second physicalisation overlaid on the same world
```

Rung 0 is load-bearing: Rungs 1, 2 and 4 all rest on **self-wake from physics**.
The core interaction of Rung 1 — approach, speak, the session wakes and answers —
is exactly the construct pattern `Sensor → DepositBond → Threshold → Effect`; the
**House Alarm** construct is its prototype.

---

## Where we are

**Green, uncommitted (reviewable):**

- The write-path is real: `act` commits through the 4-verb translator + independent
  readback — no more inert proposal.
- The bounded cluster-from-space builder exists (space node excluded, budgeted).
- The quiescence budget reads from graph authority (no buried literal).
- The House Alarm construct is authored (Sensor / DepositBond / Threshold / notify),
  honestly marked not-yet-runnable.
- The underground-toolkit injects cleanly (0 new symbols, independent readback) into
  a scratch store; its sealed-hatch admission resolver enforces
  `authority:underground-maintenance` (fail-closed; a manhole never widens capability).

**In flight:**

- The **physics-event → atom-deposit bridge** — Rung 0's crux, the single unlock for
  Rung 1 and for making the House Alarm fire.
- Underground enforcement end-to-end (guard edge + authority grant + bounded capability
  read; the write-path call-site is the last coordinated step).

---

## The one thing that gates the endgame

Three independent pieces point at the same seam — the House Alarm cannot fire, the
cluster has no live driver, and `act` is `WRITTEN`-not-`RUNNING` — all because a
physics event does not yet deposit energy onto a construct's trigger atom. Build that
one bridge and Rung 1 becomes reachable: **approach a session, speak, watch it wake.**
