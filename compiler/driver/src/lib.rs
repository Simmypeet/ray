//! Contains the main `run()` function for the compiler.

use std::{
    fs::File,
    io::{self, Write},
    process::ExitCode,
    sync::Arc,
};

use qbice::{serialize::Plugin, stable_hash::SeededStableHasherBuilder};
use rayc_c::context::{Context, instantiation::CDef};
use rayc_qbice::{Engine, InMemoryFactory, IncrementalStorageEngine, TrackedEngine};
use rayc_symbol::symbol_kind::get_all_def_ids;
use rayc_symbol_impl::source_map::create_source_map;
use rayc_target::{Arguments, TargetID};
use tracing::instrument;

use crate::term::ReportTerm;

pub mod term;

async fn create_engine(argument: &Arguments, report_term: &mut ReportTerm<'_>) -> Option<Engine> {
    if let Some(inc_path) = argument.incremental_path() {
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

/// Runs the program with the given arguments.
#[must_use]
#[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
#[instrument(skip(err_writer, _out_writer))]
pub async fn run(
    argument: Arguments,
    err_writer: &mut dyn Write,
    _out_writer: &mut dyn Write,
) -> ExitCode {
    let mut report_term = ReportTerm::new(err_writer, argument.fancy());

    let Some(mut engine) = create_engine(&argument, &mut report_term).await else {
        return ExitCode::FAILURE;
    };

    // Due to how rust compiler work, if a crate is linked without having any
    // symbols in it used, the crate will be completely ignored and the
    // static distributed registration will be optimized out, causing the engine
    // to not have the executors
    rayc_source_file_impl::black_box();
    rayc_lexical_impl::black_box();
    rayc_syntax_impl::black_box();
    rayc_semantic_element_impl::black_box();

    engine.register_program(rayc_qbice::RAY_PROGRAM);

    // set the initial input, the invocation arguments
    let target_name = argument.target_name();
    let local_target_id = TargetID::from_target_name(&target_name);

    let engine = Arc::new(engine);

    {
        let mut input_session = engine.input_session().await;

        // rayc_corelib_impl::initialize_corelib(&mut input_session).await;

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
                        (input_session.intern_unsized(target_name), local_target_id),
                        (input_session.intern_unsized("core".to_owned()), TargetID::CORE),
                    ]
                    .into_iter()
                    .collect(),
                ),
            )
            .await;

        if let Some(explicit_seed) = argument.target_seed() {
            input_session
                .set_input(rayc_target::SeedKey { target_id: local_target_id }, explicit_seed)
                .await;
        }

        input_session
            .set_input(
                rayc_target::Key { target_id: local_target_id },
                input_session.intern(argument.clone()),
            )
            .await;

        input_session
            .set_input(
                rayc_target::IRVerificationKey { target_id: local_target_id },
                argument.verify_ir(),
            )
            .await;

        rayc_source_file_impl::refresh_source_file_executors(&mut input_session).await;

        input_session.commit().await;
    }

    // now the query can start ...

    let tracked_engine = engine.clone().tracked().await;

    let source_map = tracked_engine.create_source_map(local_target_id).await;

    report_term.set_source_map(&source_map);

    let diagnostic_count = {
        let check = tracked_engine.query(&rayc_check::Key { target_id: local_target_id }).await;

        let mut diagnostics: Vec<_> = check.all_diagnostics().collect();
        diagnostics.sort();

        for diag in &diagnostics {
            report_term.report_rendered(diag);
        }

        diagnostics.len()
    };

    if diagnostic_count != 0 {
        report_term
            .report_simple_error(format!("Compilation aborted due to {diagnostic_count} error(s)"));

        ExitCode::FAILURE
    } else {
        write_c(&tracked_engine, local_target_id, &mut report_term).await
    }
}

async fn write_c(
    engine: &TrackedEngine,
    target_id: TargetID,
    report_term: &mut ReportTerm<'_>,
) -> ExitCode {
    let mut file = match File::create_new("output.c") {
        Ok(file) => file,
        Err(error) => {
            report_term.report_simple_error(format!("Failed to create output.c: {error}"));
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = write_c_translation_unit(engine, target_id, &mut file).await {
        report_term.report_simple_error(format!("Failed to generate C output: {error}"));
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

/// Writes a complete C translation unit for `target_id` to `buf`.
pub async fn write_c_translation_unit(
    engine: &TrackedEngine,
    target_id: TargetID,
    buf: &mut impl Write,
) -> io::Result<()> {
    let mut generator = Context::new(engine.clone());
    let def_ids = engine.get_all_def_ids(target_id).await;

    for def_id in def_ids.iter().copied() {
        let cdef = CDef::builder().def_id(target_id.make_global(def_id)).build();
        let _ = generator.get_cdef_id(cdef).await;
    }

    let cdef_ids = generator.cdef_decl_ids().collect::<Vec<_>>();
    let mut function_definitions = Vec::with_capacity(cdef_ids.len());

    for cdef_id in cdef_ids {
        let mut definition = Vec::new();
        let mut writer = rayc_c::writer::Writer::new(&mut definition);
        writer.generate_function_definition(cdef_id, &mut generator).await?;
        function_definitions.push(definition);
    }

    writeln!(buf, "/* This file is auto generated by Ray compiler */")?;
    writeln!(buf, "#include <stdint.h>")?;
    writeln!(buf, "#include <stdbool.h>")?;

    writeln!(buf)?;
    writeln!(buf, "/* Composite type forward declarations */")?;

    generator.write_forward_decl_tuples(buf)?;

    writeln!(buf)?;
    writeln!(buf, "/* Composite type definitions */")?;

    generator.write_tuple_struct_defs(buf)?;

    writeln!(buf)?;
    writeln!(buf, "/* Function forward declarations */")?;

    generator.write_forward_decl_cdefs(buf).await?;

    writeln!(buf)?;
    writeln!(buf, "/* Function definitions */")?;

    for definition in function_definitions {
        buf.write_all(&definition)?;
        writeln!(buf)?;
        writeln!(buf)?;
    }

    Ok(())
}
