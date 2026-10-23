//! Implements the `new` subcommand, which creates a project.

use std::{io::Write, process::ExitCode};

use rayc_project::{Kind, Project};
use rayc_target::New;

use crate::term::ReportTerm;

/// Creates the project described by `arguments` and reports the outcome to
/// `err_writer`.
#[must_use]
pub fn create_project(arguments: &New, err_writer: &mut dyn Write) -> ExitCode {
    let kind = if arguments.is_library() { Kind::Library } else { Kind::Binary };
    let mut report_term = ReportTerm::new(err_writer, arguments.fancy());

    match Project::create(arguments.path(), arguments.requested_name(), kind) {
        Ok(project) => {
            report_term.report_simple_info(format!(
                "created {} project `{}` in '{}'",
                kind.description(),
                project.name(),
                arguments.path().display()
            ));

            ExitCode::SUCCESS
        }
        Err(error) => {
            // a name inferred from the directory can be overridden
            if arguments.requested_name().is_none() && error.is_project_name_error() {
                report_term.report_simple_error_with_help(
                    error.to_string(),
                    "use `--name` to name the project independently of its directory",
                );
            } else {
                report_term.report_simple_error(error.to_string());
            }

            ExitCode::FAILURE
        }
    }
}
