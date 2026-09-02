# Agent Operating Rules

Reusable operating rules for Claude Code (or any coding agent) working on my projects.
Drop this file into a repository, or reference it from `CLAUDE.md`. The agent must follow
every applicable rule below exactly as written unless a higher-priority system, platform,
security, or user instruction conflicts with it.

---

## 1. Full autonomy — drive to a verified terminal state

- Once work starts, never pause merely to ask whether to proceed or end a turn on only a
  plan. The user may not be monitoring the session and may be unable to answer mid-run.
- Make sensible, reversible decisions; retry transient failures; route around blockers;
  and drive the task to a verified, finished result.
- Stop only when progress is truly impossible or unsafe without user input. Even then,
  finish everything that can be finished and leave a clear written status stating what is
  done, what is blocked, why it is blocked, and exactly what input is required.
- Where the execution environment supports timers, background processes, hooks, CI
  polling, or scheduled continuation, use them for long waits such as rate limits,
  deployments, or CI. Never falsely claim that monitoring or scheduled continuation
  exists when the environment cannot provide it.
- Autonomy never overrides security controls, production safeguards, cost limits,
  platform permissions, or explicit user constraints.

## 2. Questions policy — ask once, up front, then execute

- When task details arrive, produce the full implementation plan immediately.
- Ask all genuinely necessary clarifying questions in one batch immediately after the
  plan: ambiguities, missing credentials, decisions only the user can make, destructive
  actions requiring authorisation, and cost ceilings.
- Do not ask questions whose answers can be determined safely from the repository,
  documentation, connected services, tests, or sensible reversible defaults.
- After that single round, ask no further non-essential questions. Execute end-to-end.
- If a later blocker creates a new requirement for user authority, credentials, payment,
  destructive action, or an irreversible product decision, stop only that blocked part;
  finish all independent work and report the exact input required.

## 3. Quality bar — 100%, no exceptions

- Perfection is the target: correct, complete, maintainable, and secure, with applicable
  edge cases handled, including empty inputs, failures, retries, concurrency, bad data,
  boundary values, and interrupted operations.
- Nothing is done until it is verified: run the tests, run the application, check the
  deployment, inspect logs and errors, and adversarially review the output.
- Fast completion matters, but speed never trades against quality. Gain speed through
  parallelism, orchestration, reuse, and focused testing—not by cutting corners.
- Make no silent scope cuts. If anything is skipped, sampled, capped, deferred, or not
  applicable, state it explicitly with the reason in the final report.
- Do not over-engineer unrelated areas. Make the smallest complete change that satisfies
  the full requirement and preserves the existing architecture.

## 4. Model tiering — use the right model for the job

- The main session uses the strongest available model, including Fable when available.
  Use it for architecture, design decisions, difficult debugging, integration, final
  verification, and synthesis.
- Fan out independent subagent or workflow stages by difficulty when the environment and
  task permit it:
  - **Cheap models (Haiku/Sonnet):** mechanical work such as file scans, renames,
    boilerplate, formatting, and simple extraction.
  - **Opus:** mid-weight implementation and review work.
  - **Fable or strongest available equivalent:** hardest reasoning, design, adversarial
    verification, and synthesis.
- Efficiency is required, but every stage's output must be verified. Model tiering must
  never lower the quality bar.
- Do not invent model availability or claim that a model or subagent was used when it was
  not available.

## 5. Storage rules for this machine

- **Never intentionally store project work, downloads, build output, caches, or temporary
  files on `C:`.** It is nearly full, and filling it may terminate the coding process.
  Override default temporary paths under `C:\Users\<user>\AppData\Local\Temp` whenever
  the relevant tool allows it.
- **`E:` (Personal)** is the main work drive. Projects live under `E:\Project\`.
- **`G:` (TOSHIBA EXT, external HDD)** is temporary or bulk scratch storage only, at
  `G:\claude-tmp`. Never put the only copy of a final work product there.
- Other drives, including `D:` and `F:`, are nearly full and must not be used.
- Point downloads, build output, caches, package-manager stores, model files, and temporary
  files to `E:` or `G:\claude-tmp` wherever configurable.
- Before large downloads, builds, model installations, or dependency operations, verify
  sufficient free space on the target drive.
- Do not move or delete existing user data merely to free space without explicit
  authorisation.

## 6. Connected services

- **GitHub:** the CLI (`gh`) is authenticated. Creating private repositories is
  pre-approved when required by the task.
- **Cloudflare:** Workers, Pages, KV, D1, R2, and Wrangler may be used when required.
- Available through MCP when configured: Supabase, Neon, Stripe, Context7 documentation,
  and Chrome browser automation.
- Verify connectivity before relying on a service. Do not claim a service action succeeded
  without checking its returned state.
- Any additional service, account, permission, credential, or paid capability required by
  the task must be raised in the single up-front question batch.
- Connected access is permission to perform task-relevant operations, not permission for
  unrelated changes, purchases, destructive actions, or disclosure of sensitive data.

## 7. Session limits and scheduling

- If a usage or session limit is approaching or announced, preserve the current state in
  documentation and Git before the limit interrupts work.
- When the environment genuinely supports timers, monitors, automation, CI continuation,
  or scheduled tasks, use them to resume at the reset time.
- Queue incoming task details while waiting and begin when execution is available.
- If automatic continuation is unavailable, leave an exact resumable checkpoint containing
  the current branch and commit, completed requirements, verification evidence, remaining
  commands, blockers, and next action.
- Never claim that a timer, monitor, background process, or scheduled continuation has
  been created unless it was actually created and its state was verified.

## 8. Completion checklist — required before reporting done

1. Every requirement from the original task is implemented, including applicable edge
   cases.
2. Everything is verified by actually running the applicable tests, application,
   deployment checks, and review steps.
3. Work is committed and pushed to the correct repository where repository delivery is
   part of the task; repositories are private unless instructed otherwise.
4. No project work, downloads, caches, build output, or temp files were intentionally
   written to `C:`.
5. Secret scanning is clean before every push.
6. Documentation matches the finished implementation.
7. The final report plainly states what was built, how it was verified, what was skipped
   or blocked, and where everything lives, including relevant paths, repository URLs, and
   deployment URLs.

## 9. Git safety — every change must be recoverable

- Before substantial work, inspect the current branch, uncommitted changes, untracked
  files, latest commit, and remote configuration.
- Never destroy, overwrite, reset, or discard pre-existing user changes.
- If the working tree contains unrelated changes, preserve them and work around them. Do
  not silently include them in commits.
- For risky or large changes, use a dedicated branch or worktree.
- Create logical checkpoint commits throughout long implementations instead of one massive
  final commit.
- Never use destructive Git operations such as `reset --hard`, forced checkout, history
  rewriting, force push, or mass deletion unless they are strictly required and explicitly
  authorised.
- Before risky migrations, broad automated refactors, or production-affecting changes,
  create a recoverable checkpoint.
- Stage and commit only files belonging to the current task. Review the staged diff before
  every commit and the outgoing commits before every push.
- The final state must be reproducible from Git and must not depend on undocumented local
  modifications.

## 10. Secrets and credentials — zero leakage

- Never print, log, commit, screenshot, echo, transmit unnecessarily, or expose secrets.
- Never place secrets in source files, documentation, test fixtures, Git history, command
  transcripts, browser screenshots, generated reports, issue text, or commit messages.
- Use environment variables, approved secret stores, Wrangler secrets, GitHub secrets, or
  the platform's official secret mechanism.
- Before every push, run automated secret scanning across tracked changes and relevant
  history.
- Treat `.env`, `.dev.vars`, credential files, tokens, cookies, private keys, service
  account files, database URLs, webhook secrets, and API keys as sensitive.
- Redact sensitive values from logs and evidence while retaining enough context to debug.
- If a secret is accidentally exposed:
  1. stop propagating it;
  2. remove it from affected files and history where safely possible;
  3. document the exposure without repeating the value;
  4. identify exactly which credential must be revoked or rotated;
  5. verify the replacement is installed correctly after rotation.

## 11. Production safety

- Use the progression local → test → staging → production wherever those environments
  exist.
- Never test destructive behaviour against production.
- Never populate production with fake or test data unless explicitly required and safely
  isolated.
- Database migrations must be reviewed before execution, backward-compatible where
  practical, backed up or checkpointed, verified after execution, and supplied with a
  rollback or forward-recovery strategy.
- Do not delete production databases, buckets, domains, DNS records, Workers, KV
  namespaces, queues, secrets, users, or persistent data merely to fix a deployment.
- Production configuration changes must be verified after deployment.
- When staging exists, verify staging before deploying the same change to production.
- Use least privilege, preserve tenant isolation, and avoid logging private production
  data.
- If a production change is irreversible, destructive, unexpectedly billable, or outside
  explicit task scope, obtain authorisation before performing it.

## 12. No fake completion

The following do not count as implementation or verification:

- TODOs standing in for required functionality
- placeholder pages or content presented as final
- hard-coded success responses
- mocked production integrations presented as complete
- disabled, skipped, deleted, or quarantined tests used to obtain a pass
- weakened assertions or coverage thresholds
- commented-out validation or security controls
- swallowed exceptions or hidden failures
- fake health checks
- fabricated test output, screenshots, logs, links, metrics, or deployment evidence
- screenshots used instead of functional verification
- changing or narrowing requirements merely to make tests pass
- claiming commands or checks ran when they did not

If temporary scaffolding is necessary, clearly mark it and replace it before declaring the
task complete. If replacement is impossible, report it as blocked or incomplete.

## 13. Debugging policy — fix root causes

- Do not blindly retry failing commands indefinitely.
- For each material failure:
  1. capture the actual error without exposing secrets;
  2. reproduce it reliably where possible;
  3. identify the root cause using code, configuration, runtime state, logs, and tests;
  4. make the smallest correct fix;
  5. rerun the directly affected check;
  6. run relevant regression tests;
  7. verify the real user journey or deployed behaviour where applicable.
- Do not suppress errors merely to obtain a green build.
- Repeated failures require a changed diagnostic approach, not repetition of the same
  command.
- Record unresolved failures with reproduction steps, evidence, impact, and the precise
  next action.

## 14. Verification hierarchy

Use every layer applicable to the change:

1. static analysis and linting
2. formatting validation
3. type checking
4. unit tests
5. integration tests
6. API and contract tests
7. database and migration tests
8. browser and end-to-end tests
9. accessibility checks
10. security checks and secret scanning
11. build and packaging verification
12. deployment smoke tests
13. production health checks where applicable and authorised
14. adversarial review of edge cases, failure paths, and requirement coverage

A passing unit-test suite alone does not verify a full-stack feature. New behaviour must
receive new automated tests whenever reasonably testable. Bug fixes must receive regression
tests that demonstrate the failure and protect against its return. Never report a test as
passing based solely on its existence.

## 15. UI and browser verification

For user-facing web applications:

- Run the application and inspect it through browser automation.
- Verify major user journeys, not merely HTTP 200 responses.
- Test, where applicable:
  - desktop and mobile viewports
  - loading, empty, error, and success states
  - keyboard navigation and focus visibility
  - screen-reader semantics and obvious accessibility failures
  - form validation and boundary values
  - authentication and authorisation boundaries
  - direct route access, refreshes, and deep links
  - slow or failed network behaviour
- Check browser-console errors, hydration warnings, failed requests, and unexpected network
  calls.
- Where visual changes are substantial, capture screenshots as evidence, but do not treat
  screenshots as a substitute for functional verification.
- Verify reduced-motion, responsive layout, contrast, and overflow where relevant.

## 16. Dependency discipline

- Do not install a dependency when the existing stack already solves the problem cleanly.
- Before adding a dependency, verify why it is needed, its maintenance status, licence
  compatibility, security history, bundle or runtime cost, and compatibility with the
  current platform.
- Pin or lock dependencies using the project's existing package-management strategy.
- Never replace the project's package manager without an explicit requirement.
- Do not perform broad dependency upgrades unrelated to the task.
- Review lockfile changes and remove unused dependencies introduced by the work.
- Run appropriate vulnerability checks and distinguish exploitable production risk from
  irrelevant development-only findings.

## 17. Documentation and project memory

Keep documentation synchronized with implementation. For substantial projects, maintain
where applicable:

- architecture and system boundaries
- implementation plan and requirement checklist
- setup and local-development instructions
- environment-variable names and purpose, never secret values
- deployment and rollback procedures
- database schema and migrations
- testing strategy and commands
- operational runbook and incident recovery
- known limitations and remaining work
- decisions and architecture decision records (ADRs)
- development story and change history

Do not describe planned functionality as implemented functionality. Update documentation
during implementation, not only at the end. Commands and examples must be tested or clearly
labelled as illustrative.

## 18. Requirement traceability

- Convert the original task into an explicit requirement checklist before implementation.
- Give every requirement a stable identifier and acceptance evidence where useful.
- Every requirement must finish in exactly one of these states:
  - **IMPLEMENTED + VERIFIED** — include the verification evidence;
  - **BLOCKED** — include the reason, impact, completed surrounding work, and exact input
    needed;
  - **NOT APPLICABLE** — include the reason.
- Maintain the checklist throughout execution and update it as evidence is produced.
- Before final reporting, compare the finished system against the original request, not
  merely the latest implementation plan or the code that happened to be written.
- Treat later scope changes as explicit amendments; do not silently replace original
  requirements.

## 19. Cost and external-resource safety

- Prefer free or local resources when they satisfy the requirement and do not materially
  reduce quality, reliability, or security.
- Before creating infrastructure, check whether an appropriate existing resource can be
  reused safely.
- Do not enable paid tiers, purchase services, increase quotas, create large GPU jobs,
  provision unexpectedly billable infrastructure, or initiate significant API usage
  without explicit prior authorisation.
- Existing pre-approved normal development usage may proceed within stated cost ceilings.
- Configure budgets, limits, retention, or alerts where available and appropriate.
- Clean up temporary cloud resources created during testing unless they are required for
  the finished system or cleanup would destroy needed evidence.
- Report any recurring cost, pricing uncertainty, or resource left running in the final
  report.

## 20. Infrastructure reproducibility

- Represent infrastructure in version-controlled configuration wherever the platform
  permits it.
- Avoid undocumented dashboard-only configuration.
- Record required bindings, routes, DNS assumptions, databases, buckets, queues, service
  roles, environment-variable names, and secret names—never secret values.
- Separate development, staging, and production configuration and resources.
- Make provisioning and deployment operations idempotent where practical.
- A fresh developer or agent should be able to reconstruct the environment from repository
  documentation and configuration, subject only to obtaining authorised credentials.
- Detect and document material drift between version-controlled configuration and the live
  environment.

## 21. Parallel-agent coordination

- Parallelize independent work when useful and supported.
- Give every subagent or workflow a clearly defined scope, owned files or directories,
  required inputs, expected outputs, and verification responsibility.
- Two agents must not modify the same files concurrently unless the overlap is intentionally
  coordinated.
- The main agent owns requirements, architecture, integration, conflict resolution, and
  final verification.
- Never trust only a subagent's statement that something passed. Verify important outputs
  in the main workflow using actual code, diffs, tests, and runtime behaviour.
- Resolve conflicting recommendations using evidence from the original requirements, code,
  tests, documentation, security constraints, and observed runtime behaviour.
- Review every subagent change before integration and do not merge incomplete, unrelated,
  or unverified output.

---

*Last updated: 2026-09-02. Owner: leelaravind.*
