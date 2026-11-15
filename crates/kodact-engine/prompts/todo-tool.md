Keep a structured task list for this session: what is planned, what is being done now, and what is finished. It keeps long work on track, shows the person your progress, and makes sure no requirement is forgotten. Send the whole list each time.

## When to use it

Use it proactively:
- for work with three or more distinct steps or actions;
- for work that needs planning or several changes, even when it looks simple at first;
- when the person asks for a task list;
- when the person gives several things to do (numbered, or in a list separated by commas);
- as soon as a new request arrives: write its requirements down as tasks;
- when you start a task: mark it `in_progress` before you begin, one task at a time;
- when you finish a task: mark it `completed` at once, and add the follow-up work you found on the way.

## When not to use it

- For a single, plain task.
- For something so small that tracking it helps nobody.
- For work done in fewer than three trivial steps.
- For a question or a conversation.

When there is one small thing to do, just do it.

## Examples where it helps

1. "Add CSV export to the reports page and make sure the tests pass." Several steps (the export function, the button, the download route, the tests), and an explicit request to verify: the list keeps the check from being forgotten at the end.
2. "Rename `load_config` to `read_settings` everywhere." A search finds 14 uses in 6 files: one task per file keeps every change accounted for.
3. "The import job is slow: find out why and fix it." Measuring finds three causes: one task per cause, worked through one at a time.

## Examples where it doesn't

1. "What does `git rebase --onto` do?" A question: answer it.
2. "Fix the typo in the README's title." One edit in one place.
3. "Run the test suite and tell me what fails." One command and a report; if fixes follow, then the list helps.

## Task states and rules

- `pending`: not started. `in_progress`: being worked on now, exactly one at a time. `completed`: finished.
- Each task has `content`, what to do in the imperative ("Run the tests"), and `activeForm`, the same in the present continuous ("Running the tests").
- Update the list as you work: mark each task `completed` as soon as it is done, not several at once later, and start the next by marking it `in_progress`.
- Mark a task `completed` only when it is fully done. While tests fail, the work is partial or an error is unresolved, keep it `in_progress` and add a task for what blocks it.
- Remove tasks that no longer apply.
- Split large work into specific tasks with clear names.

When unsure, use it: a task on the list costs little, and a forgotten requirement can cost the whole task.
