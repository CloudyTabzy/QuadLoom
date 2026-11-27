# 0021 — Contain backend OOM and aborts outside Blender

**Status:** implemented 2026-10-24; validation recorded below.

## Problem

Rust allocation failure aborts the process instead of unwinding. `catch_unwind`
protects against ordinary panics, but cannot protect Blender against an allocator
abort, stack overflow or a fatal dependency failure. The native attribute mapper
had the same exposure as the remesher. A cooperative cancel also cannot stop a
backend which never reaches its next checkpoint.

## Decision

The Blender operator always runs heavy native calls in a standalone Python
process, using Blender's bundled Python and the installed native wheel. A small
parent thread manages it; bpy is used only on Blender's main thread. The direct
native Python API and CLI retain their existing in-process behavior.

The child owns the stage cache. Quick Re-run reuses that process and cache until
the setting is disabled, its memory limit changes, the run fails/is cancelled,
or the scene/file/add-on lifecycle discards it. Changing mesh or upstream settings
still invalidates the native cache by its existing input key. Attribute mapping
runs in the same child, preserving seam/material constraints and transfer warnings.

**System → Worker Memory (MiB)** defaults to 2048, minimum 256. Each new worker is
capped at the smaller of the configured value and half the available memory
where measurable. Windows uses a Job Object process commit limit, with
kill-on-close. Unix sets `RLIMIT_AS` in the child. If limit installation fails,
the run fails; there is no in-process fallback. No console window is opened.

Small JSON messages/manifests refer to raw buffers in a private temporary
directory. This avoids pickle, base64 expansion and huge JSON arrays. Manifests
are bounded to 1 MiB and a maximum decoded depth of 32; buffer IDs are integers,
lengths must match the files, and total buffer bytes must fit half the worker's
budget in each direction. Files are published atomically. Input/output files are
removed after consumption; teardown waits for active file writers before cleanup.

The child reports progress independently of the native call. Cancel terminates
and reaps the child, even for uncooperative work. Parent exit closes the control
pipe; the child exits on EOF (and Windows also kills it through the Job Object).
An unexpected exit reports its code and a retry hint. Ordinary native failures
preserve their type name, stage and hint. A worker failure during mapping fails
the run rather than applying partially received geometry.

## Blender edge cases

- Completed but uncollected jobs still reserve the operator. They cannot race a
  newer run. Late cancellation is checked before result creation.
- Jobs hold the originating scene's identity, never a live bpy scene reference.
  Switching scenes discards the job; the result cannot land in another scene.
- Modal setup failure, undo/redo, file load and disable discard work. A headless
  timeout requests cancellation and cannot apply an unfinished result.
- Snapshot/output admission checks estimate host memory before large copies.
  Counts must fit Blender's signed 32-bit indices; all coordinates must be finite.
  Existing offset/range/repeated-corner checks remain. Frame patch IDs are checked
  before `bincount`, preventing a corrupt ID from requesting a huge host buffer.
- Intermediate construction tracks mesh allocations before an object exists,
  so an exception cannot leave an orphan mesh. The source remains unchanged.
- Initial Rust mesh buffers use `try_reserve_exact`, returning a stage-tagged
  error on refusal or capacity overflow. This is useful graceful recovery, not
  a claim that every dependency allocation is fallible.

## Limits of the guarantee

The process boundary contains backend aborts, not arbitrary Blender bugs.
Modifier evaluation, snapshotting, result construction and attribute application
still allocate inside Blender. Admission checks are conservative estimates and
can race other applications. No claim is made that an already exhausted system,
arbitrary modifiers or Blender's own allocator can never crash. Linux/macOS
limits are implemented but their Blender packaging remains unverified/deferred.

Process startup and disk transport add overhead, and cancellation loses the
Quick Re-run cache. The normal cache avoids another interpreter startup.

## Evidence

Full project gate passed on Windows / Blender 5.2.2:

- `cargo test --workspace --all-features`: 306 passed, 6 ignored.
- `cargo test --workspace`: 303 passed, 6 ignored.
- Both clippy feature configurations with `-D warnings`: clean.
- `python tools/build.py --dev`: wheel and extension built and validated.
- Python tests: 93 passed; Blender scenarios: 47 passed, including both GUI cases.

Tests exercise real OS allocation denial, a deliberate Rust 1 GiB allocation
under a 256 MiB limit (allocator abort), abrupt worker exit, parent death,
stalled-worker cancellation, recovery, mapping/cache reuse, invalid IPC and
geometry, scene switch, and intermediate-allocation failure. Fault injection is
development-build only; never distribute the dev extension.

A fresh review found two Important issues, both reproduced RED then fixed and
verified by the complete Python/Blender suites: validate every IPC descriptor
and admit cumulative decoded bytes before allocating any buffer; resolve the
Blender Python executable on the main thread, passing plain paths to the worker.
Minor deferred: renaming a scene during a run can leave its status text stale
after discard; worker termination and result rejection still work.

## Addendum 2026-11-26: progress must never fail a run

**Problem.** The first full benchmark run through the add-on since the worker
landed (70 meshes, released 0.4.0 zip) lost 11 runs to
`QuadLoom worker failed: [Errno 13] Permission denied: ...progress.json`.
Re-running those 11 one at a time on an idle machine lost 2 again: roughly one
run in six, on any mesh, more often on longer runs.

**Cause.** The child replaced `progress.json` about 20 times a second while the
parent polled it 50 times a second. Windows refuses to open a file in the middle
of being replaced, and refuses to replace a file another handle has open. Both
sides treated that as fatal: in the parent it failed the run and discarded the
worker and its Quick Re-run cache; in the child it killed the progress thread, so
the status line froze for the rest of the run. A two-second probe of the released
functions saw 69 refused reads and 4,659 refused replaces. The scenario and unit
suites missed it because their meshes finish in a few polls.

**Decision.** Progress is advisory, and the protocol now says so:

- `worker_protocol.read_progress` returns `None` for a file that is missing,
  busy, half-written or malformed; the parent keeps the last value it saw.
- `worker_protocol.publish_progress` skips a tick when the file is busy; the
  next tick publishes again.
- `publish` (manifests, which do matter) retries a refused rename with a
  doubling pause, about 0.6 s in all, before raising: a virus scanner holding a
  new file is the same transient.
- Removing a finished job's folder can no longer fail the run: the result is
  already read, and the session's folder is removed when the worker closes.

Integrity checks on input and output manifests are unchanged and stay strict.

**Evidence.** Eight new tests in `tests/python/test_worker.py`, each seen failing
on the released code first: the injected lock in a real run, the reader and the
writer alone, a real open handle (Windows), two readers racing a writer for half
a second, the manifest retry and its limit, and the cleanup failure. Full gate
green (321 / 318 Rust tests, both clippy configurations, 125 Python tests, 49
Blender scenarios). The two benchmark cases that failed twice completed with the
fix, giving 70 of 70.

**Lesson.** Run the whole benchmark through the add-on before a release, not
only the scenarios: it is the only test whose runs last long enough to meet the
real process boundary.

## Workflow choices

Work used a task branch in the existing checkout to preserve the installed
test/build paths. Progress was tracked manually. At the time there was no
top-level README; this decision holds the safety documentation.

## Primary references

- [Windows Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
- [Process commit limit](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information)
- [Python subprocess](https://docs.python.org/3/library/subprocess.html)
- [Rust fallible vector reservation](https://doc.rust-lang.org/std/vec/struct.Vec.html#method.try_reserve_exact)
