use std::env;

use std::io::IsTerminal;

use anyhow::Result;
use clap::{CommandFactory, Parser};

/// Resolve whether to output JSON based on explicit flags and TTY detection.
/// When stdout is piped (not a TTY), defaults to JSON — matching rg/fd/eza behavior.
/// `--json` forces JSON even at a TTY. `--no-json` forces pretty even in a pipe.
fn auto_json(explicit_json: bool, no_json: bool) -> bool {
    if no_json {
        return false;
    }
    if explicit_json {
        return true;
    }
    // Auto-detect: JSON when stdout is not a terminal
    !std::io::stdout().is_terminal()
}

mod cli;

use cli::{
    Cli, Command, ConfigCommand, CreateOpts, CreateSubcommand, DepCommand, DoctorCommand,
    McpCommand,
};
use mana::commands::create::CreateArgs;
use mana::commands::plan::PlanArgs;
use mana::commands::quick::QuickArgs;
use mana::commands::{
    cmd_adopt, cmd_agents, cmd_claim, cmd_close, cmd_config_get, cmd_config_set, cmd_context,
    cmd_create, cmd_delete, cmd_dep_add, cmd_dep_list, cmd_dep_remove, cmd_diff, cmd_doctor,
    cmd_edit, cmd_fact, cmd_graph, cmd_init, cmd_list, cmd_locks, cmd_locks_clear, cmd_logs,
    cmd_mcp_serve, cmd_memory_context, cmd_move_from, cmd_move_to, cmd_mutate, cmd_next,
    cmd_onboard, cmd_plan, cmd_quick, cmd_recall, cmd_release, cmd_reopen, cmd_run, cmd_show,
    cmd_stats, cmd_status, cmd_sync, cmd_tidy, cmd_trace, cmd_tree, cmd_trust, cmd_unarchive,
    cmd_update, cmd_verify, cmd_verify_facts,
    review::{cmd_review, ReviewArgs},
    review_human::cmd_review_human,
};
use mana::discovery::find_mana_dir;
use mana::index::Index;
use mana::util::validate_unit_id;

// Helper to resolve a single unit ID (handles @latest selector or plain IDs)
fn resolve_unit_id(id: &str, mana_dir: &std::path::Path) -> Result<String> {
    if id == "@latest" {
        let index = Index::load(mana_dir)?;
        index
            .units
            .iter()
            .max_by_key(|e| e.updated_at)
            .map(|e| e.id.clone())
            .ok_or_else(|| anyhow::anyhow!("@latest: no units in index"))
    } else if id.starts_with('@') {
        anyhow::bail!("Unknown selector: {}", id)
    } else {
        Ok(id.to_string())
    }
}

// Helper to resolve multiple unit IDs
fn resolve_unit_ids(ids: Vec<String>, mana_dir: &std::path::Path) -> Result<Vec<String>> {
    ids.into_iter()
        .map(|id| resolve_unit_id(&id, mana_dir))
        .collect()
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Init is special - doesn't need mana_dir
    if let Command::Init {
        name,
        agent,
        run,
        plan,
        setup,
        no_agent,
    } = cli.command
    {
        return cmd_init(
            None,
            mana::commands::init::InitArgs {
                project_name: name,
                agent,
                run,
                plan,
                setup,
                no_agent,
            },
        );
    }

    // Completions don't need mana_dir either
    if let Command::Completions { shell } = cli.command {
        let mut cmd = Cli::command();
        clap_complete::generate(shell, &mut cmd, "mana", &mut std::io::stdout());
        return Ok(());
    }

    // Onboard doesn't need mana_dir — works on any project directory
    if let Command::Onboard { dir } = cli.command {
        let project_root = if dir == std::path::Path::new(".") {
            env::current_dir()?
        } else {
            dir
        };
        return cmd_onboard(&project_root);
    }

    // All other commands need mana_dir
    let mana_dir = find_mana_dir(&env::current_dir()?)?;

    match cli.command {
        Command::Init { .. } => unreachable!(),
        Command::Completions { .. } => unreachable!(),
        Command::Onboard { .. } => unreachable!(),

        Command::Create { args } => {
            let CreateOpts {
                subcommand,
                title,
                set_title,
                description,
                acceptance,
                notes,
                design,
                verify,
                parent,
                priority,
                labels,
                assignee,
                deps,
                produces,
                requires,
                paths,
                on_fail,
                pass_ok,
                verify_timeout,
                claim,
                by,
                epic,
                feature,
                decisions,
                run,
                interactive,
                json,
                force,
            } = *args;
            // Handle 'mana create next' subcommand
            if let Some(CreateSubcommand::Next {
                title,
                set_title,
                description,
                acceptance,
                notes,
                design,
                verify,
                parent,
                priority,
                labels,
                assignee,
                deps,
                produces,
                requires,
                paths: next_paths,
                on_fail,
                pass_ok,
                verify_timeout,
                claim,
                by,
                run,
                epic,
                json,
            }) = subcommand
            {
                // Resolve @latest to get the most recently created/updated unit
                let latest_id = resolve_unit_id("@latest", &mana_dir).map_err(|_| {
                    anyhow::anyhow!(
                        "No previous unit found. 'mana create next' requires at least one existing unit.\n\
                         Use 'mana create' for the first unit in a chain."
                    )
                })?;

                // Merge @latest dep with any explicit --deps
                let merged_deps = match deps {
                    Some(d) => Some(format!("{},{}", latest_id, d)),
                    None => Some(latest_id.clone()),
                };

                use mana::commands::stdin::resolve_stdin_opt;
                let description = resolve_stdin_opt(description)?;
                let acceptance = resolve_stdin_opt(acceptance)?;
                let notes = resolve_stdin_opt(notes)?;

                let resolved_title = title.or(set_title);
                let title = resolved_title
                    .ok_or_else(|| anyhow::anyhow!("mana create next: title is required"))?;

                if run && verify.is_none() {
                    anyhow::bail!(
                        "--run requires --verify\n\n\
                         Cannot spawn an agent without a test."
                    );
                }

                let on_fail = on_fail
                    .map(|s| mana::commands::create::parse_on_fail(&s))
                    .transpose()?;

                let unit_id = cmd_create(
                    &mana_dir,
                    CreateArgs {
                        title,
                        description,
                        acceptance,
                        notes,
                        design,
                        verify,
                        priority,
                        labels,
                        assignee,
                        deps: merged_deps,
                        parent,
                        produces,
                        requires,
                        paths: next_paths,
                        on_fail,
                        pass_ok,
                        claim,
                        by,
                        verify_timeout,
                        feature: false,
                        epic,
                        decisions: Vec::new(),
                        force,
                    },
                )?;

                eprintln!("⛓ Chained after unit {} (@latest)", latest_id);

                if json {
                    let unit_path = mana::discovery::find_unit_file(&mana_dir, &unit_id)?;
                    let unit = mana::unit::Unit::from_file(&unit_path)?;
                    println!("{}", serde_json::to_string(&unit)?);
                }

                if run {
                    use mana::config::Config;
                    let config = Config::load_with_extends(&mana_dir)?;
                    match &config.run {
                        Some(template) => {
                            let cmd = mana::spawner::substitute_template_with_model(
                                template,
                                &unit_id,
                                config.run_model.as_deref(),
                            );
                            eprintln!("Spawning: {}", cmd);
                            let status =
                                std::process::Command::new("sh").args(["-c", &cmd]).status();
                            match status {
                                Ok(s) if s.success() => {}
                                Ok(s) => eprintln!(
                                    "Run command exited with code {}",
                                    s.code().unwrap_or(-1)
                                ),
                                Err(e) => eprintln!("Failed to run command: {}", e),
                            }
                        }
                        None => {
                            anyhow::bail!(
                                "--run requires a configured agent.\n\
                                 Run: mana init --setup"
                            );
                        }
                    }
                }

                return Ok(());
            }

            // Resolve "-" values from stdin
            use mana::commands::stdin::resolve_stdin_opt;
            let description = resolve_stdin_opt(description)?;
            let acceptance = resolve_stdin_opt(acceptance)?;
            let notes = resolve_stdin_opt(notes)?;

            let resolved_title = title.or(set_title);

            // Determine if we should enter interactive mode:
            // 1. Explicit -i / --interactive flag, OR
            // 2. No title provided + stderr is a TTY + not --run
            let use_interactive = interactive
                || (resolved_title.is_none() && !run && std::io::stderr().is_terminal());

            let (unit_id, run_after) = if use_interactive {
                use mana::commands::interactive::{interactive_create, Prefill};

                // Pass any CLI flags as prefill — they skip prompts
                let prefill = Prefill {
                    title: resolved_title,
                    description,
                    acceptance,
                    notes,
                    design,
                    verify,
                    parent,
                    priority,
                    labels,
                    assignee,
                    deps,
                    produces,
                    requires,
                    pass_ok: if pass_ok { Some(true) } else { None },
                };

                let args = interactive_create(&mana_dir, prefill)?;
                let id = cmd_create(&mana_dir, args)?;
                (id, false)
            } else {
                let title = resolved_title
                    .ok_or_else(|| anyhow::anyhow!("mana create: title is required"))?;

                // --run requires --verify
                if run && verify.is_none() {
                    anyhow::bail!(
                        "--run requires --verify\n\n\
                         Cannot spawn an agent without a test. If you can't write a verify command,\n\
                         this is a GOAL that needs decomposition, not a SPEC ready for implementation."
                    );
                }

                // Parse --on-fail flag
                let on_fail = on_fail
                    .map(|s| mana::commands::create::parse_on_fail(&s))
                    .transpose()?;

                let id = cmd_create(
                    &mana_dir,
                    CreateArgs {
                        title,
                        description,
                        acceptance,
                        notes,
                        design,
                        verify,
                        priority,
                        labels,
                        assignee,
                        deps,
                        parent,
                        produces,
                        requires,
                        paths,
                        on_fail,
                        pass_ok,
                        verify_timeout,
                        claim,
                        by,
                        feature,
                        decisions,
                        epic,
                        force,
                    },
                )?;
                (id, run)
            };
            let run = run_after;

            // JSON output for piping (human messages go to stderr)
            if json {
                let unit_path = mana::discovery::find_unit_file(&mana_dir, &unit_id)?;
                let unit = mana::unit::Unit::from_file(&unit_path)?;
                println!("{}", serde_json::to_string(&unit)?);
            }

            // --run: spawn an agent for the new unit using configured command
            if run {
                use mana::config::Config;
                let config = Config::load_with_extends(&mana_dir)?;
                match &config.run {
                    Some(template) => {
                        let cmd = mana::spawner::substitute_template_with_model(
                            template,
                            &unit_id,
                            config.run_model.as_deref(),
                        );
                        eprintln!("Spawning: {}", cmd);
                        let status = std::process::Command::new("sh").args(["-c", &cmd]).status();
                        match status {
                            Ok(s) if s.success() => {}
                            Ok(s) => {
                                eprintln!("Run command exited with code {}", s.code().unwrap_or(-1))
                            }
                            Err(e) => eprintln!("Failed to run command: {}", e),
                        }
                    }
                    None => {
                        anyhow::bail!(
                            "--run requires a configured agent.\n\n\
                             Run: mana init --setup\n\n\
                             Or set manually: mana config set run \"<command>\"\n\n\
                             The command template uses {{id}} as a placeholder for the unit ID.\n\n\
                             Examples:\n  \
                               mana config set run \"pi @.mana/{{id}}-*.md 'implement and mana close {{id}}'\"\n  \
                               mana config set run \"claude -p 'implement unit {{id}} and run mana close {{id}}'\""
                        );
                    }
                }
            }

            Ok(())
        }

        Command::Read {
            id,
            json,
            no_json,
            short,
            history,
        } => {
            // Skip validation for selectors (start with @)
            if !id.starts_with('@') {
                validate_unit_id(&id)?;
            }
            let resolved_id = resolve_unit_id(&id, &mana_dir)?;
            cmd_show(
                &resolved_id,
                auto_json(json, no_json),
                short,
                history,
                &mana_dir,
            )
        }

        Command::Edit { id } => {
            validate_unit_id(&id)?;
            let resolved_id = resolve_unit_id(&id, &mana_dir)?;
            cmd_edit(&mana_dir, &resolved_id)
        }

        Command::List {
            status,
            priority,
            parent,
            label,
            assignee,
            all,
            mine,
            json,
            no_json,
            ids,
            format,
            search,
        } => {
            // --search delegates to recall (replaces standalone `mana recall`)
            if let Some(ref query) = search {
                return cmd_recall(&mana_dir, query, all, auto_json(json, no_json));
            }

            // --ids and --format are explicit overrides — don't auto-JSON
            let effective_json = if ids || format.is_some() {
                json // only if explicitly passed
            } else {
                auto_json(json, no_json)
            };
            cmd_list(
                status.as_deref(),
                priority,
                parent.as_deref(),
                label.as_deref(),
                assignee.as_deref(),
                mine,
                all,
                effective_json,
                ids,
                format.as_deref(),
                &mana_dir,
            )
        }

        Command::Update {
            id,
            title,
            description,
            acceptance,
            notes,
            design,
            status,
            priority,
            assignee,
            add_label,
            remove_label,
            decisions,
            resolve_decisions,
            claim,
            release,
            by,
            parent,
            add_dep,
            remove_dep,
        } => {
            use mana::commands::stdin::resolve_stdin_opt;
            validate_unit_id(&id)?;
            let resolved_id = resolve_unit_id(&id, &mana_dir)?;

            // Handle --claim / --release (replaces standalone `mana claim`)
            if claim {
                cmd_claim(&mana_dir, &resolved_id, by, false)?;
                return Ok(());
            }
            if release {
                cmd_release(&mana_dir, &resolved_id)?;
                return Ok(());
            }

            // Handle --parent (replaces standalone `mana adopt`)
            if let Some(ref parent_id) = parent {
                validate_unit_id(parent_id)?;
                cmd_adopt(&mana_dir, parent_id, std::slice::from_ref(&resolved_id))?;
            }

            // Handle --add-dep / --remove-dep (replaces standalone `mana dep`)
            if let Some(ref dep_id) = add_dep {
                validate_unit_id(dep_id)?;
                let resolved_dep = resolve_unit_id(dep_id, &mana_dir)?;
                cmd_dep_add(&mana_dir, &resolved_id, &resolved_dep)?;
            }
            if let Some(ref dep_id) = remove_dep {
                validate_unit_id(dep_id)?;
                let resolved_dep = resolve_unit_id(dep_id, &mana_dir)?;
                cmd_dep_remove(&mana_dir, &resolved_id, &resolved_dep)?;
            }

            // Resolve "-" values from stdin
            let description = resolve_stdin_opt(description)?;
            let notes = resolve_stdin_opt(notes)?;
            let acceptance = resolve_stdin_opt(acceptance)?;

            // Skip the regular update if only structural flags were passed
            let has_field_updates = title.is_some()
                || description.is_some()
                || acceptance.is_some()
                || notes.is_some()
                || design.is_some()
                || status.is_some()
                || priority.is_some()
                || assignee.is_some()
                || add_label.is_some()
                || remove_label.is_some()
                || !decisions.is_empty()
                || !resolve_decisions.is_empty();

            if !has_field_updates && (parent.is_some() || add_dep.is_some() || remove_dep.is_some())
            {
                return Ok(());
            }

            cmd_update(
                &mana_dir,
                &resolved_id,
                title,
                description,
                acceptance,
                notes,
                design,
                status,
                priority,
                assignee,
                add_label,
                remove_label,
                decisions,
                resolve_decisions,
            )
        }

        Command::Close {
            ids,
            reason,
            force,
            failed,
            defer_verify,
            stdin,
            check,
        } => {
            let ids = if stdin {
                mana::commands::stdin::read_ids_from_stdin()?
            } else {
                ids
            };
            for id in &ids {
                validate_unit_id(id)?;
            }
            let resolved_ids = resolve_unit_ids(ids, &mana_dir)?;

            // --check: run verify without closing (replaces standalone `mana verify`)
            if check {
                let out = mana::output::Output::new();
                for id in &resolved_ids {
                    let passed = cmd_verify(&mana_dir, id, &out)?;
                    if !passed {
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }

            // MANA_BATCH_VERIFY=1 auto-defers verify, same as --defer-verify
            let defer = defer_verify || std::env::var("MANA_BATCH_VERIFY").as_deref() == Ok("1");
            if failed {
                mana::commands::close::cmd_close_failed(&mana_dir, resolved_ids, reason)
            } else {
                cmd_close(&mana_dir, resolved_ids, reason, force, defer)
            }
        }

        Command::Verify {
            id, json, no_json, ..
        } => {
            validate_unit_id(&id)?;
            let resolved_id = resolve_unit_id(&id, &mana_dir)?;
            let out = mana::output::Output::new();
            let passed = cmd_verify(&mana_dir, &resolved_id, &out)?;
            if auto_json(json, no_json) {
                println!(
                    "{}",
                    serde_json::json!({"id": resolved_id, "passed": passed})
                );
            }
            if !passed {
                std::process::exit(1);
            }
            Ok(())
        }

        Command::Claim {
            id,
            release,
            by,
            force,
        } => {
            validate_unit_id(&id)?;
            let resolved_id = resolve_unit_id(&id, &mana_dir)?;
            if release {
                cmd_release(&mana_dir, &resolved_id)
            } else {
                cmd_claim(&mana_dir, &resolved_id, by, force)
            }
        }

        Command::Reopen { id } => {
            validate_unit_id(&id)?;
            let resolved_id = resolve_unit_id(&id, &mana_dir)?;
            cmd_reopen(&mana_dir, &resolved_id)
        }

        Command::Delete { id } => {
            validate_unit_id(&id)?;
            let resolved_id = resolve_unit_id(&id, &mana_dir)?;
            cmd_delete(&mana_dir, &resolved_id)
        }

        Command::Dep { command } => match command {
            DepCommand::Add { id, depends_on } => {
                validate_unit_id(&id)?;
                validate_unit_id(&depends_on)?;
                let resolved_id = resolve_unit_id(&id, &mana_dir)?;
                let resolved_depends_on = resolve_unit_id(&depends_on, &mana_dir)?;
                cmd_dep_add(&mana_dir, &resolved_id, &resolved_depends_on)
            }
            DepCommand::Remove { id, depends_on } => {
                validate_unit_id(&id)?;
                validate_unit_id(&depends_on)?;
                let resolved_id = resolve_unit_id(&id, &mana_dir)?;
                let resolved_depends_on = resolve_unit_id(&depends_on, &mana_dir)?;
                cmd_dep_remove(&mana_dir, &resolved_id, &resolved_depends_on)
            }
            DepCommand::List { id } => {
                validate_unit_id(&id)?;
                let resolved_id = resolve_unit_id(&id, &mana_dir)?;
                cmd_dep_list(&mana_dir, &resolved_id)
            }
        },

        Command::Status { json, no_json } => cmd_status(auto_json(json, no_json), &mana_dir),

        Command::Next {
            count,
            json,
            no_json,
        } => cmd_next(count, auto_json(json, no_json), &mana_dir),

        Command::Context {
            id,
            json,
            no_json,
            structure_only,
            agent_prompt,
            instructions,
            overlaps,
        } => {
            match id {
                Some(ref id_str) => {
                    validate_unit_id(id_str)?;
                    let resolved_id = resolve_unit_id(id_str, &mana_dir)?;
                    cmd_context(
                        &mana_dir,
                        &resolved_id,
                        auto_json(json, no_json),
                        structure_only,
                        agent_prompt,
                        instructions,
                        overlaps,
                    )
                }
                None => {
                    // No ID: output memory context
                    cmd_memory_context(&mana_dir, auto_json(json, no_json))
                }
            }
        }

        Command::Tree { id } => {
            if let Some(ref id_val) = id {
                validate_unit_id(id_val)?;
            }
            cmd_tree(&mana_dir, id.as_deref())
        }
        Command::Graph { format } => cmd_graph(&mana_dir, &format),
        Command::Sync => cmd_sync(&mana_dir),
        Command::Tidy { dry_run, .. } => {
            let out = mana::output::Output::new();
            cmd_tidy(&mana_dir, dry_run, &out)
        }
        Command::Stats { json, no_json } => cmd_stats(&mana_dir, auto_json(json, no_json)),
        Command::Doctor { command } => {
            let fix = matches!(command, Some(DoctorCommand::Fix));
            cmd_doctor(&mana_dir, fix)
        }
        Command::Trust { revoke, check } => cmd_trust(&mana_dir, revoke, check),

        Command::Unarchive { id } => {
            validate_unit_id(&id)?;
            let resolved_id = resolve_unit_id(&id, &mana_dir)?;
            cmd_unarchive(&mana_dir, &resolved_id)
        }

        Command::Locks { clear } => {
            if clear {
                cmd_locks_clear(&mana_dir)
            } else {
                cmd_locks(&mana_dir)
            }
        }

        Command::Quick {
            title,
            description,
            acceptance,
            notes,
            verify,
            priority,
            by,
            produces,
            requires,
            parent,
            on_fail,
            pass_ok,
            verify_timeout,
            force,
        } => {
            if let Some(ref p) = parent {
                validate_unit_id(p)?;
            }

            // Parse --on-fail flag
            let on_fail = on_fail
                .map(|s| mana::commands::create::parse_on_fail(&s))
                .transpose()?;

            cmd_quick(
                &mana_dir,
                QuickArgs {
                    title,
                    description,
                    acceptance,
                    notes,
                    verify,
                    priority,
                    by,
                    produces,
                    requires,
                    parent,
                    on_fail,
                    pass_ok,
                    verify_timeout,
                    force,
                },
            )
        }

        Command::Move { from, to, ids } => {
            for id in &ids {
                validate_unit_id(id)?;
            }
            match (from, to) {
                (Some(src), None) => cmd_move_from(&mana_dir, &src, &ids).map(|_| ()),
                (None, Some(dst)) => cmd_move_to(&mana_dir, &dst, &ids).map(|_| ()),
                _ => unreachable!("clap enforces --from or --to"),
            }
        }

        Command::Adopt { parent, children } => {
            validate_unit_id(&parent)?;
            for child in &children {
                validate_unit_id(child)?;
            }
            let resolved_parent = resolve_unit_id(&parent, &mana_dir)?;
            let resolved_children = resolve_unit_ids(children, &mana_dir)?;
            cmd_adopt(&mana_dir, &resolved_parent, &resolved_children).map(|_| ())
        }

        Command::Run {
            id,
            jobs,
            dry_run,
            loop_mode,
            keep_going,
            timeout,
            idle_timeout,
            json_stream,
            review,
        } => cmd_run(
            &mana_dir,
            mana::commands::run::RunArgs {
                id,
                jobs,
                dry_run,
                loop_mode,
                keep_going,
                timeout,
                idle_timeout,
                json_stream,
                review,
            },
        ),

        Command::Plan {
            id,
            strategy,
            auto,
            dry_run,
        } => {
            validate_unit_id(&id)?;
            let resolved_id = resolve_unit_id(&id, &mana_dir)?;
            cmd_plan(
                &mana_dir,
                PlanArgs {
                    id: resolved_id,
                    strategy,
                    auto,
                    dry_run,
                },
            )
        }

        Command::Agents { json, no_json } => cmd_agents(&mana_dir, auto_json(json, no_json)),

        Command::Logs { id, follow, all } => {
            validate_unit_id(&id)?;
            let resolved_id = resolve_unit_id(&id, &mana_dir)?;
            cmd_logs(&mana_dir, &resolved_id, follow, all)
        }

        Command::Fact {
            title,
            verify,
            description,
            paths,
            ttl,
            pass_ok,
        } => {
            cmd_fact(&mana_dir, title, verify, description, paths, ttl, pass_ok)?;
            Ok(())
        }

        Command::Recall {
            query,
            all,
            json,
            no_json,
        } => cmd_recall(&mana_dir, &query, all, auto_json(json, no_json)),

        Command::VerifyFacts => cmd_verify_facts(&mana_dir),

        Command::Config { command } => match command {
            ConfigCommand::Get { key } => cmd_config_get(&mana_dir, &key),
            ConfigCommand::GetProject { key } => {
                mana::commands::config_cmd::cmd_config_get_project(&mana_dir, &key)
            }
            ConfigCommand::GetGlobal { key } => {
                mana::commands::config_cmd::cmd_config_get_global(&mana_dir, &key)
            }
            ConfigCommand::Inspect { key } => {
                mana::commands::config_cmd::cmd_config_inspect(&mana_dir, key.as_deref())
            }
            ConfigCommand::Doctor => mana::commands::config_cmd::cmd_config_doctor(&mana_dir),
            ConfigCommand::Set { key, value } => cmd_config_set(&mana_dir, &key, &value),
            ConfigCommand::SetProject { key, value } => {
                mana::commands::config_cmd::cmd_config_set_project(&mana_dir, &key, &value)
            }
            ConfigCommand::SetGlobal { key, value } => {
                mana::commands::config_cmd::cmd_config_set_global(&mana_dir, &key, &value)
            }
        },

        Command::Mcp { command } => match command {
            McpCommand::Serve => cmd_mcp_serve(&mana_dir),
        },

        Command::Trace { id, json, no_json } => {
            validate_unit_id(&id)?;
            let resolved_id = resolve_unit_id(&id, &mana_dir)?;
            cmd_trace(&resolved_id, auto_json(json, no_json), &mana_dir)
        }

        Command::Diff {
            id,
            stat,
            name_only,
            no_color,
        } => {
            validate_unit_id(&id)?;
            let resolved_id = resolve_unit_id(&id, &mana_dir)?;
            let output = if stat {
                mana::commands::diff::DiffOutput::Stat
            } else if name_only {
                mana::commands::diff::DiffOutput::NameOnly
            } else {
                mana::commands::diff::DiffOutput::Full
            };
            cmd_diff(&mana_dir, &resolved_id, output, no_color)
        }

        Command::Mutate {
            id,
            max,
            timeout,
            diff_base,
            json,
            no_json,
        } => {
            validate_unit_id(&id)?;
            let resolved_id = resolve_unit_id(&id, &mana_dir)?;
            cmd_mutate(
                &mana_dir,
                mana::commands::mutate::MutateArgs {
                    id: resolved_id,
                    max_mutants: max,
                    timeout,
                    diff_base,
                    json: auto_json(json, no_json),
                },
            )
        }

        Command::Review {
            id,
            approve,
            request_changes,
            reject,
            agent,
            diff,
            model,
        } => {
            if agent {
                // Old behavior: AI adversarial review
                let id = id.ok_or_else(|| anyhow::anyhow!("unit ID required for --agent mode"))?;
                validate_unit_id(&id)?;
                let resolved_id = resolve_unit_id(&id, &mana_dir)?;
                cmd_review(
                    &mana_dir,
                    ReviewArgs {
                        id: resolved_id,
                        model,
                        diff_only: diff,
                    },
                )
            } else {
                // New behavior: human review
                cmd_review_human(
                    &mana_dir,
                    id.as_deref(),
                    approve,
                    request_changes.as_deref(),
                    reject.as_deref(),
                )
            }
        }
    }
}
