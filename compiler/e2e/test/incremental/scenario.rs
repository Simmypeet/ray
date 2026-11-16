//! The scenarios of the incremental tests: templates in `scenarios` that
//! render a program, before or after an edit, at the sizes of its knobs.
//!
//! A scenario is a [minijinja] template that starts with a header, a comment
//! holding TOML:
//!
//! ```text
//! {#
//! edit = "what the edit changes"
//! no_work = true # optional: the edit must cost no work at all
//!
//! [[knobs]]
//! name = "modules"  # the template variable that holds its size
//! base = 3          # its size in the base program
//! adds = "what growing the program along it adds"
//! #}
//! ```
//!
//! The template renders the program with the size of each knob in the
//! variable of its name, and `edit` false before the edit and true after it.
//! A line `--- file <path>` starts each file of the program, and `main.ray`
//! is its root. The pieces that scenarios share are macros in `shared`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::{Path, PathBuf},
};

use minijinja::{AutoEscape, Environment, UndefinedBehavior, Value, syntax::SyntaxConfig};
use serde::Deserialize;
use similar::TextDiff;

/// The line prefix that starts a file in a rendered scenario.
const FILE_MARKER: &str = "--- file ";

/// The directory of the scenario templates.
pub fn directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("test/incremental/scenarios")
}

/// The path of the template of the scenario `name`.
pub fn template_path(name: &str) -> PathBuf { directory().join(format!("{name}.ray.j2")) }

/// The names of every scenario template, without their extension.
pub fn names() -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(directory())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter_map(|file_name| file_name.strip_suffix(".ray.j2").map(str::to_owned))
        .collect();
    names.sort_unstable();
    names
}

/// The header of a scenario template.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    /// What the edit changes, for snapshots.
    pub edit: String,

    /// Whether the edit must cost no work at all.
    #[serde(default)]
    pub no_work: bool,

    /// The dimensions the program grows along, in the order the test grows
    /// them.
    pub knobs: Vec<Knob>,
}

/// A dimension that a scenario's program grows along.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Knob {
    /// The template variable that holds its size.
    pub name: String,

    /// Its size in the base program.
    pub base: usize,

    /// What growing the program along it adds, for snapshots.
    pub adds: String,
}

/// A scenario template, loaded.
pub struct Scenario {
    name: String,
    header: Header,
    environment: Environment<'static>,
}

/// The files of a rendered program, by their path in its directory.
pub type Files = BTreeMap<String, String>;

impl Scenario {
    /// Loads the scenario `name` from its template in [`directory`].
    pub fn load(name: &str) -> Self {
        let path = template_path(name);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let header = parse_header(&source).unwrap_or_else(|error| {
            panic!("{}: the template should start with a TOML header: {error}", path.display())
        });

        for knob in &header.knobs {
            assert!(knob.name != "edit", "a knob cannot be named `edit`, the edit's variable");
            assert!(knob.base > 0, "the base size of `{}` should be positive", knob.name);
        }

        let mut environment = Environment::new();
        environment.set_loader(minijinja::path_loader(directory()));
        environment.set_auto_escape_callback(|_| AutoEscape::None);
        environment.set_undefined_behavior(UndefinedBehavior::Strict);
        environment.set_syntax(
            SyntaxConfig::builder()
                // a block tag on a line of its own leaves no blank line
                // behind, which keeps the indentation of the program intact
                .trim_blocks(true)
                .lstrip_blocks(true)
                .keep_trailing_newline(true)
                .build()
                .unwrap(),
        );

        Self { name: name.to_owned(), header, environment }
    }

    pub const fn header(&self) -> &Header { &self.header }

    /// The sizes of the base program, one for each knob of the header.
    pub fn base_sizes(&self) -> Vec<usize> {
        self.header.knobs.iter().map(|knob| knob.base).collect()
    }

    /// The knobs of the header with their size in `sizes`, for messages.
    pub fn describe_sizes(&self, sizes: &[usize]) -> String {
        let knobs: Vec<_> = self
            .header
            .knobs
            .iter()
            .zip(sizes)
            .map(|(knob, size)| format!("{} = {size}", knob.name))
            .collect();
        knobs.join(", ")
    }

    /// Writes the program at `sizes` before the edit to `directory/before`
    /// and after it to `directory/after`, replacing what they held.
    pub fn dump(&self, sizes: &[usize], directory: &Path) {
        for (edited, subdirectory) in [(false, "before"), (true, "after")] {
            let path = directory.join(subdirectory);
            if path.exists() {
                std::fs::remove_dir_all(&path).unwrap();
            }
            write_files(&path, &self.render(sizes, edited));
        }
    }

    /// Renders the program at `sizes`, one for each knob of the header,
    /// before the edit or after it.
    pub fn render(&self, sizes: &[usize], edited: bool) -> Files {
        let mut context: BTreeMap<String, Value> = self
            .header
            .knobs
            .iter()
            .zip(sizes)
            .map(|(knob, &size)| (knob.name.clone(), Value::from(size)))
            .collect();
        context.insert("edit".to_owned(), Value::from(edited));

        let rendered = self
            .environment
            .get_template(&format!("{}.ray.j2", self.name))
            .and_then(|template| template.render(context))
            .unwrap_or_else(|error| panic!("{}: {error:#}", self.name));

        let files = split_files(&rendered);
        assert!(
            files.contains_key("main.ray"),
            "{}: the program should have a `main.ray`",
            self.name
        );
        files
    }
}

/// Writes `files` to `directory`, creating the directories they need.
pub fn write_files(directory: &Path, files: &Files) {
    for (path, text) in files {
        let path = directory.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

/// A unified diff of every file that `after` adds, removes or changes from
/// `before`, or nothing if they are the same.
pub fn diff(before: &Files, after: &Files) -> String {
    let paths: BTreeSet<_> = before.keys().chain(after.keys()).collect();

    let mut diff = String::new();
    for path in paths {
        let old = before.get(path).map_or("", String::as_str);
        let new = after.get(path).map_or("", String::as_str);
        if old == new {
            continue;
        }

        let old_name = if before.contains_key(path) { path.as_str() } else { "/dev/null" };
        let new_name = if after.contains_key(path) { path.as_str() } else { "/dev/null" };
        write!(
            diff,
            "{}",
            TextDiff::from_lines(old, new)
                .unified_diff()
                .context_radius(2)
                .header(old_name, new_name)
        )
        .unwrap();
    }

    diff
}

/// The header of the template `source`: the TOML in the comment it starts
/// with.
fn parse_header(source: &str) -> Result<Header, String> {
    let comment = source.trim_start().strip_prefix("{#").ok_or("no comment at the start")?;
    let (toml, _) = comment.split_once("#}").ok_or("the comment does not end")?;
    toml::from_str(toml).map_err(|error| error.to_string())
}

/// The files of the rendered program `rendered`, each starting at a
/// [`FILE_MARKER`] line, and ending in one newline.
fn split_files(rendered: &str) -> Files {
    let mut files = Files::new();
    let mut current: Option<(&str, String)> = None;

    let mut finish = |current: Option<(&str, String)>| {
        if let Some((path, text)) = current {
            let previous = files.insert(path.to_owned(), format!("{}\n", text.trim_end()));
            assert!(previous.is_none(), "the program has `{path}` twice");
        }
    };

    for line in rendered.split_inclusive('\n') {
        // `trim_blocks` removes the line break after any block tag, so an
        // `{% if %}` or `{% endif %}` at the end of a line of text joins the
        // next line to it; a condition inside a line takes `{{ a if c else b }}`
        assert!(
            !line.get(1..).is_some_and(|rest| rest.contains(FILE_MARKER)),
            "a file marker should start its line, but a block tag at the end of the line before \
             it may have joined them: {line:?}"
        );

        if let Some(path) = line.strip_prefix(FILE_MARKER) {
            finish(current.take());
            current = Some((path.trim(), String::new()));
        } else if let Some((_, text)) = &mut current {
            text.push_str(line);
        } else {
            assert!(line.trim().is_empty(), "text before the first file: {line:?}");
        }
    }
    finish(current);

    files
}
