//! Contains the main `run()` function for the compiler.

use std::{io::Write, path::Path, process::ExitCode, sync::Arc};

use qbice::{serialize::Plugin, stable_hash::SeededStableHasherBuilder};
use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_qbice::{Engine, InMemoryFactory, IncrementalStorageEngine, TrackedEngine};
use rayc_symbol_impl::source_map::{SourceMap, create_source_map};
use rayc_target::{Arguments, TargetID};
use tracing::instrument;

use crate::{
    artifact::execute, entry_point::validate_entry_point, invocation::Invocation, term::ReportTerm,
};

mod artifact;
mod entry_point;
mod invocation;
mod native;
mod new;
pub mod term;

pub use new::create_project;

async fn create_engine(
    incremental_path: Option<&Path>,
    report_term: &mut ReportTerm<'_>,
) -> Option<Engine> {
    if let Some(inc_path) = incremental_path {
        // the database creates only its own directory, not its parents
        if let Err(err) = std::fs::create_dir_all(inc_path) {
            report_term.report_simple_error(format!(
                "failed to create incremental directory '{}': {err}",
                inc_path.display()
            ));

            return None;
        }

        match Engine::new_with(
            Plugin::new(),
            IncrementalStorageEngine(inc_path),
            SeededStableHasherBuilder::new(0),
        )
        .await
        {
            Ok(engine) => Some(engine),
            Err(err) => {
                report_term.report_simple_error(format!(
                    "failed to create incremental engine at '{}': {err}",
                    inc_path.display()
                ));

                None
            }
        }
    } else {
        Some(
            Engine::new_with(Plugin::new(), InMemoryFactory, SeededStableHasherBuilder::new(0))
                .await
                .expect("in-memory is infailable"),
        )
    }
}

/// A compilation of a target whose query inputs are set up from the command
/// line arguments, ready to be queried.
pub struct Session {
    invocation: Invocation,
    engine: TrackedEngine,
    target_id: TargetID,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("invocation", &self.invocation)
            .field("target_id", &self.target_id)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// Resolves what `argument` compiles and sets up the query inputs of its
    /// target, reporting to `report_term` why it cannot.
    pub async fn new(argument: &Arguments, report_term: &mut ReportTerm<'_>) -> Option<Self> {
        // resolve what to compile from the arguments and the project manifest
        let invocation = match Invocation::resolve(argument) {
            Ok(invocation) => invocation,
            Err(error) => {
                report_term.report_simple_error(error.to_string());
                return None;
            }
        };

        if invocation.target_name() == "core" {
            report_term.report_simple_error(
                "target name 'core' is reserved for the compiler core library",
            );
            return None;
        }

        let mut engine = create_engine(invocation.incremental_path(), report_term).await?;

        // Due to how rust compiler work, if a crate is linked without having
        // any symbols in it used, the crate will be completely ignored and the
        // static distributed registration will be optimized out, causing the
        // engine to not have the executors
        rayc_source_file_impl::black_box();
        rayc_lexical_impl::black_box();
        rayc_syntax_impl::black_box();
        rayc_semantic_element_impl::black_box();
        rayc_ir_builder::black_box();

        engine.register_program(rayc_qbice::RAY_PROGRAM);

        // set the initial input, the resolved invocation
        let target_name = invocation.target_name().to_owned();
        let local_target_id = TargetID::from_target_name(&target_name);

        let engine = Arc::new(engine);

        {
            let mut input_session = engine.input_session().await;

            input_session
                .set_input(
                    rayc_target::LinkKey { target_id: local_target_id },
                    input_session.intern(std::iter::once(TargetID::CORE).collect()),
                )
                .await;

            input_session
                .set_input(
                    rayc_target::AllTargetIDsKey,
                    input_session.intern([local_target_id, TargetID::CORE].into_iter().collect()),
                )
                .await;

            input_session
                .set_input(
                    rayc_target::MapKey,
                    input_session.intern(
                        [
                            (input_session.intern_unsized(target_name.clone()), local_target_id),
                            (input_session.intern_unsized("core".to_owned()), TargetID::CORE),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                )
                .await;

            // extract the target information required by the queries from the
            // resolved invocation
            input_session
                .set_input(
                    rayc_target::RootFilePathKey { target_id: local_target_id },
                    input_session.intern_unsized(invocation.root_file().to_path_buf()),
                )
                .await;

            input_session
                .set_input(
                    rayc_target::NameKey { target_id: local_target_id },
                    input_session.intern_unsized(target_name),
                )
                .await;

            if let Some(explicit_seed) = argument.target_seed() {
                input_session
                    .set_input(rayc_target::SeedKey { target_id: local_target_id }, explicit_seed)
                    .await;
            }

            input_session
                .set_input(
                    rayc_target::IRVerificationKey { target_id: local_target_id },
                    argument.verify_ir(),
                )
                .await;

            rayc_source_file_impl::refresh_source_file_executors(&mut input_session).await;

            rayc_corelib::initialize(&mut input_session).await;
            input_session.commit().await;
        }

        Some(Self { invocation, engine: engine.tracked().await, target_id: local_target_id })
    }

    /// Returns the engine the queries of the target run on.
    #[must_use]
    pub const fn engine(&self) -> &TrackedEngine { &self.engine }

    /// Returns the ID of the target being compiled.
    #[must_use]
    pub const fn target_id(&self) -> TargetID { self.target_id }

    /// Returns the source files the diagnostics of the target point into.
    pub async fn source_map(&self) -> SourceMap {
        self.engine.create_source_map(self.target_id).await
    }

    /// Checks the target for semantic errors, returning every diagnostic in
    /// the order they are reported.
    pub async fn check(&self) -> Vec<Rendered<ByteIndex>> {
        let check = self.engine.query(&rayc_check::Key { target_id: self.target_id }).await;

        let mut diagnostics: Vec<_> = check.all_diagnostics().cloned().collect();
        diagnostics.sort();

        diagnostics
    }
}

/// Runs the program with the given arguments.
#[must_use]
#[instrument(skip(err_writer, _out_writer))]
pub async fn run(
    argument: Arguments,
    err_writer: &mut dyn Write,
    _out_writer: &mut dyn Write,
) -> ExitCode {
    let mut report_term = ReportTerm::new(err_writer, argument.fancy());

    let Some(session) = Session::new(&argument, &mut report_term).await else {
        return ExitCode::FAILURE;
    };

    // now the query can start ...

    let source_map = session.source_map().await;

    report_term.set_source_map(&source_map);

    let diagnostic_count = {
        let diagnostics = session.check().await;

        for diag in &diagnostics {
            report_term.report_rendered(diag);
        }

        diagnostics.len()
    };

    if diagnostic_count != 0 {
        report_term
            .report_simple_error(format!("Compilation aborted due to {diagnostic_count} error(s)"));

        return ExitCode::FAILURE;
    }

    let Some(artifact) = session.invocation.artifact() else {
        return ExitCode::SUCCESS;
    };

    let entry_point = if artifact.requires_entry_point() {
        match validate_entry_point(&session.engine, session.target_id).await {
            Ok(entry_point) => Some(entry_point),
            Err(error) => {
                report_term.report_rendered(&error.report(&session.engine).await);
                report_term.report_simple_error("Compilation aborted due to 1 error(s)");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };

    match execute(&session.engine, session.target_id, artifact, entry_point).await {
        Ok(exit_code) => exit_code,
        Err(error) => {
            report_term.report_simple_error(error.to_string());
            ExitCode::FAILURE
        }
    }
}
