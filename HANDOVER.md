# Architecture implementation swarm — 2026-09-05

## Resume
Read this file, then `git status --short` in this repository and the engine worktree below. Do not repeat the architecture audit. User authorized implementing its findings with the cheapest available implementers and requested an early persistent handover to save restart context. Use gpt-5.6-luna agents, fork_turns=none, compact task briefs. No allowance/quota visibility exists.

## Repositories / safety
- App: `/Users/U765382/Developer/Private/rust-player`, branch `main` at start.
- Engine dependency: Git pin `466a35741544207475348e65e02e9c8cb96f384f` in Cargo.toml, source cached under `/Users/U765382/.cargo/git/checkouts/spotatui-bb3cc31e66475cd7/466a357` (read-only; never edit cache).
- Engine implementation worktree: `/Users/U765382/Developer/Private/spotatui-player-performance`, branch `player-performance`, based on the exact pin.
- Sibling `../spotatui` is on another feature branch. Do not edit it or existing unrelated worktrees.
- `.cargo/config.toml` NOW patches the Git engine dependency to `../spotatui-player-performance`; Cargo.lock updated accordingly. Workspace builds exercise local engine fixes. This is a coordinated-development override, NOT a production pin update: publish/review engine branch, update Cargo.toml revision, remove patch and regenerate lock before shipping.
- Existing untracked user file `architecture-review-20260901-232316.html`: preserve.
- Read AGENTS.md, CONTEXT.md and relevant docs/adr before edits. Engine also has nested instructions.
- No publishing, remote pushes or PRs performed. Local implementation and checks authorized.

## Audit findings / acceptance
1. Engine infra/network/playback.rs:start_playback trims URI lists to 100 before Native/Remote routing; adapter action_for_command sends full list. Move limit to API boundary; Native load retains all items. Also partial catalog pages become fixed implicit lists: completeness still needs design/implementation.
2. Engine runtime/pump.rs awaits Spotify operations serially, delaying transport behind catalog. Separate ordered transport/catalog work safely; do not blindly spawn all commands.
3. App main.rs:PlayerApp::send calls synchronous Runtime::command -> engine apply:block_on. Move UI dispatch to one FIFO background worker; preserve fold acknowledgement semantics (ADR0013).
4. UI changes search target immediately but renders old search_detail; detail lacks target identity. Album B can display/play album A tracks. Needs target/request identity across contract, engine and adapter.
5. Detail/search eager rows; Queue eager and not scrollable. Virtualize large track lists and Queue using existing uniform_list patterns.
6. Adapter map_search/map_library interprets global error notice as catalog failure; unrelated playback/persistence errors hide valid data. Preserve existing catalog data, use scoped errors where available.
7. Adapter owns implicit list and reconstructs cursor by track identity on each relay, starting from original selection. Duplicates / queued same-track identity / failed starts cause incorrect display. Engine should own authoritative list/cursor.
8. Text input ignores Option/Alt input, no IME or selection. Fix Alt text regression separately; full native text input remains follow-up.
9. Pump sync bridge recv_timeout(50ms) produces 20 idle wakeups/s. Replace with awaitable channel / explicit shutdown wake, preserving shutdown.

## Initial verification
`cargo test --locked --offline -p player-core -p player-spotatui` passed 32 tests before edits. No live UI/CPU profiling performed. Existing tests only assert adapter emits full list, miss engine truncation.

## Work allocation / current state
- Engine worktree CREATED at the path above, branch `player-performance`, exact base 466a357.
- Active Luna agents (medium reasoning, no inherited chat):
  - `ui_dispatch`: apps/player/src/main.rs plus optional command_dispatch.rs; one shared FIFO background worker, orderly shutdown, test queue responsiveness/order.
  - `adapter_errors`: crates/player-spotatui/src/lib.rs; retain valid catalog during unrelated notices, real DismissNotice action, focused tests.
  - `engine_lists`: engine src/infra/network/playback.rs; retain full native list, cap only API requests/fallbacks, regression tests.
- Wave 2 candidate: same UI agent virtualizes detail/Queue; engine bridge idle-wakeup fix if bounded. No agent owns core contract yet.
- UI FIFO dispatcher implemented; focused dispatcher test passed. Main reviewed and hardened test cleanup (release slow handler even when timeout fails). Final integrated checks pending.
- ui_dispatch agent now owns main.rs virtualization of album/playlist detail + Queue.
- Main implemented text_input.rs Option printable characters (e.g. ß) accepted while command chords remain ignored; pure regression test added, pending app test run. Full IME/selection remains unresolved.
- Adapter search heuristic was rejected in review: lifetime revision > 0 cannot establish current query success. Agent correcting cache invalidation and narrowing if scope unavailable.
- Engine list fix in progress; review requested cap at shared API payload boundary (cover fallbacks) and avoid misleading helper-only test claims.
- Agents must not commit, edit others' files, or format the entire repository.
- Engine URI fix complete: cap moved into shared api_playback_body, original list/offset reaches native loads and native fallback. Agent ran 52 focused streaming tests. Native helper test is not an end-to-end routing regression; boundary test covers API cap. Debug inspection of LoadRequest used because librespot context field is private.
- Adapter changes complete: nonempty search results and fetched library preserved despite notices; empty-search ambiguity remains due to engine global-only error field. 16 adapter tests passed. Library cache cleared whenever source data absent.
- UI list fix complete: finite flex viewport for album/playlist detail, virtualized Queue with fixed 44px truncated rows; artist detail/search categories remain eager. 16 app tests passed. No live GUI verification.
- Integrated `cargo test --offline --workspace` PASSED 52 tests (20 core, 16 adapter, 16 app) via local engine override. This predates upcoming identity/bridge edits. Known dead-code warnings, no failures.
- engine_lists agent investigating explicit shutdown sentinel for pump bridge instead of 7-file channel migration; no idle fix landed yet.
- adapter_errors agent NOW IMPLEMENTING detail identity across engine frontend/mod.rs, core SearchDetail/projector/fake, adapter, UI main.rs target filtering. Canonical identity must come from actual result metadata, not requested target. UI virtualization agent done and no longer editing main.rs.
- Completeness follow-up seam found: engine playlist_track_pages builds contiguous pages and retains page next/total; frontend SearchDetail loses these. Expose completion explicitly before treating list as complete; do not silently extend selected list on arbitrary catalog refresh.
- No native GUI smoke: Computer Use requires node_repl, which is absent from enabled tools. Native screenshot/interaction verification remains pending.
- Coordinator: integrate/test, record unresolved larger changes accurately. Avoid cheap agents doing broad architectural rewrites without a reviewed seam.

## Restart command
Tell Codex: `Continue the implementation swarm from HANDOVER.md. Use cheapest available implementers; preserve existing changes; update handover before stopping.`

## Resumed session — 2026-09-05
- Preserved all pre-existing app/engine changes and the user HTML artifact; no audit repeated.
- Resumed Luna tasks: `detail_identity` finishing partial identity/completeness implementation; `engine_bridge` reviewed/finished bridge; `cursor_seam` read-only investigation of authoritative cursor seam.
- Baseline workspace test exposed unfinished `target_locator` patterns/fixtures in core projector; identity agent owns correction. Do not treat previous 52-test pass as current verification.
- Bridge now blocks on `recv()` and uses an explicit `ShutdownPump` sentinel, with RAII wake/join cleanup; two focused pump tests passed. Coordinator reviewed diff. Catalog/transport split remains unresolved because both share mutable Spotify Network state.
- Coordinator reviewed UI FIFO dispatcher, Queue/detail virtualization, Option input, and shared API payload cap. No native GUI check available (`node_repl` absent).
- Cursor investigation confirmed no safe adapter-only fix: engine `core/app/native_recovery.rs::update_offset_for_track` itself matches URI identity (duplicates ambiguous); `frontend::Snapshot` exposes no authoritative occurrence index. Explicit Queue ownership already exists in engine transport/fold. Required next design: engine-owned list/request generation + occurrence index and confirmed/failed-start lifecycle, queue suspension/resumption, projection to adapter display metadata. Do not expose raw recovery offset as if authoritative. Investigation made no edits.
- Engine playback regression suite rerun with `cargo test --locked --offline --lib infra::network::playback::tests`: 52 passed (existing warnings only).
- Bridge regression tests hardened after review: timeout preserves join handle and sends fallback sentinel before awaiting cleanup, preventing a failed wake assertion from hanging Tokio teardown. Two pump tests still pass.
- Identity first pass reviewed; follow-up requested before final integration: keep Spotify locator normalization in adapter, not core (ADR 0006); base playlist completion on contiguous page coverage, not playable row count; add engine two-page/sparse/stale identity tests; explain partial single-track playback in UI.

## Remaining architecture work (not claimed fixed)
- Spotify catalog/transport separation: serial pump awaits the same mutable `Network`; preserve ordering/auth/recovery when designing separate ownership lanes.
- Authoritative Implicit Playback List occurrence cursor and failed-start rollback (see cursor investigation above).
- Completeness/pagination for search and library surfaces beyond the detail seam; fetching subsequent pages automatically is not implemented by the detail completion flag.
- Artist detail and categorized search results still render eagerly; album/playlist tracks and explicit Queue are virtualized.
- Scoped catalog failures: loaded data survives unrelated notices, but a successful empty search remains ambiguous under the global error field.
- Full text selection/IME/native text-input integration; only Option printable input regression is fixed.
- Native GUI scrolling/playback smoke and idle CPU measurement.
- Production engine publication/pin update/removal of local override (no remote actions authorized/performed in this run).

## Latest verification — 2026-09-07 (supersedes in-flight notes)
User requested restoring the app build after interrupting the prior run. The current code already builds; no additional source changes were needed this turn.
- `cargo build --locked --offline --workspace`: PASS, app binary built in dev profile.
- `cargo test --locked --offline --workspace`: PASS, 54 tests (6 core unit + 15 contract + 17 adapter + 16 app).
- Engine `cargo test --locked --offline --lib playlist_detail_publishes_contiguous_pages_and_ignores_stale_completions`: PASS. This previously unrun test exercises actual page folding/publication: sparse terminal page, middle episode, contiguous completion, actual playlist identity, revisions, stale generation rejection.
- Identity normalization now resides in adapter; core compares canonical locators exactly. Playlist detail uses contiguous track table and page coverage; partial detail rows explain individual playback.
- Existing unused/dead-code warnings remain. Local engine override remains required. No commits/pushes, packaging or GUI smoke performed.

## Isolated implementation — 2026-09-07
- User requires the main worktree to remain runnable. Do new implementation only in `/Users/U765382/Developer/Private/rust-player-search-empty-results` and `/Users/U765382/Developer/Private/spotatui-search-empty-results` (both branch `search-empty-results`). These include the earlier uncommitted baseline fixes; do not mistake all their diffs for this new task.
- Originals retain the previously verified baseline; the new empty-search fix was removed from originals. `cargo build --locked --offline -p rust-player` passed again after restoration.
- New task: expose presence of a fetched search page (including empty) in engine Snapshot; adapter preserves empty results during unrelated error notices. Focused validation underway in isolated worktrees. No publishing/merging.
- Isolated empty-search fix verified: 6 adapter search tests and 1 engine page-presence test passed. Original main app build passed after removing new task changes. No merge into main. Next work should continue in isolated worktrees.

## Search virtualization — 2026-09-07
- Implemented only in `rust-player-search-empty-results/apps/player/src/main.rs`: categorized search and artist detail now each use one lazy uniform_list; constant-size section metadata maps visible rows to category-local indices. No eager element list for these surfaces.
- Rows use a fixed 60px height with clipping/truncation; headings, navigation, album context menus, playback indices and enqueue actions retained.
- App tests passed: 17, including new section-boundary/empty-category indexing regression. `git diff --check` passed. Parent reviewed layout/indexing; GUI smoke remains pending.
- Checksum comparison confirmed all tracked files in original app and engine worktrees unchanged. Nothing merged or published.
- Remaining priorities: catalog/transport separation, authoritative occurrence-aware playback cursor, broader pagination, full IME, GUI/CPU verification. Empty-search preservation and eager search/artist rendering are now addressed in isolated worktrees.

## Merged tested changes — 2026-09-08
- Merged from isolated worktrees into the main working tree: lazy search/category and artist-detail rows, fixed row clipping/truncation, empty-search page presence in the engine snapshot, and adapter preservation of successful empty searches during unrelated errors.
- `cargo build --locked --offline --workspace`: PASS.
- `cargo test --locked --offline --workspace --quiet`: PASS (6 core, 15 contract, 18 adapter, 17 app tests; existing warnings only).
- Larger engine-owned occurrence cursor and catalog/transport lane split remain intentionally unmerged.
