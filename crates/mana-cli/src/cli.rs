use clap::{Args, Parser, Subcommand};

/// Parse priority from "P0"-"P4", "p0"-"p4", or "0"-"4".
fn parse_priority(s: &str) -> Result<u8, String> {
    let num_str = s
        .strip_prefix('P')
        .or_else(|| s.strip_prefix('p'))
        .unwrap_or(s);
    let n: u8 = num_str
        .parse()
        .map_err(|_| format!("invalid priority '{s}': expected P0-P4 or 0-4"))?;
    if n > 4 {
        return Err(format!("priority {n} out of range: expected 0-4"));
    }
    Ok(n)
}

#[derive(Parser)]
#[command(
    name = "mana",
    about = "Agent work coordination and project memory",
    version,
    help_template = "\
{about-with-newline}
Usage: {usage}

Commands:
  WORK UNITS
    init         Initialize .mana/ in the current directory
    create       Create a new task, epic, or fact-backed unit [aliases: new]
    read         Display full unit details [aliases: show, view]
    list         List/search/filter units [aliases: ls]
    edit         Edit unit in $EDITOR
    update       Update fields, claim, set parent, add deps
    close        Close units (verify first), or --check to verify only
    delete       Delete a unit and clean up references

  QUERY
    status       Show project status: claimed, ready tasks, epics, and blocked units
    next         Recommend the best unit to work on next
    tree         Show hierarchical tree of units
    brief        Current-truth operational summary for a unit subtree
    context      Output context for a task, epic, or memory (no args)
    search       Search the mana system for a unit ID

  AGENTS
    run          Dispatch ready tasks to agents
    plan         Decompose an epic into smaller tasks
    agents       Show running and recently completed agents
    logs         View agent output from log files
    review       Post-close review of an implementation
    diff         Show git diff of what an agent changed

  MEMORY
    fact         Create a verified fact
    verify-facts Re-verify all facts, detect staleness

  MAINTENANCE
    tidy         Clean up: archive, release stale claims, close passing units
    groom       Propose project-management cleanup actions (dry-run only)
    doctor       Health check -- orphans, cycles, index freshness
    config       Manage project configuration
    stats        Project statistics

  OTHER
    mcp          MCP server for IDE integration
    move         Move units between projects
    onboard      Configure coding agents to use mana
    completions  Generate shell completions
  help         Print this message or the help of the given subcommand(s)

{options}
Getting started:
  mana init                                         Initialize .mana/ in this directory
  mana create \"fix bug\" --verify \"cargo test auth\"  Create a task with a verify gate
  imp run <unit-id>                                 Preferred single-unit execution path
  mana status                                       See what's in flight

See 'mana <command> --help' for details and examples."
)]
pub struct Cli {
    /// Use the outermost ancestor .mana instead of the nearest project .mana
    #[arg(long, global = true)]
    pub root: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    // -- TASKS --
    /// Initialize .mana/ in the current directory
    ///
    /// Creates .mana/ with config.yaml and sets up agent command templates.
    /// Agent presets (pi, claude, aider) auto-configure the run/plan commands.
    /// Use --setup on an existing project to reconfigure the agent.
    #[command(
        display_order = 1,
        after_help = "\
Examples:
  mana init                     Interactive setup
  mana init --agent pi          Use pi agent preset
  mana init --agent claude      Use Claude Code preset
  mana init myproject           Name the project explicitly
  mana init --setup             Reconfigure agent on existing project"
    )]
    Init {
        /// Project name (auto-detected from directory if omitted)
        name: Option<String>,

        /// Use a known agent preset (pi, claude, aider)
        #[arg(long)]
        agent: Option<String>,

        /// Custom run command template (use {id} for unit ID)
        #[arg(long)]
        run: Option<String>,

        /// Custom plan command template (use {id} for unit ID)
        #[arg(long)]
        plan: Option<String>,

        /// Reconfigure agent on existing project
        #[arg(long)]
        setup: bool,

        /// Skip agent setup
        #[arg(long)]
        no_agent: bool,
    },

    /// Configure coding agents to use mana (Claude Code, pi, Cursor, Aider, etc.)
    ///
    /// Scans the project for known coding-agent config files and writes mana
    /// integration instructions into each one. Idempotent — safe to run multiple times.
    ///
    /// Detected agents and their actions:
    ///   CLAUDE.md / .claude/settings.json → append workflow + status hook
    ///   .pi/                               → write .pi/agent/skills/mana/SKILL.md
    ///   .cursor/rules or .cursorrules      → append mana workflow rules
    ///   AGENTS.md                          → append mana workflow section
    ///   .cline/ or cline_docs/             → write mana.md
    ///   opencode.yaml or .opencode/        → write or patch mana config
    ///   .aider.conf.yml                    → append mana conventions comment
    ///   (none detected)                    → create AGENTS.md from scratch
    #[command(
        display_order = 2,
        after_help = "\
Examples:
  mana onboard                 Auto-detect and configure all coding agents
  mana onboard --dir ~/myproj  Configure agents in a specific project directory"
    )]
    Onboard {
        /// Project directory to configure (defaults to current directory)
        #[arg(long, default_value = ".")]
        dir: std::path::PathBuf,
    },

    /// Create a new task or epic
    ///
    /// Every unit needs a verify gate (--verify) — a shell command that must exit 0
    /// to close the unit. The --description is the agent's prompt when dispatched via
    /// a runtime path like `imp run <id>` or legacy `mana run`: include concrete steps,
    /// file paths, embedded types/signatures, and what NOT to do.
    ///
    /// Use -p (--pass-ok) when verify already passes (refactors, docs, type changes).
    /// Use --parent to create child units under a larger parent task.
    /// Use --produces/--requires to set up artifact-based dependency ordering.
    #[command(
        visible_alias = "new",
        display_order = 2,
        args_conflicts_with_subcommands = true,
        after_help = "\
Examples:
  mana create \"fix login bug\" --verify \"cargo test auth::login\"
  mana create \"add tests\" --verify \"pytest tests/auth.py\" -p
  mana create \"refactor API\" --verify \"cargo build\" --description \"## Task\\n...\"
  mana create \"add endpoint\" --parent 5 --verify \"cargo test\" --produces \"UserAPI\"
  mana create next \"step 2\" --verify \"cargo test\"   (auto-depends on last unit)

Verify patterns:
  Rust     cargo test module::test_name
  JS/TS    npx vitest run path/to/test
  Python   pytest tests/file.py -k test_name
  Go       go test ./pkg -run TestName
  Check    grep -q 'expected' file.txt
  Remove   ! grep -rq 'old_pattern' src/
  Multi    cmd1 && cmd2 && cmd3"
    )]
    Create {
        #[command(flatten)]
        args: Box<CreateOpts>,
    },

    /// Display full unit details
    ///
    /// Shows all fields: title, description, verify command, status, dependencies,
    /// history, and notes. Use --short for a one-line summary.
    #[command(visible_aliases = ["show", "view"], display_order = 4)]
    Read {
        /// Unit ID
        id: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Force human-readable output even when piped
        #[arg(long = "no-json", conflicts_with = "json")]
        no_json: bool,

        /// One-line summary
        #[arg(long)]
        short: bool,

        /// Show all history entries (default: last 10)
        #[arg(long)]
        history: bool,
    },

    /// List units with filtering
    ///
    /// By default shows open and in-progress units. Use --all to include closed.
    /// Combine filters to narrow results. Use --ids for piping to other commands.
    #[command(
        visible_alias = "ls",
        display_order = 5,
        after_help = "\
Examples:
  mana ls                              All open/in-progress units
  mana ls --all                        Include closed units
  mana ls --status in_progress         Only claimed units
  mana ls --label bug --priority 0     High-priority bugs
  mana ls --parent 5                   Children of unit 5
  mana ls --ids | xargs -I{} mana show {}   Pipe to other commands
  mana ls --format '{id}\\t{title}'     Custom output format"
    )]
    List {
        /// Filter by status (open, in_progress, closed)
        #[arg(long)]
        status: Option<String>,

        /// Filter by priority (P0-P4 or 0-4)
        #[arg(long, value_parser = parse_priority)]
        priority: Option<u8>,

        /// Show children of a parent
        #[arg(long)]
        parent: Option<String>,

        /// Filter by label
        #[arg(long)]
        label: Option<String>,

        /// Filter by assignee
        #[arg(long)]
        assignee: Option<String>,

        /// Show only units claimed by or created by the current user
        #[arg(long)]
        mine: bool,

        /// Include closed units
        #[arg(long)]
        all: bool,

        /// JSON output
        #[arg(long)]
        json: bool,

        /// Force human-readable output even when piped
        #[arg(long = "no-json", conflicts_with = "json")]
        no_json: bool,

        /// Output only unit IDs (one per line, for piping)
        #[arg(long, conflicts_with = "json")]
        ids: bool,

        /// Custom output format (e.g. '{id}\t{title}\t{status}')
        #[arg(long, conflicts_with_all = ["json", "ids"])]
        format: Option<String>,

        /// Search titles, descriptions, and notes by keyword
        #[arg(long)]
        search: Option<String>,
    },

    /// Search the mana system for a unit ID
    ///
    /// Searches the current mana ecosystem, including nested project `.mana/`
    /// directories, for an exact unit ID match.
    #[command(display_order = 12)]
    Search {
        /// Unit ID
        id: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Force human-readable output even when piped
        #[arg(long = "no-json", conflicts_with = "json")]
        no_json: bool,
    },

    /// Edit unit in $EDITOR
    #[command(display_order = 6)]
    Edit {
        /// Unit ID
        id: String,
    },

    /// Update unit fields
    ///
    /// Use --note to log progress during work. Notes are timestamped and appended —
    /// they survive retries, so the next agent reads what was tried and what failed.
    /// Essential for debugging repeated failures.
    #[command(
        display_order = 7,
        after_help = "\
Examples:
  mana update 5 --note \"Completed auth module, starting tests\"
  mana update 5 --note \"Failed: JWT lib incompatible. Avoid: jsonwebtoken 8.x\"
  mana update 5 --priority 0
  mana update 5 --title \"Revised scope\" --add-label bug"
    )]
    Update {
        /// Unit ID
        id: String,

        /// New title
        #[arg(long)]
        title: Option<String>,

        /// New description
        #[arg(long)]
        description: Option<String>,

        /// New acceptance criteria
        #[arg(long)]
        acceptance: Option<String>,

        /// Append a note (with timestamp separator)
        #[arg(long, visible_alias = "note")]
        notes: Option<String>,

        /// New design notes
        #[arg(long)]
        design: Option<String>,

        /// New status (open, in_progress, closed)
        #[arg(long)]
        status: Option<String>,

        /// New priority (P0-P4 or 0-4)
        #[arg(long, value_parser = parse_priority)]
        priority: Option<u8>,

        /// New assignee
        #[arg(long)]
        assignee: Option<String>,

        /// Add a label
        #[arg(long)]
        add_label: Option<String>,

        /// Remove a label
        #[arg(long)]
        remove_label: Option<String>,

        /// Add an unresolved decision/question (repeatable)
        #[arg(long = "decision")]
        decisions: Vec<String>,

        /// Resolve a decision by index (0-based) or by text match
        #[arg(long = "resolve-decision")]
        resolve_decisions: Vec<String>,

        /// Claim this unit for work (sets status to in_progress)
        #[arg(long)]
        claim: bool,

        /// Release claim on this unit (sets status back to open)
        #[arg(long, conflicts_with = "claim")]
        release: bool,

        /// Who is claiming (used with --claim)
        #[arg(long, requires = "claim")]
        by: Option<String>,

        /// Set parent unit ID
        #[arg(long)]
        parent: Option<String>,

        /// Add a dependency (this unit depends on the given ID)
        #[arg(long = "add-dep")]
        add_dep: Option<String>,

        /// Remove a dependency
        #[arg(long = "remove-dep")]
        remove_dep: Option<String>,
    },

    /// Close one or more units (runs verify gate first)
    ///
    /// Runs the unit's verify command first — if it exits 0, the unit is closed.
    /// If verify fails, the close is rejected unless --force is used.
    /// Multiple IDs can be passed to batch-close.
    ///
    /// Use --failed to mark an attempt as explicitly failed (agent giving up).
    /// The unit stays open and the claim is released for another agent to retry.
    #[command(
        display_order = 9,
        after_help = "\
Examples:
  mana close 5                              Close after verify passes
  mana close 5 6 7                          Batch close
  mana close --force 5                      Skip verify (force close)
  mana close --failed 5 --reason \"blocked\"  Mark attempt as failed
  mana ls --ids | mana close --stdin          Close all listed units"
    )]
    Close {
        /// Unit IDs (or use --stdin to read from pipe)
        #[arg(required_unless_present = "stdin")]
        ids: Vec<String>,

        /// Close reason
        #[arg(long)]
        reason: Option<String>,

        /// Skip verify command (force close)
        #[arg(long, conflicts_with = "failed")]
        force: bool,

        /// Mark attempt as failed (release claim, unit stays open)
        #[arg(long)]
        failed: bool,

        /// Skip verify and mark unit as awaiting_verify (for batch runner)
        ///
        /// Also activated automatically when MANA_BATCH_VERIFY=1 is set.
        /// The runner is responsible for running verify and finalizing the unit.
        #[arg(long, conflicts_with = "force", conflicts_with = "failed")]
        defer_verify: bool,

        /// Read unit IDs from stdin (one per line)
        #[arg(long)]
        stdin: bool,

        /// Run verify without closing (dry-run verify check)
        #[arg(
            long,
            conflicts_with = "force",
            conflicts_with = "failed",
            conflicts_with = "defer_verify"
        )]
        check: bool,
    },

    /// Run a unit's verify command without closing
    ///
    /// DEPRECATED: Use `mana close --check <id>` instead.
    #[command(display_order = 10, hide = true)]
    Verify {
        /// Unit ID
        id: String,

        /// Output result as JSON
        #[arg(long)]
        json: bool,

        /// Force human-readable output even when piped
        #[arg(long = "no-json", conflicts_with = "json")]
        no_json: bool,

        /// Suppress informational output
        #[arg(long, short = 'q')]
        quiet: bool,
    },

    /// Reopen a closed unit
    ///
    /// DEPRECATED: Use `mana update <id> --status open` instead.
    #[command(display_order = 11, hide = true)]
    Reopen {
        /// Unit ID
        id: String,
    },

    /// Delete a unit and clean up references
    #[command(display_order = 12)]
    Delete {
        /// Unit ID
        id: String,
    },

    // -- DEPENDENCIES --
    /// Manage dependencies between units
    ///
    /// DEPRECATED: Use `mana update <id> --add-dep/--remove-dep` instead.
    #[command(display_order = 30, hide = true)]
    Dep {
        #[command(subcommand)]
        command: DepCommand,
    },
    // -- QUERY --
    /// Show project status: claimed, ready tasks, epics, and blocked units
    ///
    /// Quick overview of what's in flight, what's ready for dispatch, and what's
    /// waiting on dependencies. Start here to understand project state.
    #[command(display_order = 20)]
    Status {
        /// JSON output
        #[arg(long)]
        json: bool,

        /// Force human-readable output even when piped
        #[arg(long = "no-json", conflicts_with = "json")]
        no_json: bool,
    },

    /// Recommend the single best unit to work on next
    ///
    /// Scores ready units by priority, dependency depth (unblocks), age, and attempt
    /// count. Returns the top-scored unit — the answer to "what should I work on?"
    #[command(
        display_order = 21,
        after_help = "\
Examples:
  mana next                Show the single best unit to work on
  mana next -n 3           Show top 3 recommendations
  mana next --json         Machine-readable JSON output"
    )]
    Next {
        /// Number of recommendations to show (default: 1)
        #[arg(short = 'n', long, default_value = "1")]
        count: usize,

        /// JSON output
        #[arg(long)]
        json: bool,

        /// Force human-readable output even when piped
        #[arg(long = "no-json", conflicts_with = "json")]
        no_json: bool,
    },

    /// Current-truth operational summary for a unit subtree
    #[command(
        display_order = 22,
        after_help = "\
Examples:
  mana brief 335       Summarize current truth for unit 335 and descendants"
    )]
    Brief {
        /// Root unit ID to summarize
        id: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Force human-readable output even when piped
        #[arg(long = "no-json", conflicts_with = "json")]
        no_json: bool,
    },

    /// Output context for a task, epic, or memory context (no args)
    ///
    /// With a unit ID: outputs complete agent context — unit spec, verify command,
    /// previous attempts, project rules, dependency context, and referenced file
    /// contents. This is the single source of truth for an agent working on a unit.
    ///
    /// Without an ID: outputs memory context — stale facts, currently claimed units,
    /// and recent completions.
    ///
    /// File paths come from the unit's `paths` field (set via --paths on create) plus
    /// any file paths mentioned in the description text (regex-extracted). Explicit
    /// paths take priority.
    #[command(
        display_order = 25,
        after_help = "\
Examples:
  mana context         Memory context (stale facts, in-progress, recent work)
  mana context 5       Complete agent context for unit 5
  mana context 5 --structure-only   Signatures only (skip file contents)
  mana context 5 --json             Machine-readable output"
    )]
    Context {
        /// Unit ID (omit for memory context)
        id: Option<String>,

        /// Output as JSON (file paths and contents)
        #[arg(long)]
        json: bool,

        /// Force human-readable output even when piped
        #[arg(long = "no-json", conflicts_with = "json")]
        no_json: bool,

        /// Output only the structural summary (signatures, imports) — skip full file contents
        #[arg(long)]
        structure_only: bool,

        /// Output the full structured agent prompt (what an agent sees during mana run)
        #[arg(long, conflicts_with = "structure_only")]
        agent_prompt: bool,

        /// Instructions to prepend to the user message (used with --agent-prompt)
        #[arg(long, requires = "agent_prompt")]
        instructions: Option<String>,

        /// Concurrent file overlaps as JSON (used with --agent-prompt)
        /// Format: [{"unit_id":"5","title":"Other","shared_files":["src/main.rs"]}]
        #[arg(long, requires = "agent_prompt")]
        overlaps: Option<String>,
    },

    /// Show hierarchical tree of units
    #[command(display_order = 23)]
    Tree {
        /// Root unit ID (shows full tree if omitted)
        id: Option<String>,
    },

    /// Display dependency graph
    #[command(display_order = 24)]
    Graph {
        /// Output format: ascii (default), mermaid, dot
        #[arg(long, default_value = "ascii")]
        format: String,
    },

    // -- MAINTENANCE --
    /// Force rebuild index from YAML files
    ///
    /// DEPRECATED: Use `mana tidy` instead (rebuilds index + cleans up state).
    #[command(display_order = 41, hide = true)]
    Sync,

    /// Archive closed units, release stale in-progress units, and rebuild the index
    #[command(display_order = 40)]
    Tidy {
        /// Show what would happen without changing any files
        #[arg(long)]
        dry_run: bool,

        /// Suppress informational output
        #[arg(long, short = 'q')]
        quiet: bool,
    },

    /// Project statistics
    #[command(display_order = 43)]
    Stats {
        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Force human-readable output even when piped
        #[arg(long = "no-json", conflicts_with = "json")]
        no_json: bool,
    },

    /// Claim a unit for work (sets status to in_progress)
    ///
    /// DEPRECATED: Use `mana update <id> --claim` instead.
    #[command(display_order = 8, hide = true)]
    Claim {
        /// Unit ID
        id: String,

        /// Release the claim instead of acquiring it
        #[arg(long)]
        release: bool,

        /// Who is claiming (agent name or user)
        #[arg(long)]
        by: Option<String>,

        /// Force claim even if verify already passes
        #[arg(long)]
        force: bool,
    },

    /// Propose project-management cleanup actions (dry-run only)
    #[command(display_order = 42)]
    Groom {
        /// Root unit ID to inspect
        id: String,

        /// Show proposals without applying changes (required; apply is not implemented)
        #[arg(long)]
        dry_run: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Force human-readable output even when piped
        #[arg(long = "no-json", conflicts_with = "json")]
        no_json: bool,
    },

    /// Health check -- index, dependency graph, and stale/misleading config
    #[command(display_order = 43)]
    Doctor {
        #[command(subcommand)]
        command: Option<DoctorCommand>,
    },

    /// Manage hook trust (enable/disable hook execution)
    #[command(display_order = 45)]
    Trust {
        /// Revoke trust (disable hooks)
        #[arg(long)]
        revoke: bool,

        /// Check current trust status
        #[arg(long)]
        check: bool,
    },

    /// Unarchive a unit (move from archive back to main units directory)
    #[command(display_order = 46)]
    Unarchive {
        /// Unit ID to unarchive
        id: String,
    },

    /// View and manage file locks for concurrent agents
    #[command(display_order = 46)]
    Locks {
        /// Force-clear all locks
        #[arg(long)]
        clear: bool,
    },

    /// Quick-create: create a unit and immediately claim it
    ///
    /// DEPRECATED: Use `mana create --claim` instead.
    #[command(
        visible_alias = "q",
        display_order = 3,
        hide = true,
        after_help = "\
Examples:
  mana quick \"fix typo in README\" --verify \"grep -q 'correct text' README.md\"
  mana quick \"add logging\" -p   (verify already passes — refactor)"
    )]
    Quick {
        /// Unit title
        title: String,

        /// Full description / agent context
        #[arg(long)]
        description: Option<String>,

        /// Acceptance criteria
        #[arg(long)]
        acceptance: Option<String>,

        /// Additional notes
        #[arg(long)]
        notes: Option<String>,

        /// Shell command that must exit 0 to close
        #[arg(long)]
        verify: Option<String>,

        /// Priority P0-P4 or 0-4 (default: P2)
        #[arg(long, value_parser = parse_priority)]
        priority: Option<u8>,

        /// Who is claiming (agent name or user)
        #[arg(long)]
        by: Option<String>,

        /// Comma-separated artifacts this unit produces
        #[arg(long)]
        produces: Option<String>,

        /// Comma-separated artifacts this unit requires
        #[arg(long)]
        requires: Option<String>,

        /// Parent unit ID (creates child unit under parent)
        #[arg(long)]
        parent: Option<String>,

        /// Action on verify failure: retry, retry:N, escalate, escalate:P0
        #[arg(long)]
        on_fail: Option<String>,

        /// Skip fail-first check (allow verify to already pass)
        #[arg(long, short = 'p')]
        pass_ok: bool,

        /// Timeout in seconds for the verify command (kills process on expiry)
        #[arg(long)]
        verify_timeout: Option<u64>,

        /// Skip duplicate title check
        #[arg(long)]
        force: bool,
    },

    /// Move units between .mana/ directories
    ///
    /// Use when units were accidentally created in the wrong directory (e.g. ~ instead
    /// of the project). Units get new sequential IDs in the destination. Parent/dependency
    /// references are cleared since they refer to the source project's ID space.
    ///
    /// Use --from to pull units into this project, or --to to push units out.
    #[command(
        display_order = 13,
        after_help = "\
Examples:
  mana move --from ~/.mana 42 43 44         Pull units from ~/.mana/ into this project
  mana move --from ~/other-project 1 2       Pull from another project
  mana move --to ~/other-project 5 6         Push units from this project elsewhere"
    )]
    Move {
        /// Pull units FROM this .mana/ or project directory into the current project
        #[arg(long, conflicts_with = "to", required_unless_present = "to")]
        from: Option<String>,

        /// Push units from the current project TO this .mana/ or project directory
        #[arg(long, conflicts_with = "from", required_unless_present = "from")]
        to: Option<String>,

        /// Unit IDs to move
        #[arg(required = true)]
        ids: Vec<String>,
    },

    /// Adopt existing units as children of a parent
    ///
    /// DEPRECATED: Use `mana update <child> --parent <parent>` instead.
    #[command(display_order = 31, hide = true)]
    Adopt {
        /// Parent unit ID
        parent: String,

        /// Unit IDs to adopt as children
        #[arg(required = true)]
        children: Vec<String>,
    },

    /// Dispatch ready tasks to agents
    ///
    /// Without an ID, finds all ready units (open, no unresolved deps) and spawns
    /// agents in parallel up to -j limit. With an ID, dispatches that specific unit.
    /// Agents run the command template from .mana/config.yaml (set via `mana init`).
    ///
    /// Use --loop-mode for continuous dispatch until all work is done — it re-scans
    /// for newly-ready units after each wave completes.
    #[command(after_help = "\
Examples:
  mana run              Dispatch all ready units (up to -j 4 parallel)
  mana run 5            Dispatch a specific unit
  mana run --loop-mode  Keep going until no ready units remain
  mana run --dry-run    Preview what would be dispatched
  mana run -j 8 --keep-going --timeout 60   High-throughput mode")]
    Run {
        /// Unit ID. Without ID, processes all ready units.
        id: Option<String>,

        /// Max parallel agents
        #[arg(short = 'j', long, default_value = "4")]
        jobs: u32,

        /// Show plan without spawning
        #[arg(long)]
        dry_run: bool,

        /// Keep running until no ready units remain
        #[arg(long, name = "loop")]
        loop_mode: bool,

        /// Continue past failures
        #[arg(long)]
        keep_going: bool,

        /// Max time per agent in minutes
        #[arg(long, default_value = "30")]
        timeout: u32,

        /// Kill agent if no output for N minutes
        #[arg(long, default_value = "5")]
        idle_timeout: u32,

        /// Emit JSON stream events to stdout (for programmatic consumers)
        #[arg(long)]
        json_stream: bool,

        /// Run adversarial review after each successful close
        #[arg(long)]
        review: bool,
    },

    /// Decompose an epic into smaller tasks
    ///
    /// Breaks a unit into smaller child units with proper dependencies.
    /// Each child should be completable by a fast, non-thinking model
    /// in a single pass.
    #[command(after_help = "\
Examples:
  mana plan 5                    Decompose unit 5 into children
  mana plan 5 --strategy layer   Suggest layer-based split
  mana plan 5 --auto             Non-interactive (no prompts)
  mana plan 5 --dry-run          Preview without creating children")]
    Plan {
        /// Unit ID to decompose
        id: String,

        /// Suggest a split strategy (feature, layer, phase, file)
        #[arg(long)]
        strategy: Option<String>,

        /// Non-interactive autonomous planning
        #[arg(long)]
        auto: bool,

        /// Show proposed split without creating
        #[arg(long)]
        dry_run: bool,
    },

    /// Show running and recently completed agents
    #[command(display_order = 37)]
    Agents {
        /// JSON output
        #[arg(long)]
        json: bool,

        /// Force human-readable output even when piped
        #[arg(long = "no-json", conflicts_with = "json")]
        no_json: bool,
    },

    /// View agent output from log files
    ///
    /// Shows the agent's stdout/stderr from its most recent run. Use --all to see
    /// output from all runs (helpful when debugging repeated failures). Use -f to
    /// follow live output while an agent is running.
    #[command(
        display_order = 38,
        after_help = "\
Examples:
  mana logs 5          Latest run output
  mana logs 5 --all    All runs (for debugging retries)
  mana logs 5 -f       Follow live output"
    )]
    Logs {
        /// Unit ID
        id: String,

        /// Follow output (tail -f)
        #[arg(short, long)]
        follow: bool,

        /// Show all runs, not just latest
        #[arg(long)]
        all: bool,
    },

    // -- AGENTS --
    /// Manage project configuration
    #[command(
        display_order = 35,
        after_help = "Examples:
  mana config get run_model
  mana config set run_model gpt-5.3-codex
  mana config set plan_model claude-sonnet-4-6
  mana config set review_model haiku
  mana config set research_model gpt-5.4

Model keys:
  run_model       Default model for legacy mana run compatibility flows
  plan_model      Default model for mana plan
  review_model    Default model for AI review flows
  research_model  Default model for project-level research/planning"
    )]
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },

    // -- MCP --
    /// MCP server for IDE integration (Cursor, Windsurf, Claude Desktop, Cline, etc.)
    #[command(display_order = 60)]
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },

    // -- MEMORY --
    /// Manage verified facts and project fact sheets
    ///
    /// Without a subcommand, creates a fact unit for compatibility. Use
    /// `mana fact check` to validate root `facts.mana` and existing fact units.
    #[command(
        display_order = 50,
        args_conflicts_with_subcommands = true,
        after_help = "\
Examples:
  mana fact \"API uses Axum 0.8\" --verify \"grep -q 'axum = \\\"0.8' Cargo.toml\"
  mana fact \"Auth tokens expire after 24h\" --verify \"grep -q '24 * 60' src/config.rs\"
  mana fact check
  mana fact check --json"
    )]
    Fact {
        #[command(subcommand)]
        command: Option<FactCommand>,

        #[command(flatten)]
        args: FactArgs,
    },

    /// Search units by keyword
    ///
    /// DEPRECATED: Use `mana list --search <query>` instead.
    #[command(
        display_order = 51,
        hide = true,
        after_help = "\
Examples:
  mana recall \"auth\"           Search open units
  mana recall \"JWT\" --all      Include closed/archived
  mana recall \"login\" --json   Machine-readable results"
    )]
    Recall {
        /// Search query
        query: String,

        /// Include closed/archived units
        #[arg(long)]
        all: bool,

        /// JSON output
        #[arg(long)]
        json: bool,

        /// Force human-readable output even when piped
        #[arg(long = "no-json", conflicts_with = "json")]
        no_json: bool,
    },

    /// Re-verify all facts, detect staleness
    #[command(display_order = 52, name = "verify-facts")]
    VerifyFacts,

    // -- TRACE --
    /// Walk unit lineage and dependency chain
    ///
    /// Shows the full context for a unit: parent chain up to root, direct children,
    /// what it depends on, what depends on it, artifacts produced/required, and
    /// a summary of all agent attempts.
    #[command(
        display_order = 41,
        after_help = "\
Examples:
  mana trace 7.3              Show full trace for unit 7.3
  mana trace 7.3 --json       Machine-readable JSON output"
    )]
    Trace {
        /// Unit ID to trace
        id: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Force human-readable output even when piped
        #[arg(long = "no-json", conflicts_with = "json")]
        no_json: bool,
    },

    /// Show git diff of what an agent changed for a unit
    ///
    /// Finds commits associated with a unit and shows their diff. Works with:
    /// - Auto-commit: finds commits with `unit-{id}` in the message
    ///   (plus legacy `Close unit {id}` commits)
    /// - Checkpoint: uses the checkpoint SHA recorded at claim time
    /// - Timestamps: falls back to diffing between claim and close times
    ///
    /// For open/in-progress units, diffs to HEAD (shows current working changes).
    #[command(
        display_order = 39,
        after_help = "\
Examples:
  mana diff 3             Show what changed for unit 3
  mana diff 3 --stat      Summary only (files changed, insertions, deletions)
  mana diff 3 --name-only Just filenames
  mana diff 3 --no-color  Disable color (for piping)
  mana diff 3 | delta     Pipe to your preferred diff viewer"
    )]
    Diff {
        /// Unit ID
        id: String,

        /// Show file-level summary instead of full diff
        #[arg(long)]
        stat: bool,

        /// Show only filenames that changed
        #[arg(long)]
        name_only: bool,

        /// Disable color output
        #[arg(long)]
        no_color: bool,
    },

    /// Mutation-test a unit's verify gate
    ///
    /// After confirming verify passes on clean code, mutates the git diff
    /// (flips operators, swaps booleans, deletes lines) and re-runs verify.
    /// Surviving mutants indicate a weak verify gate that doesn't catch changes.
    #[command(
        display_order = 36,
        after_help = "\
Examples:
  mana mutate 5                       Test verify strength for unit 5
  mana mutate 5 --max 10              Test at most 10 mutants
  mana mutate 5 --timeout 30          30s timeout per verify run
  mana mutate 5 --diff-base HEAD~3    Diff against 3 commits ago
  mana mutate 5 --json                Machine-readable output"
    )]
    Mutate {
        /// Unit ID
        id: String,

        /// Maximum number of mutants to test (0 = all)
        #[arg(long, default_value = "0")]
        max: usize,

        /// Timeout per verify run in seconds
        #[arg(long)]
        timeout: Option<u64>,

        /// Git ref to diff against (default: HEAD)
        #[arg(long, default_value = "HEAD")]
        diff_base: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Force human-readable output even when piped
        #[arg(long = "no-json", conflicts_with = "json")]
        no_json: bool,
    },

    /// Adversarial post-close review of a unit's implementation
    ///
    /// Spawns a review agent with the unit's spec + current git diff as context.
    /// The review agent outputs a verdict: approve, request-changes, or flag.
    #[command(
        display_order = 39,
        after_help = "\
Examples:
  mana review 5                     Run AI adversarial review for unit 5
  mana review 5 --diff              Review only the current git diff
  mana review 5 --model claude      Review with a specific model"
    )]
    Review {
        /// Unit ID to review
        id: Option<String>,

        /// Approve the unit (reserved; human review UI is archived)
        #[arg(long, hide = true)]
        approve: bool,

        /// Request changes with feedback message (reserved; human review UI is archived)
        #[arg(long, hide = true, value_name = "FEEDBACK")]
        request_changes: Option<String>,

        /// Reject the unit with a reason (reserved; human review UI is archived)
        #[arg(long, hide = true, value_name = "REASON")]
        reject: Option<String>,

        /// Reserved for compatibility; agent review is the only active mode
        #[arg(long, hide = true)]
        agent: bool,

        /// Include only the git diff
        #[arg(long)]
        diff: bool,

        /// Override model
        #[arg(long)]
        model: Option<String>,
    },

    // -- SHELL COMPLETIONS --
    /// Generate shell completions
    ///
    /// Prints a completion script to stdout. Add to your shell's rc file:
    ///   bash:  eval "$(mana completions bash)"
    ///   zsh:   eval "$(mana completions zsh)"
    ///   fish:  mana completions fish | source
    #[command(
        display_order = 70,
        after_help = "\
Examples:
  mana completions bash              Print bash completions
  mana completions zsh               Print zsh completions
  mana completions fish              Print fish completions
  mana completions powershell        Print PowerShell completions

Install permanently:
  bash:  echo 'eval \"$(mana completions bash)\"' >> ~/.bashrc
  zsh:   echo 'eval \"$(mana completions zsh)\"' >> ~/.zshrc
  fish:  mana completions fish > ~/.config/fish/completions/mana.fish"
    )]
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Args)]
pub struct FactArgs {
    /// Fact title (what is true)
    pub title: Option<String>,

    /// Shell command that verifies this fact (required when creating a fact unit)
    #[arg(long)]
    pub verify: Option<String>,

    /// Description / additional context
    #[arg(long)]
    pub description: Option<String>,

    /// Comma-separated file paths this fact is relevant to
    #[arg(long)]
    pub paths: Option<String>,

    /// Time-to-live in days before fact becomes stale (default: 30)
    #[arg(long)]
    pub ttl: Option<i64>,

    /// Skip fail-first check
    #[arg(long, short = 'p')]
    pub pass_ok: bool,
}

#[derive(Debug, Subcommand)]
pub enum FactCommand {
    /// Check facts.mana and re-verify existing fact units
    Check {
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,

        /// Force human-readable output even when piped
        #[arg(long = "no-json", conflicts_with = "json")]
        no_json: bool,
    },
}

#[derive(Subcommand)]
pub enum DepCommand {
    /// Add a dependency (id depends on depends-on-id)
    Add {
        /// Unit ID that will have the dependency
        id: String,

        /// Unit ID that must be completed first
        depends_on: String,
    },

    /// Remove a dependency
    Remove {
        /// Unit ID
        id: String,

        /// Dependency to remove
        depends_on: String,
    },

    /// Show dependencies and dependents of a unit
    List {
        /// Unit ID
        id: String,
    },
}

#[derive(Subcommand)]
pub enum DoctorCommand {
    /// Automatically fix safe, deterministic issues
    Fix,
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Get the effective value for a configuration key
    Get {
        /// Config key. Model keys: run_model (mana run), plan_model (mana plan), review_model (AI review), research_model (project research/planning).
        key: String,
    },

    /// Get the raw project-local value for a configuration key
    GetProject {
        /// Config key.
        key: String,
    },

    /// Get the raw global value for a configuration key
    GetGlobal {
        /// Config key.
        key: String,
    },

    /// Inspect effective/local/global values and source information
    Inspect {
        /// Optional config key. Omit to inspect common runtime settings.
        key: Option<String>,
    },

    /// Detect stale or misleading local config overrides
    Doctor,

    /// Set a project-local configuration value
    Set {
        /// Config key. Model keys: run_model (mana run), plan_model (mana plan), review_model (AI review), research_model (project research/planning).
        key: String,

        /// New value
        value: String,
    },

    /// Set a project-local configuration value explicitly
    SetProject {
        /// Config key.
        key: String,

        /// New value
        value: String,
    },

    /// Set a global configuration value in ~/.config/mana/config.yaml
    SetGlobal {
        /// Config key.
        key: String,

        /// New value
        value: String,
    },
}

#[derive(Subcommand)]
pub enum McpCommand {
    /// Start MCP server on stdio (JSON-RPC 2.0)
    Serve,
}

#[derive(Subcommand)]
pub enum CreateSubcommand {
    /// Create a unit that depends on the most recently created unit (sequential chaining)
    ///
    /// Automatically adds a dependency on @latest, enabling easy sequential chains:
    ///   mana create "Step 1" -p
    ///   mana create next "Step 2" --verify "cargo test step2"
    ///   mana create next "Step 3" --verify "cargo test step3"
    Next {
        /// Unit title
        title: Option<String>,

        /// Unit title (alternative to positional arg)
        #[arg(long, conflicts_with = "title")]
        set_title: Option<String>,

        /// Full description / agent context
        #[arg(long)]
        description: Option<String>,

        /// Acceptance criteria
        #[arg(long)]
        acceptance: Option<String>,

        /// Additional notes
        #[arg(long)]
        notes: Option<String>,

        /// Design decisions
        #[arg(long)]
        design: Option<String>,

        /// Shell command that must exit 0 to close
        #[arg(long)]
        verify: Option<String>,

        /// Parent unit ID -- child gets next dot-number
        #[arg(long)]
        parent: Option<String>,

        /// Priority P0-P4 or 0-4 (default: P2)
        #[arg(long, value_parser = parse_priority)]
        priority: Option<u8>,

        /// Comma-separated labels
        #[arg(long)]
        labels: Option<String>,

        /// Assignee name
        #[arg(long)]
        assignee: Option<String>,

        /// Additional comma-separated dependency IDs (merged with auto @latest dep)
        #[arg(long)]
        deps: Option<String>,

        /// Comma-separated artifacts this unit produces
        #[arg(long)]
        produces: Option<String>,

        /// Comma-separated artifacts this unit requires
        #[arg(long)]
        requires: Option<String>,

        /// Comma-separated file paths relevant to this unit (used by mana context)
        #[arg(long)]
        paths: Option<String>,

        /// Action on verify failure: retry, retry:N, escalate, escalate:P0
        #[arg(long)]
        on_fail: Option<String>,

        /// Skip fail-first check (allow verify to already pass)
        #[arg(long, short = 'p')]
        pass_ok: bool,

        /// Timeout in seconds for the verify command (kills process on expiry)
        #[arg(long)]
        verify_timeout: Option<u64>,

        /// Claim the unit immediately (sets status to in_progress)
        #[arg(long, conflicts_with = "run")]
        claim: bool,

        /// Who is claiming (requires --claim)
        #[arg(long, requires = "claim")]
        by: Option<String>,

        /// Spawn an agent to work on this unit (requires --verify)
        #[arg(long)]
        run: bool,

        /// Mark the new unit as an epic instead of a task (--epic)
        #[arg(long)]
        epic: bool,

        /// Output created unit as JSON (for piping)
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Args)]
pub struct CreateOpts {
    #[command(subcommand)]
    pub subcommand: Option<CreateSubcommand>,

    /// Unit title
    pub title: Option<String>,

    /// Unit title (alternative to positional arg)
    #[arg(long, conflicts_with = "title")]
    pub set_title: Option<String>,

    /// Full description / agent context
    #[arg(long)]
    pub description: Option<String>,

    /// Acceptance criteria
    #[arg(long)]
    pub acceptance: Option<String>,

    /// Additional notes
    #[arg(long)]
    pub notes: Option<String>,

    /// Design decisions
    #[arg(long)]
    pub design: Option<String>,

    /// Shell command that must exit 0 to close
    #[arg(long)]
    pub verify: Option<String>,

    /// Parent unit ID -- child gets next dot-number
    #[arg(long)]
    pub parent: Option<String>,

    /// Priority P0-P4 or 0-4 (default: P2)
    #[arg(long, value_parser = parse_priority)]
    pub priority: Option<u8>,

    /// Comma-separated labels
    #[arg(long)]
    pub labels: Option<String>,

    /// Assignee name
    #[arg(long)]
    pub assignee: Option<String>,

    /// Comma-separated dependency IDs
    #[arg(long)]
    pub deps: Option<String>,

    /// Comma-separated artifacts this unit produces
    #[arg(long)]
    pub produces: Option<String>,

    /// Comma-separated artifacts this unit requires
    #[arg(long)]
    pub requires: Option<String>,

    /// Comma-separated file paths relevant to this unit (used by mana context)
    #[arg(long)]
    pub paths: Option<String>,

    /// Action on verify failure: retry, retry:N, escalate, escalate:P0
    #[arg(long)]
    pub on_fail: Option<String>,

    /// Skip fail-first check (allow verify to already pass)
    #[arg(long, short = 'p')]
    pub pass_ok: bool,

    /// Timeout in seconds for the verify command (kills process on expiry)
    #[arg(long)]
    pub verify_timeout: Option<u64>,

    /// Claim the unit immediately (sets status to in_progress)
    #[arg(long, conflicts_with = "run")]
    pub claim: bool,

    /// Who is claiming (requires --claim)
    #[arg(long, requires = "claim")]
    pub by: Option<String>,

    /// Spawn an agent to work on this unit (requires --verify)
    #[arg(long)]
    pub run: bool,

    /// Mark as a product feature (human-only close, no verify gate required)
    #[arg(long)]
    pub feature: bool,

    /// Mark the created unit as an epic (non-dispatchable parent/grouping record, `--epic`)
    #[arg(long)]
    pub epic: bool,

    /// Launch interactive wizard (prompts for all fields step-by-step)
    #[arg(long, short = 'i')]
    pub interactive: bool,

    /// Unresolved decision/question that blocks autonomous execution (repeatable)
    #[arg(long = "decision")]
    pub decisions: Vec<String>,

    /// Output created unit as JSON (for piping)
    #[arg(long)]
    pub json: bool,

    /// Skip duplicate title check
    #[arg(long)]
    pub force: bool,
}
