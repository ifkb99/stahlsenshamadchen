//! The heatmap's command line, read strictly.
//!
//! The first parser looked up the flags it knew and ignored every other
//! word, so `--game 4` (for `--games`) fought the default sixteen battles
//! and printed numbers that looked like the experiment asked for, and a
//! playtester read conclusions into a run she never made. Now every word
//! is accounted for before a mod loads or a battle is fought: a flag nobody
//! knows, a flag without its value, a stray word, a value that does not
//! read, and a flag the chosen run would not read are all refused, naming
//! the word.
//!
//! [`FLAGS`] is the one list. The parser looks every flag up in it and
//! [`usage`] prints it, so the help cannot offer a flag the parser refuses
//! or leave out one it takes; a test holds the module doc comment in
//! `main.rs` to the same list.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::str::FromStr;

use tactics_core::harness::overrides::Override;

use crate::tally::Cell;

/// The experiment a run makes. They are exclusive: one picture per run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// AI battles, fought here: the default.
    Fight,
    /// Hand games from `examples/command` (`--seat`).
    Seats,
    /// B minus A from two `--json` files (`--diff`).
    Diff,
    /// The ground's sight lines, no battle at all (`--sight`).
    Sight,
}

impl Kind {
    /// How a refusal names the run ("`--games` means nothing to a sight
    /// survey").
    fn describe(self) -> &'static str {
        match self {
            Kind::Fight => "AI battles",
            Kind::Seats => "hand games (--seat)",
            Kind::Diff => "a diff (--diff)",
            Kind::Sight => "a sight survey (--sight)",
        }
    }

    /// How the usage tags a flag that only some runs read.
    fn tag(self) -> &'static str {
        match self {
            Kind::Fight => "AI battles",
            Kind::Seats => "--seat",
            Kind::Diff => "--diff",
            Kind::Sight => "--sight",
        }
    }
}

/// What a flag takes after it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Takes {
    /// Nothing: being there is the value.
    Switch,
    /// One word, and the flag at most once: two `--games` would leave the
    /// reader guessing which one ran.
    Value,
    /// One word each time, and the flag as often as wanted (`--set`).
    Repeated,
    /// One or more words, up to the next `--flag`; given again, the words
    /// add up (`--seat a --seat b` is `--seat a b`).
    Words,
}

/// One flag the heatmap reads.
struct Flag {
    name: &'static str,
    takes: Takes,
    /// What its value looks like in the usage; empty for a switch.
    value: &'static str,
    /// The runs that read it. Any other run refuses it, because a flag
    /// that is silently not read is the mistake this parser exists for.
    kinds: &'static [Kind],
    help: &'static str,
}

const EVERY: &[Kind] = &[Kind::Fight, Kind::Seats, Kind::Diff, Kind::Sight];
const ON_A_MAP: &[Kind] = &[Kind::Fight, Kind::Seats, Kind::Sight];
const BATTLES: &[Kind] = &[Kind::Fight, Kind::Seats];
const FIGHT: &[Kind] = &[Kind::Fight];
const SEATS: &[Kind] = &[Kind::Seats];
const DIFF: &[Kind] = &[Kind::Diff];
const SIGHT: &[Kind] = &[Kind::Sight];

/// Every flag, in the order the usage prints them. `--help` / `-h` are not
/// here: they are answered before anything else is read.
const FLAGS: &[Flag] = &[
    Flag {
        name: "--map",
        takes: Takes::Value,
        value: "<map>",
        kinds: ON_A_MAP,
        help: "the battle map (default slice_hollerbach)",
    },
    Flag {
        name: "--games",
        takes: Takes::Value,
        value: "<n>",
        kinds: FIGHT,
        help: "AI battles to fight (default 16)",
    },
    Flag {
        name: "--seed",
        takes: Takes::Value,
        value: "<n>",
        kinds: FIGHT,
        help: "the first battle's seed; the rest count up from it (default 1)",
    },
    Flag {
        name: "--planner",
        takes: Takes::Value,
        value: "<name>",
        kinds: FIGHT,
        help: "both sides' planner (default utility)",
    },
    Flag {
        name: "--swap",
        takes: Takes::Switch,
        value: "",
        kinds: FIGHT,
        help: "exchange the doctrines (massed_armor against elastic_defense)",
    },
    Flag {
        name: "--map-ai",
        takes: Takes::Switch,
        value: "",
        kinds: FIGHT,
        help: "use the map's own `ai` blocks where it gives them",
    },
    Flag {
        name: "--seat",
        takes: Takes::Words,
        value: "<dir>...",
        kinds: SEATS,
        help: "read hand games from examples/command instead of fighting",
    },
    Flag {
        name: "--vehicle",
        takes: Takes::Value,
        value: "<id>",
        kinds: BATTLES,
        help: "count only units of this chassis (the ending still counts)",
    },
    Flag {
        name: "--timeline",
        takes: Takes::Switch,
        value: "",
        kinds: BATTLES,
        help: "print every unit, round by round, as text",
    },
    Flag {
        name: "--label",
        takes: Takes::Value,
        value: "<name>",
        kinds: BATTLES,
        help: "name this run, for --diff to tell two runs apart",
    },
    Flag {
        name: "--json",
        takes: Takes::Value,
        value: "<file>",
        kinds: BATTLES,
        help: "keep the counts in a file, for --diff",
    },
    Flag {
        name: "--diff",
        takes: Takes::Words,
        value: "<a.json> <b.json>",
        kinds: DIFF,
        help: "draw B minus A from two --json files",
    },
    Flag {
        name: "--sight",
        takes: Takes::Switch,
        value: "",
        kinds: SIGHT,
        help: "read only the ground: what each hex overlooks",
    },
    Flag {
        name: "--range",
        takes: Takes::Value,
        value: "<n>",
        kinds: SIGHT,
        help: "how far a hex sees, in hexes (default 8)",
    },
    Flag {
        name: "--from",
        takes: Takes::Words,
        value: "<col,row>...",
        kinds: SIGHT,
        help: "what these hexes see (default each side's start and each objective)",
    },
    Flag {
        name: "--across",
        takes: Takes::Words,
        value: "<a-b> <c-d>",
        kinds: SIGHT,
        help: "how much of rows c-d can be seen from rows a-b",
    },
    Flag {
        name: "--set",
        takes: Takes::Repeated,
        value: "<path=value>",
        kinds: EVERY,
        help: "any mod field, as `balance --set`; repeatable",
    },
    Flag {
        name: "--out",
        takes: Takes::Value,
        value: "<file>",
        kinds: EVERY,
        help: "where the SVG goes (default target/heatmap.svg)",
    },
];

/// What the command line asked for.
#[derive(Debug)]
pub enum Parsed {
    /// `--help` or `-h`: print [`usage`] and stop.
    Help,
    Run(Args),
}

/// One run, every value read and checked.
#[derive(Debug)]
pub struct Args {
    /// `--set`, in the order given: a later one wins a shared path.
    pub sets: Vec<Override>,
    pub out: PathBuf,
    pub map: String,
    pub run: Run,
}

/// The experiment, with the flags only it reads.
#[derive(Debug)]
pub enum Run {
    Fight {
        games: u64,
        first: u64,
        planner: String,
        map_ai: bool,
        swap: bool,
        record: Record,
    },
    Seats {
        dirs: Vec<PathBuf>,
        record: Record,
    },
    Diff {
        a: PathBuf,
        b: PathBuf,
    },
    Sight {
        range: u32,
        from: Vec<Cell>,
        /// `--across`: the band looked from, then the band looked at.
        across: Option<[(i32, i32); 2]>,
    },
}

/// What a run of battles (fought or read) keeps and prints.
#[derive(Debug, Default)]
pub struct Record {
    pub vehicle: Option<String>,
    pub timeline: bool,
    pub label: Option<String>,
    pub json: Option<String>,
}

/// Why a command line was refused. Each names the word to fix.
#[derive(Debug, thiserror::Error)]
pub enum ArgError {
    #[error("unknown flag `{word}`{}", meant.map(|m| format!(" (did you mean `{m}`?)")).unwrap_or_default())]
    Unknown {
        word: String,
        meant: Option<&'static str>,
    },
    #[error("stray argument `{0}`: every word must follow a flag that takes it")]
    Stray(String),
    #[error("`{flag}` needs {value}")]
    MissingValue {
        flag: &'static str,
        value: &'static str,
    },
    #[error("`{0}` is given twice")]
    Twice(&'static str),
    #[error("`{flag} {value}`: {why}")]
    BadValue {
        flag: &'static str,
        value: String,
        why: String,
    },
    #[error("`{0}` and `{1}` are different runs; make one at a time")]
    TwoRuns(&'static str, &'static str),
    #[error("`{flag}` means nothing to {run}, which would silently not read it")]
    NotRead {
        flag: &'static str,
        run: &'static str,
    },
}

/// The help, built from [`FLAGS`].
pub fn usage() -> String {
    let mut text = String::from(
        "usage: heatmap [flags]\n\
         \x20 (cargo run --release -p tactics_core --example heatmap -- [flags])\n\
         \n\
         Where a map's battles happen. A run fights AI battles unless --seat,\n\
         --diff or --sight makes it another; a flag that run would not read is\n\
         refused, as is any word that is not a flag below or its value.\n\
         \n",
    );
    for flag in FLAGS {
        let shape = format!("{} {}", flag.name, flag.value);
        let only = if flag.kinds == EVERY {
            String::new()
        } else {
            let tags: Vec<&str> = flag.kinds.iter().map(|k| k.tag()).collect();
            format!(" [{}]", tags.join(", "))
        };
        writeln!(text, "  {:<28}{}{only}", shape.trim_end(), flag.help).expect("a string takes it");
    }
    writeln!(text, "  {:<28}print this and stop", "-h, --help").expect("a string takes it");
    text
}

/// Read a command line (without the program name).
pub fn parse(args: &[String]) -> Result<Parsed, ArgError> {
    // Help first, even beside a mistake: someone asking how to spell a
    // flag has usually just misspelt one.
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Ok(Parsed::Help);
    }
    let given = collect(args)?;
    let present = |name: &str| given.iter().any(|(f, _)| f.name == name);

    let runs: Vec<(&'static str, Kind)> = [
        ("--seat", Kind::Seats),
        ("--diff", Kind::Diff),
        ("--sight", Kind::Sight),
    ]
    .into_iter()
    .filter(|(name, _)| present(name))
    .collect();
    let kind = match runs[..] {
        [] => Kind::Fight,
        [(_, kind)] => kind,
        [(a, _), (b, _), ..] => return Err(ArgError::TwoRuns(a, b)),
    };
    if let Some((flag, _)) = given.iter().find(|(f, _)| !f.kinds.contains(&kind)) {
        return Err(ArgError::NotRead {
            flag: flag.name,
            run: kind.describe(),
        });
    }

    let words = |name: &str| -> &[String] {
        given
            .iter()
            .find(|(f, _)| f.name == name)
            .map_or(&[][..], |(_, v)| &v[..])
    };
    let one = |name: &str| words(name).first().cloned();
    let on = |name: &str| present(name);

    let sets = words("--set")
        .iter()
        .map(|text| Override::parse(text).map_err(|why| bad("--set", text, why)))
        .collect::<Result<_, _>>()?;
    let record = || Record {
        vehicle: one("--vehicle"),
        timeline: on("--timeline"),
        label: one("--label"),
        json: one("--json"),
    };
    let run = match kind {
        Kind::Fight => Run::Fight {
            games: number(one("--games"), "--games", 16)?,
            first: number(one("--seed"), "--seed", 1)?,
            planner: one("--planner").unwrap_or_else(|| "utility".into()),
            map_ai: on("--map-ai"),
            swap: on("--swap"),
            record: record(),
        },
        Kind::Seats => Run::Seats {
            dirs: words("--seat").iter().map(PathBuf::from).collect(),
            record: record(),
        },
        Kind::Diff => match words("--diff") {
            [a, b] => Run::Diff {
                a: a.into(),
                b: b.into(),
            },
            files => {
                return Err(bad(
                    "--diff",
                    &files.join(" "),
                    format!("takes two json files, not {}", files.len()),
                ));
            }
        },
        Kind::Sight => Run::Sight {
            range: number(one("--range"), "--range", 8)?,
            from: words("--from")
                .iter()
                .map(|text| cell(text).ok_or_else(|| bad("--from", text, "a hex is col,row")))
                .collect::<Result<_, _>>()?,
            across: match words("--across") {
                [] => None,
                [from, to] => Some([band(from)?, band(to)?]),
                bands => {
                    return Err(bad(
                        "--across",
                        &bands.join(" "),
                        format!("takes two bands of rows, not {}", bands.len()),
                    ));
                }
            },
        },
    };
    Ok(Parsed::Run(Args {
        sets,
        out: PathBuf::from(one("--out").unwrap_or_else(|| "target/heatmap.svg".into())),
        map: one("--map").unwrap_or_else(|| "slice_hollerbach".into()),
        run,
    }))
}

/// Walk the words: each must be a known flag or a value one takes.
fn collect(args: &[String]) -> Result<Vec<(&'static Flag, Vec<String>)>, ArgError> {
    // A value never starts with `--`: `--map --games 4` is a map missing,
    // not a map called `--games`. A single dash may start one (`-3`).
    let is_value = |w: &&String| !w.starts_with("--");
    let mut given: Vec<(&'static Flag, Vec<String>)> = Vec::new();
    let mut rest = args.iter().peekable();
    while let Some(word) = rest.next() {
        let Some(flag) = FLAGS.iter().find(|f| f.name == word) else {
            return Err(if word.starts_with('-') {
                ArgError::Unknown {
                    word: word.clone(),
                    meant: nearest(word),
                }
            } else {
                ArgError::Stray(word.clone())
            });
        };
        let mut values = Vec::new();
        match flag.takes {
            Takes::Switch => {}
            Takes::Value | Takes::Repeated => values.extend(rest.next_if(is_value).cloned()),
            Takes::Words => {
                while let Some(v) = rest.next_if(is_value) {
                    values.push(v.clone());
                }
            }
        }
        if flag.takes != Takes::Switch && values.is_empty() {
            return Err(ArgError::MissingValue {
                flag: flag.name,
                value: flag.value,
            });
        }
        match given.iter_mut().find(|(f, _)| f.name == flag.name) {
            Some((_, old)) if matches!(flag.takes, Takes::Repeated | Takes::Words) => {
                old.extend(values)
            }
            Some(_) => return Err(ArgError::Twice(flag.name)),
            None => given.push((flag, values)),
        }
    }
    Ok(given)
}

fn bad(flag: &'static str, value: &str, why: impl Into<String>) -> ArgError {
    ArgError::BadValue {
        flag,
        value: value.into(),
        why: why.into(),
    }
}

fn number<T: FromStr>(text: Option<String>, flag: &'static str, default: T) -> Result<T, ArgError> {
    match text {
        None => Ok(default),
        Some(t) => t
            .parse()
            .map_err(|_| bad(flag, &t, "is not a whole number")),
    }
}

/// `13,10`: a hex as the map file writes it, column then row.
fn cell(text: &str) -> Option<Cell> {
    let (c, r) = text.split_once(',')?;
    Some([c.parse().ok()?, r.parse().ok()?])
}

/// `4-9`: rows 4 to 9 inclusive.
fn band(text: &str) -> Result<(i32, i32), ArgError> {
    let rows = text
        .split_once('-')
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)));
    match rows {
        Some((a, b)) if a <= b => Ok((a, b)),
        Some(_) => Err(bad("--across", text, "a band runs from the lower row up")),
        None => Err(bad("--across", text, "a band is rows a-b")),
    }
}

/// The known flag a misspelt one most likely meant: within two edits, the
/// closest, the earlier in [`FLAGS`] on a tie.
fn nearest(word: &str) -> Option<&'static str> {
    FLAGS
        .iter()
        .map(|f| (edits(word, f.name), f.name))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, name)| name)
}

/// Levenshtein distance, by characters.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(ca != *cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    fn run(line: &str) -> Args {
        match parse(&words(line)) {
            Ok(Parsed::Run(args)) => args,
            other => panic!("`{line}` should read as a run: {other:?}"),
        }
    }

    fn refusal(line: &str) -> String {
        match parse(&words(line)) {
            Err(e) => e.to_string(),
            Ok(other) => panic!("`{line}` should be refused, read as {other:?}"),
        }
    }

    #[test]
    fn a_misspelt_flag_is_refused_naming_it_and_the_flag_it_meant() {
        let err = refusal("--game 4");
        assert!(err.contains("`--game`"), "{err}");
        assert!(err.contains("did you mean `--games`"), "{err}");
        let err = refusal("--sight --maps x");
        assert!(err.contains("`--maps`"), "{err}");
        // Nothing near: no guess is better than a wrong one.
        let err = refusal("--zzzzzzzz");
        assert!(
            err.contains("`--zzzzzzzz`") && !err.contains("mean"),
            "{err}"
        );
        assert!(refusal("-x").contains("`-x`"));
    }

    #[test]
    fn a_flag_missing_its_value_is_refused() {
        for (line, flag) in [
            ("--games", "--games"),
            ("--map --games 4", "--map"),
            ("--seat", "--seat"),
            ("--diff --json a.json", "--diff"),
            ("--sight --from", "--from"),
            ("--set", "--set"),
        ] {
            let err = refusal(line);
            assert!(err.contains(&format!("`{flag}` needs")), "{line}: {err}");
        }
    }

    #[test]
    fn a_stray_word_is_refused() {
        for (line, word) in [
            ("--games 4 8", "8"),
            ("slice_hollerbach", "slice_hollerbach"),
            ("--swap 3", "3"),
            ("--sight --range 8 9", "9"),
        ] {
            let err = refusal(line);
            assert!(
                err.contains(&format!("stray argument `{word}`")),
                "{line}: {err}"
            );
        }
    }

    #[test]
    fn a_value_that_does_not_read_is_refused() {
        for (line, part) in [
            ("--games four", "`--games four`"),
            ("--seed -1", "`--seed -1`"),
            ("--sight --range far", "`--range far`"),
            ("--sight --from 13", "`--from 13`"),
            ("--sight --across 4-9", "`--across 4-9`"),
            ("--sight --across 4-9 x 12-17", "`--across 4-9 x 12-17`"),
            ("--sight --across 9-4 12-17", "`--across 9-4`"),
            ("--diff a.json", "`--diff a.json`"),
            ("--diff a.json b.json c.json", "takes two json files, not 3"),
            ("--set planner", "`--set planner`"),
        ] {
            let err = refusal(line);
            assert!(err.contains(part), "{line}: {err}");
        }
    }

    #[test]
    fn a_flag_the_run_would_not_read_is_refused() {
        for (line, flag) in [
            ("--sight --games 4", "--games"),
            ("--diff a.json b.json --map m", "--map"),
            ("--diff a.json b.json --timeline", "--timeline"),
            ("--seat d --swap", "--swap"),
            ("--range 8", "--range"),
        ] {
            let err = refusal(line);
            assert!(
                err.contains(&format!("`{flag}` means nothing")),
                "{line}: {err}"
            );
        }
    }

    #[test]
    fn two_runs_at_once_are_refused() {
        let err = refusal("--sight --diff a.json b.json");
        assert!(err.contains("--diff") && err.contains("--sight"), "{err}");
    }

    #[test]
    fn a_flag_given_twice_is_refused_unless_it_is_one_that_repeats() {
        assert!(refusal("--games 4 --games 8").contains("`--games` is given twice"));
        assert!(refusal("--swap --swap").contains("`--swap` is given twice"));
        let args = run("--set planner.a=1 --set balance.b=2");
        let paths: Vec<&str> = args.sets.iter().map(|o| o.path.as_str()).collect();
        assert_eq!(paths, ["planner.a", "balance.b"]);
        match run("--seat a --seat b c").run {
            Run::Seats { dirs, .. } => assert_eq!(dirs.len(), 3),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn help_answers_to_either_spelling_even_beside_a_mistake() {
        for line in ["--help", "-h", "--game 4 --help", "--sight -h"] {
            assert!(matches!(parse(&words(line)), Ok(Parsed::Help)), "{line}");
        }
    }

    #[test]
    fn no_flags_is_the_experiment_the_old_parser_ran() {
        let args = run("");
        assert_eq!(args.map, "slice_hollerbach");
        assert_eq!(args.out, PathBuf::from("target/heatmap.svg"));
        assert!(args.sets.is_empty());
        match args.run {
            Run::Fight {
                games: 16,
                first: 1,
                planner,
                map_ai: false,
                swap: false,
                record,
            } => {
                assert_eq!(planner, "utility");
                assert!(record.vehicle.is_none() && !record.timeline);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn every_spelling_the_old_parser_took_still_reads() {
        let args = run(
            "--map slice_hollerbach --games 4 --seed 7 --planner utility --swap --map-ai \
             --vehicle light_tank --timeline --label wiesel --json a.json \
             --set planner.goal_places=16 --set balance.x=2 --out t/h.svg",
        );
        assert_eq!(args.sets.len(), 2);
        assert_eq!(args.out, PathBuf::from("t/h.svg"));
        match args.run {
            Run::Fight {
                games: 4,
                first: 7,
                map_ai: true,
                swap: true,
                record,
                ..
            } => {
                assert_eq!(record.vehicle.as_deref(), Some("light_tank"));
                assert_eq!(record.label.as_deref(), Some("wiesel"));
                assert_eq!(record.json.as_deref(), Some("a.json"));
                assert!(record.timeline);
            }
            other => panic!("{other:?}"),
        }

        match run("--seat d1 d2 --timeline --json s.json --map slice_hollerbach").run {
            Run::Seats { dirs, record } => {
                assert_eq!(dirs, [PathBuf::from("d1"), PathBuf::from("d2")]);
                assert!(record.timeline);
            }
            other => panic!("{other:?}"),
        }

        match run("--diff a.json b.json --out d.svg --set planner.x=1").run {
            Run::Diff { a, b } => assert_eq!((a, b), ("a.json".into(), "b.json".into())),
            other => panic!("{other:?}"),
        }

        match run("--sight --map m --range 6 --from 13,10 2,-3 --across 4-9 12-17").run {
            Run::Sight {
                range: 6,
                from,
                across,
            } => {
                assert_eq!(from, [[13, 10], [2, -3]]);
                assert_eq!(across, Some([(4, 9), (12, 17)]));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_usage_and_the_doc_comment_name_every_flag_the_parser_takes() {
        let help = usage();
        let doc: String = include_str!("main.rs")
            .lines()
            .filter_map(|l| l.strip_prefix("//!"))
            .collect::<Vec<_>>()
            .join("\n");
        for flag in FLAGS {
            assert!(help.contains(flag.name), "usage lacks {}", flag.name);
            assert!(doc.contains(flag.name), "main.rs's doc lacks {}", flag.name);
        }
        // And the other way: the doc offers nothing the parser refuses.
        for word in doc.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
            if let Some(name) = word.strip_prefix("--").filter(|n| !n.is_empty()) {
                let flag = format!("--{name}");
                assert!(
                    ["--help", "--release", "--example"].contains(&flag.as_str())
                        || FLAGS.iter().any(|f| f.name == flag),
                    "main.rs's doc offers {flag}, which the parser does not take"
                );
            }
        }
    }
}
