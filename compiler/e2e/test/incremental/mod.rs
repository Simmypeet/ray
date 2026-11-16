//! Tests how the work of an edit grows with the program around it.
//!
//! Each scenario in `scenarios` is a template of a program and an edit to it
//! (see [`scenario`]). Its test renders the program, checks it, edits it, and
//! checks it again, counting what the engine did for the second check. It
//! measures the same edit in programs of different sizes, growing one
//! dimension, a knob, at a time, and classifies how each count of each query
//! type grows along each knob. Work that grows with the program is how an
//! edit that should cost little ends up taking seconds.
//!
//! The counts come from the engine rather than the clock, so they are exact
//! and the same on every run. That makes the growth exact too: counts that
//! grow as a polynomial of degree `d` in a knob have a `d + 1`-th finite
//! difference of zero over evenly spaced sizes.
//!
//! Each test snapshots every count that grows along a knob, with how it
//! grows, so that a query that starts or stops growing shows up in review as
//! a change of the snapshot. Work that grows faster than linearly fails the
//! test outright, since no edit should cost that. Why the work in a snapshot
//! grows, where it is by design, is in a comment of its template.

use std::{collections::BTreeMap, fmt::Write as _, path::Path};

use qbice::{
    ExecutionStyle, Observability, QueryStatistics, Statistics, StatisticsLevel, StatisticsReset,
};
use rayc_diagnostic::{ByteIndex, Rendered};
use rayc_driver::{
    Session,
    term::{ReportTerm, render_plain},
};
use rayc_target::{Arguments, Input};
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

use crate::scenario::{Scenario, write_files};

mod scenario;

/// The number of evenly spaced sizes measured along each knob, from its base
/// size: enough to check a linear growth twice.
const SIZES: usize = 4;

/// A test for each scenario of `scenarios`, named after its template, and a
/// test that every template has one.
macro_rules! scenarios {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                check_scenario(stringify!($name));
            }
        )*

        #[test]
        fn every_scenario_has_a_test() {
            let mut tested = vec![$(stringify!($name)),*];
            tested.sort_unstable();
            assert_eq!(
                scenario::names(),
                tested,
                "every template in `scenarios` should be listed in `scenarios!`"
            );
        }
    };
}

scenarios! {
    unchanged_rewrite,
    function_body_edit,
    leading_comment_moves_every_item,
    struct_field_type_edit,
    add_function,
    remove_function,
}

/// Checks how the work of the edit of the scenario `name` grows, and
/// snapshots it with what the edit changes and the diagnostics of the
/// program.
///
/// With `RAYC_SCENARIO_DUMP` set to a directory, it first writes the base
/// program before and after the edit to `before` and `after` in the
/// directory's subdirectory `name`, to read or check by hand.
fn check_scenario(name: &str) {
    let scenario = Scenario::load(name);
    if let Some(dump) = std::env::var_os("RAYC_SCENARIO_DUMP") {
        scenario.dump(&scenario.base_sizes(), &Path::new(&dump).join(name));
    }

    let checked = run(check_growth(&scenario));

    // the snapshot comes first: where it differs, such as by a diagnostic
    // nobody expected, it explains the other failures best
    let mut settings = insta::Settings::clone_current();
    settings.set_input_file(scenario::template_path(name));
    settings.bind(|| insta::assert_snapshot!(name, checked.snapshot));

    assert!(checked.failures.is_empty(), "{}", checked.failures.join("\n"));
}

/// How a count grows along a knob, over evenly spaced sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Growth {
    /// The count is the same at every size.
    Constant,

    /// The count grows by the same amount with each step.
    Linear,

    /// The count grows by more with each step, or irregularly.
    Faster,
}

impl Growth {
    /// How `values`, taken at evenly spaced sizes, grow: by the lowest order
    /// of their finite differences that is zero throughout.
    fn of(values: &[usize]) -> Self {
        let is_zero = |order| differences(values, order).iter().all(|difference| *difference == 0);

        if is_zero(1) {
            Self::Constant
        } else if is_zero(2) {
            Self::Linear
        } else {
            Self::Faster
        }
    }
}

/// A count of the work of a query type whose growth is checked. The edges
/// checked and the queries verified clean are left out: a repair visits every
/// query that the check reads, so they grow with the program for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Count {
    Fresh,
    RecomputedChanged,
    RecomputedUnchanged,
    DirtiedEdges,
}

impl Count {
    const ALL: [Self; 4] =
        [Self::Fresh, Self::RecomputedChanged, Self::RecomputedUnchanged, Self::DirtiedEdges];

    /// The count in `statistics`.
    const fn of(self, statistics: &QueryStatistics) -> usize {
        match self {
            Self::Fresh => statistics.fresh(),
            Self::RecomputedChanged => statistics.recomputed_changed(),
            Self::RecomputedUnchanged => statistics.recomputed_unchanged(),
            Self::DirtiedEdges => statistics.dirtied_edges(),
        }
    }

    const fn describe(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::RecomputedChanged => "recomputed changed",
            Self::RecomputedUnchanged => "recomputed unchanged",
            Self::DirtiedEdges => "dirtied edges",
        }
    }
}

/// How the work of the edit of a scenario grows, checked.
struct Checked {
    /// What the edit changes, the diagnostics of the base program before and
    /// after it, and every count that grows along a knob, with how it grows,
    /// by knob and query type.
    snapshot: String,

    /// Why the test fails, other than by its snapshot, one message each.
    failures: Vec<String>,
}

/// Measures the edit of `scenario` on its base program, and on the program
/// grown along each of its knobs in steps of its base size, and snapshots how
/// every count of every query type grows along each knob.
///
/// Fails if the edit changes nothing, unless it must cost no work, and lists
/// a failure if the same edit on the same program does different work or
/// reports different diagnostics, if a grown program has diagnostics where
/// the base one has none or the other way around, if any count grows faster
/// than linearly,
/// or if an edit that must cost no work does some.
async fn check_growth(scenario: &Scenario) -> Checked {
    let header = scenario.header();
    let base = scenario.base_sizes();
    let mut failures = Vec::new();

    // an edit that changes nothing does no work, which would pass for an
    // edit whose work is all constant
    let diff = scenario::diff(&scenario.render(&base, false), &scenario.render(&base, true));
    assert!(
        !diff.is_empty() || header.no_work,
        "the edit should change the program, or set `no_work = true` if it means not to"
    );

    let at_base = measure(scenario, &base).await;

    if header.no_work {
        let total = at_base.statistics.total();
        if Count::ALL.map(|count| count.of(&total)) != [0; 4] {
            failures.push(format!("the edit should cost no work: {total:?}"));
        }
    }

    // the counts are only worth comparing if the same edit on the same
    // program always does the same work
    let again = measure(scenario, &base).await;
    let runs = [at_base.statistics.clone(), again.statistics];
    let mut differing = String::new();
    for query in queries_of(&runs) {
        for count in Count::ALL {
            let values = values_of(&runs, query, count);
            if Growth::of(&values) != Growth::Constant {
                writeln!(differing, "  {query}: {} {values:?}", count.describe()).unwrap();
            }
        }
    }
    if (&at_base.before, &at_base.after) != (&again.before, &again.after) {
        writeln!(differing, "  the diagnostics").unwrap();
    }
    if !differing.is_empty() {
        failures
            .push(format!("the same edit on the same program did different work:\n{differing}"));
    }

    let mut snapshot = format!("edit: {}\n", header.edit);
    let diff: Vec<_> = Some(diff).filter(|diff| !diff.is_empty()).into_iter().collect();
    write_section(&mut snapshot, &format!("diff at {}", scenario.describe_sizes(&base)), &diff);
    write_section(&mut snapshot, "diagnostics before the edit", &at_base.before);
    write_section(&mut snapshot, "diagnostics after the edit", &at_base.after);

    for (index, knob) in header.knobs.iter().enumerate() {
        let sizes: Vec<_> = (1..=SIZES).map(|multiple| knob.base * multiple).collect();
        let mut series = vec![at_base.statistics.clone()];
        for &size in &sizes[1..] {
            let mut grown = base.clone();
            grown[index] = size;
            let measured = measure(scenario, &grown).await;

            // a grown program with diagnostics where the base program has
            // none, or the other way around, checks something else, such as a
            // template that renders a broken program only when large; how
            // many there are may grow, such as an error for each user of
            // what the edit breaks
            if (measured.before.is_empty(), measured.after.is_empty())
                != (at_base.before.is_empty(), at_base.after.is_empty())
            {
                let mut message = format!(
                    "at {}, the program has {} diagnostics before the edit and {} after it, \
                     unlike {} and {} at the base sizes:\n",
                    scenario.describe_sizes(&grown),
                    measured.before.len(),
                    measured.after.len(),
                    at_base.before.len(),
                    at_base.after.len(),
                );
                write_section(&mut message, "diagnostics before the edit", &measured.before);
                write_section(&mut message, "diagnostics after the edit", &measured.after);
                failures.push(message);
            }

            series.push(measured.statistics);
        }

        let along = format!("{} {sizes:?}", knob.name);
        report(&along, &series);

        let mut growing = Vec::new();
        let mut faster = String::new();
        for query in queries_of(&series) {
            let mut linear = Vec::new();
            for count in Count::ALL {
                let values = values_of(&series, query, count);
                match Growth::of(&values) {
                    Growth::Constant => {}
                    Growth::Linear => linear.push(count),
                    Growth::Faster => writeln!(
                        faster,
                        "  {query}: {} {values:?}\n    grew by {:?} per step",
                        count.describe(),
                        differences(&values, 1),
                    )
                    .unwrap(),
                }
            }

            if !linear.is_empty() {
                let style = series.iter().find_map(|statistics| statistics.execution_style(query));
                growing.push(GrowingQuery { style, query, linear });
            }
        }
        if !faster.is_empty() {
            failures.push(format!(
                "the work of the edit grew faster than linearly along {along}:\n{faster}"
            ));
        }

        write!(snapshot, "\nalong {along}:\n  adds: {}\n", knob.adds).unwrap();
        write_growing(&mut snapshot, &mut growing);
    }

    Checked { snapshot, failures }
}

/// Writes `title` to `snapshot`, and `items` below it, indented and a blank
/// line apart, or that there are none.
fn write_section(snapshot: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        writeln!(snapshot, "{title}: none").unwrap();
        return;
    }

    writeln!(snapshot, "{title}:").unwrap();
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            snapshot.push('\n');
        }
        for line in item.lines() {
            writeln!(snapshot, "{}", format!("  {line}").trim_end()).unwrap();
        }
    }
}

/// A query type with counts that grow linearly along a knob.
struct GrowingQuery {
    /// How the engine executes it, or `None` for an input.
    style: Option<ExecutionStyle>,

    /// Its name, as [`std::any::type_name`] names it.
    query: &'static str,

    /// The counts that grow linearly, in the order of [`Count::ALL`].
    linear: Vec<Count>,
}

impl GrowingQuery {
    /// The kind of query, which groups the query types of a snapshot.
    const fn kind(&self) -> (u8, &'static str) {
        match self.style {
            Some(ExecutionStyle::Firewall) => (0, "firewalls"),
            Some(ExecutionStyle::Projection) => (1, "projections"),
            Some(ExecutionStyle::Normal) => (2, "normal queries"),
            Some(ExecutionStyle::ExternalInput) => (3, "external inputs"),
            None => (4, "inputs"),
        }
    }

    /// The crate of the query type, and its path in the crate.
    fn crate_and_path(&self) -> (&'static str, &'static str) {
        self.query.split_once("::").unwrap_or(("", self.query))
    }
}

/// Writes the query types of `growing` to `snapshot`, grouped by their kind
/// and then by their crate, with the query types of a crate whose counts
/// grow alike together and their growth aligned, or that nothing grows.
fn write_growing(snapshot: &mut String, growing: &mut [GrowingQuery]) {
    if growing.is_empty() {
        snapshot.push_str("  grows: nothing\n");
        return;
    }

    snapshot.push_str("  grows:\n");

    // the query types of a crate whose counts grow alike are listed together
    growing.sort_by(|a, b| {
        let (a_crate, a_path) = a.crate_and_path();
        let (b_crate, b_path) = b.crate_and_path();
        (a.kind(), a_crate, &a.linear, a_path).cmp(&(b.kind(), b_crate, &b.linear, b_path))
    });

    for of_kind in growing.chunk_by(|a, b| a.kind() == b.kind()) {
        writeln!(snapshot, "    {}:", of_kind[0].kind().1).unwrap();

        for of_crate in of_kind.chunk_by(|a, b| a.crate_and_path().0 == b.crate_and_path().0) {
            writeln!(snapshot, "      {}:", of_crate[0].crate_and_path().0).unwrap();

            // the growth of the query types of a crate starts in one column
            let width =
                of_crate.iter().map(|query| query.crate_and_path().1.len()).max().unwrap_or(0);
            for query in of_crate {
                writeln!(
                    snapshot,
                    "        {:width$} linear in {}",
                    query.crate_and_path().1,
                    query
                        .linear
                        .iter()
                        .map(|count| count.describe())
                        .collect::<Vec<_>>()
                        .join(", "),
                )
                .unwrap();
            }
        }
    }
}

/// What checking a program, editing it and checking it again did.
struct Measurement {
    /// The diagnostics of the check before the edit, rendered.
    before: Vec<String>,

    /// The diagnostics of the check after the edit, rendered.
    after: Vec<String>,

    /// The work the engine did for the check after the edit.
    statistics: Work,
}

/// The work the engine did, by the name of each query type, which the
/// snapshots sort and group by.
#[derive(Debug, Clone)]
struct Work {
    by_name: BTreeMap<&'static str, QueryStatistics>,

    /// The execution style of each query type with a registered executor.
    styles: BTreeMap<&'static str, ExecutionStyle>,

    total: QueryStatistics,
}

impl Work {
    /// Names the query types of `statistics` through `session`.
    ///
    /// The inputs have no executor to name them and are left out: they are
    /// never executed and call nothing, so they only count as verified
    /// clean, which the snapshots do not follow. `total` still counts them.
    fn new(statistics: &Statistics, session: &Session) -> Self {
        let named: Vec<_> = statistics
            .iter()
            .filter_map(|(query_type, work)| {
                Some((query_type, session.query_type_name(query_type)?, *work))
            })
            .collect();

        let by_name = named.iter().map(|&(_, name, work)| (name, work)).collect();
        let styles = named
            .iter()
            .filter_map(|&(query_type, name, _)| {
                session.query_execution_style(query_type).map(|style| (name, style))
            })
            .collect();

        Self { by_name, styles, total: statistics.total() }
    }

    fn get(&self, name: &str) -> Option<&QueryStatistics> { self.by_name.get(name) }

    /// How the engine executes the query type `name`, or `None` for an input.
    fn execution_style(&self, name: &str) -> Option<ExecutionStyle> {
        self.styles.get(name).copied()
    }

    fn iter(&self) -> impl Iterator<Item = (&'static str, &QueryStatistics)> {
        self.by_name.iter().map(|(name, work)| (*name, work))
    }

    const fn total(&self) -> QueryStatistics { self.total }
}

/// Checks the program of `scenario` at `sizes`, applies its edit, checks it
/// again, and returns the diagnostics of both checks and the work of the
/// second.
async fn measure(scenario: &Scenario, sizes: &[usize]) -> Measurement {
    let directory = tempfile::tempdir().unwrap();
    let before_edit = scenario.render(sizes, false);
    write_files(directory.path(), &before_edit);

    // a root file rather than a project directory, which keeps the engine's
    // storage in memory
    let input = Input::builder()
        .path(directory.path().join("main.ray"))
        .target_seed(0)
        .verify_ir(false)
        .build();

    let mut err_writer = Vec::new();
    // the counters start again from zero when `refresh` commits the edit
    let observability = Observability::builder()
        .statistics(StatisticsLevel::ByQueryType)
        .reset(StatisticsReset::OnCommit)
        .build();
    let session = Session::new_with_observability(
        &Arguments::new_check(input),
        observability,
        &mut ReportTerm::new(&mut err_writer, false),
    )
    .await
    .unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&err_writer)));
    let before = render_diagnostics(&session, &session.check().await, directory.path()).await;

    // count only the work of the check after the edit, which writes every
    // file of the edited program again, changed or not, and removes the files
    // it no longer has
    let after_edit = scenario.render(sizes, true);
    for path in before_edit.keys().filter(|path| !after_edit.contains_key(*path)) {
        std::fs::remove_file(directory.path().join(path)).unwrap();
    }
    write_files(directory.path(), &after_edit);

    let session = session.refresh().await;
    let diagnostics = session.check().await;

    // taken before rendering the diagnostics, whose source map is work of
    // its own
    let statistics = Work::new(&session.statistics(), &session);
    let after = render_diagnostics(&session, &diagnostics, directory.path()).await;

    Measurement { before, after, statistics }
}

/// `diagnostics` of `session`, rendered as the compiler prints them, with
/// paths relative to `directory`, the program's.
///
/// Rendering builds the source map, whose queries would join the work of the
/// edit after it, so it only does when there are diagnostics to render.
async fn render_diagnostics(
    session: &Session,
    diagnostics: &[Rendered<ByteIndex>],
    directory: &Path,
) -> Vec<String> {
    if diagnostics.is_empty() {
        return Vec::new();
    }

    let source_map = session.source_map().await;
    let prefixes = [dunce::canonicalize(directory).unwrap(), directory.to_path_buf()]
        .map(|directory| format!("{}{}", directory.display(), std::path::MAIN_SEPARATOR));

    diagnostics
        .iter()
        .map(|diagnostic| {
            let mut rendered = render_plain(diagnostic, &source_map).trim_end().to_owned();
            for prefix in &prefixes {
                rendered = rendered.replace(prefix.as_str(), "");
            }
            rendered
        })
        .collect()
}

/// The query types that did work at any size of `series`.
fn queries_of(series: &[Work]) -> Vec<&'static str> {
    let mut queries: Vec<_> =
        series.iter().flat_map(|statistics| statistics.iter().map(|(name, _)| name)).collect();
    queries.sort_unstable();
    queries.dedup();
    queries
}

/// The `count` of the query type `query` at each size of `series`.
fn values_of(series: &[Work], query: &str, count: Count) -> Vec<usize> {
    series
        .iter()
        .map(|statistics| count.of(&statistics.get(query).copied().unwrap_or_default()))
        .collect()
}

/// The finite differences of `values` of the given `order`: the differences of
/// each pair of neighbors, taken `order` times.
fn differences(values: &[usize], order: usize) -> Vec<i128> {
    let mut differences: Vec<i128> = values.iter().map(|&value| value as i128).collect();
    for _ in 0..order {
        differences = differences.windows(2).map(|pair| pair[1] - pair[0]).collect();
    }

    differences
}

/// Prints the total work at each size `along` something, shown with
/// `--nocapture`.
fn report(along: &str, series: &[Work]) {
    println!("along {along}:");
    for statistics in series {
        let total = statistics.total();
        println!(
            "  {} executed ({} fresh, {} changed, {} unchanged), {} dirtied edges, {} verified \
             clean, {} edges checked",
            total.executed(),
            total.fresh(),
            total.recomputed_changed(),
            total.recomputed_unchanged(),
            total.dirtied_edges(),
            total.verified_clean(),
            total.edges_checked(),
        );
    }
}

/// Runs `test` on a runtime with the stack that checking a program needs,
/// logging what `RAYC_LOG` selects.
fn run<T>(test: impl Future<Output = T>) -> T {
    let _ =
        tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().with_filter(
                EnvFilter::try_from_env("RAYC_LOG").unwrap_or_else(|_| "ERROR".into()),
            ))
            .try_init();

    stacker::maybe_grow(3 * 1024 * 1024, 8 * 1024 * 1024, || {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(if cfg!(debug_assertions) {
                // the futures of a debug build are large
                8 * 1024 * 1024
            } else {
                2 * 1024 * 1024
            })
            .build()
            .expect("failed to create the Tokio runtime")
            .block_on(test)
    })
}
