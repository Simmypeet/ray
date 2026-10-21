//! This crate contains the information about the target of the compilation.

use std::{
    collections::HashMap,
    hash::Hasher,
    path::{Path, PathBuf},
};

use bon::Builder;
use clap::{Args, Subcommand, builder::styling};
use derive_new::new;
use enum_as_inner::EnumAsInner;
use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, StableHash, program::Registration, storage::intern::Interned,
};
use rand::Rng;
use rayc_hash::FxHashSet;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use siphasher::sip128::Hasher128;

#[cfg(any(test, feature = "arbitrary"))]
pub mod arbitrary;

/// Represents an identifier for a target.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Encode,
    Decode,
    StableHash,
    Identifiable,
)]
pub struct TargetID {
    lo: u64,
    hi: u64,
}

impl TargetID {
    /// Represents a `core` target which is included in every compilation.
    pub const CORE: Self = Self { lo: 0, hi: 0 };

    /// A placeholder target ID commonly used for testings.
    pub const TEST: Self = Self { lo: 1, hi: 0 };

    /// Creates a new [`TargetID`] from the given ID.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn new(id: u128) -> Self {
        assert!(
            id != 0,
            "TargetID(0) is reserved for `core` module, use `TargetID::CORE` to obtain the core \
             target"
        );

        Self { lo: id as u64, hi: (id >> 64) as u64 }
    }

    /// Creates a new [`TargetID`] from the given low and high u64 values.
    #[must_use]
    pub fn from_lo_hi(lo: u64, hi: u64) -> Self {
        assert!(
            lo != 0 || hi != 0,
            "TargetID(0) is reserved for `core` module, use `TargetID::CORE` to obtain the core \
             target"
        );

        Self { lo, hi }
    }

    /// Creates a new [`TargetID`] from the given name.
    #[must_use]
    pub fn from_target_name(name: &str) -> Self {
        let initial_key: u128 = 0xbfb4_e73d_a005_9e37_b30e_e65f_35cf_88b0;
        let mut sip_hasher =
            siphasher::sip128::SipHasher24::new_with_key(&initial_key.to_le_bytes());

        sip_hasher.write(name.as_bytes());

        let hash = sip_hasher.finish128();

        let lo = hash.h1;
        let hi = hash.h2;

        Self { lo, hi }
    }

    /// Creates a new [`Global`] identifier from the given [`TargetID`] and the
    /// given local identifier.
    #[must_use]
    pub const fn make_global<ID>(self, id: ID) -> Global<ID> { Global { id, target_id: self } }
}

/// A struct used for identifying an entity across different targets.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Encode,
    Decode,
    StableHash,
    Identifiable,
    new,
)]
pub struct Global<ID> {
    /// The identifier to the target that the entity is defined in.
    pub target_id: TargetID,

    /// The identifier to the local entity defined within the target.
    pub id: ID,
}

/// The input to the compiler.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Args, Builder)]
pub struct Input {
    /// The source file or project to compile.
    ///
    /// A source file is compiled on its own as the root of the module tree.
    /// A directory, or the current directory when omitted, is compiled as the
    /// project whose `ray.toml` manifest is in it or its nearest parent.
    path: Option<PathBuf>,

    /// The name of the target; if not specified, the target name will be
    /// taken from the project manifest, or inferred from the file name.
    #[clap(short = 't', long = "target")]
    target_name: Option<String>,

    /// The paths to the `plib` library to link to the target.
    #[clap(short = 'l', long = "link")]
    #[builder(default = Vec::new())]
    library_paths: Vec<PathBuf>,

    /// The path to the incremental compilation data; projects default to
    /// `target/inc` in the project directory.
    #[clap(long = "inc")]
    incremental_path: Option<PathBuf>,

    /// Produces the chrome tracing format for the compilation.
    ///
    /// This is primarily used for debugging purposes and can be viewed in
    /// the Chrome browser.
    #[clap(long = "chrome")]
    #[builder(default = false)]
    chrome_tracing: bool,

    /// The seed for the compiler internal ID generation.
    ///
    /// This option is meant to be used internally for testing and debugging
    /// purposes.
    #[clap(long = "target-seed")]
    target_seed: Option<u64>,

    /// Displays the diagnostics without colors.
    #[clap(long = "no-fancy", default_value = "true", action = clap::ArgAction::SetFalse)]
    #[builder(default = true)]
    fancy: bool,

    /// Enables IR verification after function IR finalization.
    ///
    /// This is an internal configuration knob used by tests and tooling. It
    /// is intentionally not exposed as a user-facing CLI flag.
    #[clap(skip = false)]
    #[builder(default = false)]
    verify_ir: bool,
}

/// The output of the compiler.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Args)]
pub struct Output {
    /// The output path of the program. If not specified, the program will be
    /// named after the target and written to the `target` directory of the
    /// project, or to the current working directory for a source file.
    #[clap(short = 'o', long = "output")]
    output: Option<PathBuf>,
}

/// Represents the `run` subcommand of the compiler.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Args)]
pub struct Run {
    /// The input file to run the program on.
    #[clap(flatten)]
    input: Input,

    /// Specifies the output path of the program.
    #[clap(flatten)]
    output: Output,

    /// The optimization level of the compiler.
    #[clap(long = "opt", default_value = "0")]
    opt_level: OptimizationLevel,
}

/// Represents the `check` subcommand of the compiler.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Args)]
pub struct Check {
    /// The input file to run the program on.
    #[clap(flatten)]
    pub input: Input,
}

/// Represents the `build` subcommand of the compiler.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Args)]
pub struct Build {
    /// The input file to run the program on.
    #[clap(flatten)]
    pub input: Input,

    /// Specifies the output path of the program.
    #[clap(flatten)]
    pub output: Output,

    /// The optimization level of the compiler.
    #[clap(long = "opt", default_value = "0")]
    pub opt_level: OptimizationLevel,

    /// Specifies the compilation format of the target; defaults to the kind
    /// in the project manifest, or `bin`.
    #[clap(long = "emit")]
    pub kind: Option<TargetKind>,
}

/// The subcomamnds of the compiler.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Subcommand, EnumAsInner)]
pub enum Command {
    /// Compiles the program as an executable binary and runs it.
    #[clap(name = "run")]
    Run(Run),

    /// Performs semantic analysis on the program and emits the diagnostics.
    #[clap(name = "check")]
    Check(Check),

    /// Builds the program and emits the output (defaults to `bin`).
    #[clap(name = "build")]
    Build(Build),
}

impl Command {
    /// Returns the input file of the command.
    #[must_use]
    pub const fn input(&self) -> &Input {
        match self {
            Self::Run(run) => &run.input,
            Self::Check(check) => &check.input,
            Self::Build(build) => &build.input,
        }
    }

    /// Returns the mutable input file of the command.
    #[must_use]
    pub const fn input_mut(&mut self) -> &mut Input {
        match self {
            Self::Run(run) => &mut run.input,
            Self::Check(check) => &mut check.input,
            Self::Build(build) => &mut build.input,
        }
    }
}

/// Represents the `new` subcommand of the compiler.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Args)]
pub struct New {
    /// The directory to create the project in; it must not exist yet.
    path: PathBuf,

    /// The name of the target; if not specified, the project is named after
    /// its directory.
    #[clap(long = "name")]
    name: Option<String>,

    /// Creates a library project instead of a binary one.
    #[clap(long = "lib")]
    library: bool,

    /// Displays the messages without colors.
    #[clap(long = "no-fancy", default_value = "true", action = clap::ArgAction::SetFalse)]
    fancy: bool,
}

impl New {
    /// Returns the directory to create the project in.
    #[must_use]
    pub fn path(&self) -> &Path { &self.path }

    /// Returns the explicitly requested target name, if one was supplied.
    #[must_use]
    pub fn requested_name(&self) -> Option<&str> { self.name.as_deref() }

    /// Returns whether the project is a library rather than a binary.
    #[must_use]
    pub const fn is_library(&self) -> bool { self.library }

    #[must_use]
    pub const fn fancy(&self) -> bool { self.fancy }
}

/// The command line of the compiler.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, clap::Parser)]
#[command(styles = get_styles(), about = "The Ray compiler")]
pub enum Cli {
    /// A command compiling a program.
    #[command(flatten)]
    Compile(Command),

    /// Creates a new project.
    #[clap(name = "new")]
    New(New),
}

/// Optimizations level for the compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, clap::ValueEnum)]
#[allow(missing_docs)]
pub enum OptimizationLevel {
    #[clap(name = "0")]
    O0,

    #[clap(name = "1")]
    O1,

    #[clap(name = "2")]
    O2,

    #[clap(name = "3")]
    O3,
}

/// The compilation format of the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, clap::ValueEnum)]
pub enum TargetKind {
    /// Compiles as an executable with a main function.
    #[clap(name = "bin")]
    Executable,

    /// Compiles as a library which can be later linked to other targets.
    #[clap(name = "lib")]
    Library,

    /// Compiles as LLVM IR.
    #[clap(name = "llvm")]
    LLvmIR,

    /// Compiles as an object file which can be later linked to create an
    /// executable.
    #[clap(name = "obj")]
    Object,

    /// Compiles as a C source file which can be later compiled to an object
    /// file or an executable.
    #[clap(name = "c")]
    C,
}

impl TargetKind {
    /// Returns the default file name of this kind of artifact built for the
    /// target named `target_name`.
    ///
    /// Unsupported legacy artifact kinds have no default file name.
    #[must_use]
    pub fn default_file_name(self, target_name: &str) -> Option<String> {
        match self {
            Self::Executable => Some(format!("{target_name}{}", std::env::consts::EXE_SUFFIX)),
            Self::Library | Self::LLvmIR => None,
            Self::Object => Some(format!("{target_name}.o")),
            Self::C => Some(format!("{target_name}.c")),
        }
    }
}

/// Represents a CLI arguments invoking the compilation process.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, clap::Parser)]
#[command(styles = get_styles(), about = "The Ray compiler")]
pub struct Arguments {
    /// The subcommand to run.
    #[clap(subcommand, flatten = true)]
    command: Command,
}

impl From<Command> for Arguments {
    fn from(command: Command) -> Self { Self { command } }
}

impl Arguments {
    #[must_use]
    pub const fn new_check(input: Input) -> Self {
        Self { command: Command::Check(Check { input }) }
    }
}

impl Arguments {
    /// Returns the source file or project directory to compile, if one was
    /// supplied.
    #[must_use]
    pub fn path(&self) -> Option<&Path> { self.command.input().path.as_deref() }

    /// Returns the explicitly requested target name, if one was supplied.
    #[must_use]
    pub fn requested_target_name(&self) -> Option<&str> {
        self.command.input().target_name.as_deref()
    }

    /// Returns whether this invocation only checks the input program.
    #[must_use]
    pub const fn is_check_only(&self) -> bool {
        match &self.command {
            Command::Run(_) | Command::Build(_) => false,
            Command::Check(_) => true,
        }
    }

    /// Returns the artifact explicitly requested by this invocation.
    ///
    /// Check-only invocations and builds without `--emit` do not request one.
    #[must_use]
    pub const fn requested_artifact_kind(&self) -> Option<TargetKind> {
        match &self.command {
            Command::Run(_) => Some(TargetKind::Executable),
            Command::Check(_) => None,
            Command::Build(build) => build.kind,
        }
    }

    /// Returns whether the produced executable should be launched.
    #[must_use]
    pub const fn should_run(&self) -> bool {
        match &self.command {
            Command::Run(_) => true,
            Command::Check(_) | Command::Build(_) => false,
        }
    }

    /// Returns the optimization level that applies to this invocation.
    ///
    /// Check-only invocations do not have an optimization level.
    #[must_use]
    pub const fn optimization_level(&self) -> Option<OptimizationLevel> {
        match &self.command {
            Command::Run(run) => Some(run.opt_level),
            Command::Check(_) => None,
            Command::Build(build) => Some(build.opt_level),
        }
    }

    /// Returns the explicitly requested output path, if one was supplied.
    #[must_use]
    pub fn requested_output_path(&self) -> Option<&Path> {
        match &self.command {
            Command::Run(run) => run.output.output.as_deref(),
            Command::Check(_) => None,
            Command::Build(build) => build.output.output.as_deref(),
        }
    }

    #[must_use]
    pub fn incremental_path(&self) -> Option<&Path> {
        self.command.input().incremental_path.as_deref()
    }

    #[must_use]
    pub const fn fancy(&self) -> bool { self.command.input().fancy }

    #[must_use]
    pub const fn verify_ir(&self) -> bool { self.command.input().verify_ir }

    /// Enables verification of finalized function IR for this invocation.
    pub const fn enable_ir_verification(&mut self) { self.command.input_mut().verify_ir = true; }

    #[must_use]
    pub const fn chrome_tracing(&self) -> bool { self.command.input().chrome_tracing }

    #[must_use]
    pub const fn target_seed(&self) -> Option<u64> { self.command.input().target_seed }
}

/// A query input for the path to the root source file of a target.
///
/// The module tree of the target stems from this file, and the root module
/// owns the directory containing it.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Encode,
    Decode,
    StableHash,
    qbice::Query,
)]
#[value(Interned<Path>)]
#[extend(name = get_target_root_file_path, by_val)]
pub struct RootFilePathKey {
    /// The target ID to retrieve the root file path for.
    pub target_id: TargetID,
}

/// A query input for the name of a target.
///
/// The name is also the name of the root module of the target.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Encode,
    Decode,
    StableHash,
    qbice::Query,
)]
#[value(Interned<str>)]
#[extend(name = get_target_name, by_val)]
pub struct NameKey {
    /// The target ID to retrieve the name for.
    pub target_id: TargetID,
}

#[must_use]
const fn get_styles() -> clap::builder::Styles {
    clap::builder::Styles::styled()
        .usage(
            styling::Style::new()
                .bold()
                .underline()
                .fg_color(Some(styling::Color::Ansi(styling::AnsiColor::Yellow))),
        )
        .header(
            styling::Style::new()
                .bold()
                .underline()
                .fg_color(Some(styling::Color::Ansi(styling::AnsiColor::Cyan))),
        )
        .literal(
            styling::Style::new().fg_color(Some(styling::Color::Ansi(styling::AnsiColor::Green))),
        )
        .invalid(
            styling::Style::new()
                .bold()
                .fg_color(Some(styling::Color::Ansi(styling::AnsiColor::Red))),
        )
        .error(
            styling::Style::new()
                .bold()
                .fg_color(Some(styling::Color::Ansi(styling::AnsiColor::Red))),
        )
        .valid(
            styling::Style::new()
                .bold()
                .underline()
                .fg_color(Some(styling::Color::Ansi(styling::AnsiColor::Green))),
        )
        .placeholder(
            styling::Style::new().fg_color(Some(styling::Color::Ansi(styling::AnsiColor::White))),
        )
}

/// A query input for mapping names to their target IDs.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Encode,
    Decode,
    StableHash,
    qbice::Query,
)]
#[value(Interned<HashMap<Interned<str>, TargetID>>)]
#[extend(name = get_target_map, by_val)]
pub struct MapKey;

/// A query for retrieving the linked targets of a given target ID.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Encode,
    Decode,
    StableHash,
    qbice::Query,
)]
#[value(Interned<FxHashSet<TargetID>>)]
#[extend(name = get_linked_targets, by_val)]
pub struct LinkKey {
    /// The target ID to retrieve the linked targets for.
    pub target_id: TargetID,
}

/// A query for retrieving the linked targets of a given target ID.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Encode,
    Decode,
    StableHash,
    qbice::Query,
)]
#[value(u64)]
#[extend(name = get_target_seed, by_val)]
pub struct SeedKey {
    /// The target ID to retrieve the seed for.
    pub target_id: TargetID,
}

/// The seed value for the `core` target.
pub const CORE_TARGET_SEED: u64 = 0x1234_5678_9abc_def0;

/// The executor that uses rabndom number generator to produce a target seed.
#[qbice::executor(config = rayc_qbice::Config)]
#[allow(clippy::unused_async)]
pub async fn target_seed_executor(key: &SeedKey, _: &TrackedEngine) -> u64 {
    if key.target_id == TargetID::CORE {
        return CORE_TARGET_SEED;
    }

    rand::rng().random()
}

#[distributed_slice(RAY_PROGRAM)]
static TARGET_SEED_EXECUTOR: Registration<Config> =
    Registration::new::<SeedKey, TargetSeedExecutor>();

/// A query for retrieving the `TargetID` that's currently being compiled in
/// the current session.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    qbice::Query,
)]
#[value(TargetID)]
#[extend(name = get_local_target_id, by_val)]
pub struct LocalTargetIDKey;

/// A query for determining whether finalized function IR should be verified.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    qbice::Query,
)]
#[value(bool)]
#[extend(name = get_ir_verification, by_val)]
pub struct IRVerificationKey {
    /// The target ID to retrieve the IR verification flag for.
    pub target_id: TargetID,
}

/// A query for retrieving all the target IDs, including the downstream
/// dependencies.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    qbice::Query,
)]
#[value(Interned<FxHashSet<TargetID>>)]
#[extend(name = get_all_target_ids, by_val)]
pub struct AllTargetIDsKey;
