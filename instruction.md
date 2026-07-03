Implement compact DAG visualization for the Operation and Change panes.

Scope:
- Render parent/child relationships using Unicode graph characters such as:
  ○ │ ├ └ ┬ ─ ╭ ╰
- Use the existing structured parent IDs. Do not parse `jj log` graph output.
- Support:
  - linear history
  - branching
  - merges / multiple parents
  - multiple operation heads
- Assign deterministic lane indices to visible nodes and render a graph prefix before each row’s text.
- Preserve the current ordering, selection, scrolling, pane focus, and reference IDs.
- Highlight the selected node without replacing its graph marker.
- Keep ROOT as `ROOT [immutable]`.

Operation pane:
- Render operation parent edges.
- Treat folded consecutive `snapshot working copy` rows as virtual display nodes.
- A folded snapshot group must preserve the visible edge between the operations before and after it.
- Virtual folded rows must not be usable as mutation targets.
- Show operation heads with a distinct marker.
- Do not yet add separate edges for undo/restore targets; keep those as textual metadata if available.

Change pane:
- Render commit parent edges.
- Support multiple parents.
- Continue identifying nodes by both change ID and commit ID.
- Do not confuse rewrite predecessor relationships with parent edges.
- Rewrite relations may remain in Detail view for now.

Layout:
- Reserve only as many graph columns as needed for the visible graph.
- Truncate descriptions before truncating essential graph structure or IDs.
- Fall back to ASCII characters when ASCII-only mode is enabled.

Architecture:
- Add a reusable graph layout/rendering module shared by both panes.
- Separate:
  1. visible-node preparation
  2. lane assignment
  3. per-row graph-cell generation
  4. Ratatui styling/rendering
- Keep graph layout independent from Ratatui where possible.

Tests:
- linear chain
- one branch
- branch and merge
- multiple roots/heads
- multiple parents
- folded snapshot group
- scrolling through a graph
- selected node rendering
- deterministic output across repeated runs
- missing parent outside the visible range must not panic

Acceptance criteria:
- Parent/child structure is visually understandable in both panes.
- Selecting and scrolling nodes still updates downstream panes correctly.
- Historical view remains read-only.
- Existing tests pass, clippy is clean, and no new panic path is introduced.
