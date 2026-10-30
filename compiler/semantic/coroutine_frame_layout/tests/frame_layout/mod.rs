//! Snapshots the coroutine frame layout of every function of a fixture that
//! suspends.
//!
//! Each fixture is a `main.ray` with a `snapshot.snap` beside it. For every
//! function with a suspension point, the snapshot lists which values go to
//! the prefix, which to the start variant and which to the variant of each
//! suspension point, each named by its source text and location.

#![allow(missing_docs)]

use std::path::{Path, PathBuf};

use insta::assert_snapshot;
use rayc_coroutine_frame_layout::{
    frame_layout::{FrameLayout, Placement, SuspensionLocalPlacement},
    savable::Savable,
    suspension_point::SuspensionPointID,
};
use rayc_driver::{
    Session,
    term::{ReportTerm, render_plain},
};
use rayc_ir::{address::Local, cfg::Terminator, get_ir, ir_function::IRFunction};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::{
    name::get_qualified_name, source_map::to_absolute_span, symbol_kind::get_all_def_with_body_ids,
};
use rayc_target::{Arguments, Input};
use rayc_type::subst::Subst;

#[test_generator::test_resources(
    "compiler/semantic/coroutine_frame_layout/tests/frame_layout/**/main.ray"
)]
fn main(resource: &str) {
    stacker::maybe_grow(3 * 1024 * 1024, 8 * 1024 * 1024, || {
        let file_path = PathBuf::from(resource);
        std::env::set_current_dir(env!("RAYC_CARGO_WORKSPACE_DIR")).unwrap();

        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            // in the debug build, the stack size of the future is not
            // optimized well, so it needs a larger stack
            .thread_stack_size(if cfg!(debug_assertions) { 8 } else { 2 } * 1024 * 1024)
            .build()
            .expect("Failed to create Tokio runtime")
            .block_on(test(&file_path));
    });
}

async fn test(file_path: &Path) {
    let snapshot = render_fixture(file_path).await;

    let mut settings = insta::Settings::clone_current();
    let full_path = std::fs::canonicalize(file_path).unwrap();
    settings.set_snapshot_path(full_path.parent().unwrap());
    settings.set_prepend_module_to_snapshot(false);
    settings.remove_snapshot_suffix();
    let _guard = settings.bind_to_scope();

    assert_snapshot!("snapshot", snapshot);
}

/// Checks the fixture, which must be free of errors, and renders the frame
/// layout of each of its functions that suspends, ordered by name.
async fn render_fixture(file_path: &Path) -> String {
    let input =
        Input::builder().path(file_path.to_path_buf()).target_seed(0).verify_ir(true).build();
    let arguments = Arguments::new_check(input);

    let mut err_writer = Vec::new();
    let session = Session::new(&arguments, &mut ReportTerm::new(&mut err_writer, false))
        .await
        .unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&err_writer)));

    // a layout is only meaningful for a program that checks
    let diagnostics = session.check().await;
    if !diagnostics.is_empty() {
        let source_map = session.source_map().await;
        let rendered = diagnostics
            .iter()
            .map(|diagnostic| render_plain(diagnostic, &source_map))
            .collect::<Vec<_>>();
        panic!("the fixture has diagnostics:\n{}", rendered.join("\n"));
    }

    let source = std::fs::read_to_string(file_path).unwrap();
    let engine = session.engine();
    let target_id = session.target_id();

    // each function that suspends, rendered under its name
    let mut functions = Vec::new();
    for &symbol_id in engine.get_all_def_with_body_ids(target_id).await.iter() {
        let def_id = target_id.make_global(symbol_id);
        let ir = engine.get_ir(def_id).await;
        let root_id = ir.root_id();

        let mut solver = Solver::new(engine.clone(), def_id).await;
        let layout =
            FrameLayout::compute(engine.clone(), &mut solver, def_id, root_id, Subst::new_empty())
                .await;
        if layout.suspension_points().next().is_none() {
            continue;
        }

        let renderer = Renderer {
            engine,
            source: &source,
            ir_function: ir.get_function(root_id),
            parameters: ir.get_function(root_id).parameter_locals(def_id, engine).await,
        };
        let name = engine.get_qualified_name(def_id).await;
        functions.push((name.clone(), renderer.render(&name, &layout).await));
    }

    functions.sort();
    functions.into_iter().map(|(_, rendered)| rendered).collect::<Vec<_>>().join("\n")
}

/// Names the values of one function by the source they come from.
struct Renderer<'a> {
    engine: &'a TrackedEngine,
    source: &'a str,
    ir_function: &'a IRFunction,

    /// The parameter locals in declaration order, to number them.
    parameters: Vec<Local>,
}

/// A place in the fixture: its `line:column` and the source text there, cut
/// at the end of the first line. Ordered by position.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Located {
    line: usize,
    column: usize,
    text: String,
}

impl std::fmt::Display for Located {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{} `{}`", self.line, self.column, self.text)
    }
}

/// A value of the frame, as rendered: where it comes from, if anywhere, and
/// what it is. Ordered by position, the values without one first.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Value {
    location: Option<Located>,
    kind: String,
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.location {
            Some(location) => write!(f, "{location} ({})", self.kind),
            None => write!(f, "{}", self.kind),
        }
    }
}

/// Which part of the frame a value is placed in, in the order they are
/// rendered.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Prefix,
    Start,

    /// The variant of a suspension point, named by the expression it
    /// suspends on.
    Variant(Located),
}

impl Part {
    fn header(&self) -> String {
        match self {
            Self::Prefix => "prefix:".to_owned(),
            Self::Start => "start:".to_owned(),
            Self::Variant(suspension_point) => format!("suspended at {suspension_point}:"),
        }
    }
}

impl Renderer<'_> {
    async fn render(&self, name: &str, layout: &FrameLayout) -> String {
        use std::fmt::Write;

        // every variant, even an empty one, then each value in its part
        let mut parts = vec![(Part::Prefix, Vec::new()), (Part::Start, Vec::new())];
        for suspension_point in layout.suspension_points() {
            parts.push((Part::Variant(self.suspension_point(suspension_point).await), Vec::new()));
        }
        for (savable, placement) in layout.placements() {
            let part = self.part(placement).await;
            let value = self.savable(savable).await;
            parts.iter_mut().find(|(candidate, _)| *candidate == part).unwrap().1.push(value);
        }

        parts.sort();
        let mut rendered = format!("{name}:\n");
        for (part, mut values) in parts {
            values.sort();
            writeln!(rendered, "  {}", part.header()).unwrap();
            for value in values {
                writeln!(rendered, "    {value}").unwrap();
            }
        }

        rendered
    }

    async fn part(&self, placement: Placement) -> Part {
        match placement {
            Placement::Prefix => Part::Prefix,
            Placement::SuspensionLocal(SuspensionLocalPlacement::Start) => Part::Start,
            Placement::SuspensionLocal(SuspensionLocalPlacement::SuspensionLocal(
                suspension_point,
            )) => Part::Variant(self.suspension_point(suspension_point).await),
        }
    }

    /// Names a value by what it is and the source it comes from.
    async fn savable(&self, savable: Savable) -> Value {
        match savable {
            Savable::Local(local) => self.local(local).await,
            Savable::Expression(expression_id) => {
                let span = self.ir_function.get_expression(expression_id).span();
                Value { location: Some(self.locate(&span).await), kind: "expression".to_owned() }
            }
            Savable::Awaitee(suspension_point) => Value {
                location: Some(self.suspension_point(suspension_point).await),
                kind: "awaitee".to_owned(),
            },
        }
    }

    async fn local(&self, local: Local) -> Value {
        match local {
            Local::Variable(variable_id) => {
                let variable = self.ir_function.get_variable(variable_id);
                let kind = if variable.is_temporary() { "temporary" } else { "variable" };
                Value { location: Some(self.locate(&variable.span()).await), kind: kind.to_owned() }
            }
            Local::Parameter(_) => {
                let position = self.parameters.iter().position(|parameter| *parameter == local);
                Value { location: None, kind: format!("parameter #{}", position.unwrap()) }
            }
            Local::ClosureParameter(_)
            | Local::OperationHandlerParameter(_)
            | Local::Capture(_) => Value { location: None, kind: format!("{local:?}") },
        }
    }

    /// Names a suspension point by the expression it suspends on.
    async fn suspension_point(&self, suspension_point: SuspensionPointID) -> Located {
        let expression_id = self
            .ir_function
            .block_terminator(suspension_point.block_id())
            .and_then(Terminator::suspending_expr)
            .unwrap();
        let span = self.ir_function.get_expression(expression_id).span();

        self.locate(&span).await
    }

    /// Locates `span` in the fixture.
    async fn locate(&self, span: &RelativeSpan) -> Located {
        let span = self.engine.to_absolute_span(span).await;
        let before = &self.source[..span.start];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().unwrap().chars().count() + 1;

        let text = &self.source[span.start..span.end];
        let text = match text.split_once('\n') {
            Some((first, _)) => format!("{} …", first.trim_end()),
            None => text.to_owned(),
        };

        Located { line, column, text }
    }
}
