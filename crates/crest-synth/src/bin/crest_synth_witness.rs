use crest_synth::testing::{BehavioralMutationCase, BehavioralMutationHarness};
use std::fmt::{Display, Formatter};

const OBSERVATION_MARKER: &str = "CREST_MUTATION_OBSERVATION ";
const USAGE: &str =
    "usage: crest-synth-witness --case <case> --mutant <none|matching-case|matching-counterexample>";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CliArguments {
    case: BehavioralMutationCase,
    mutant_enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CliError(String);

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl Display for CliError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn next_value<I>(arguments: &mut I, option: &str) -> Result<String, CliError>
where
    I: Iterator<Item = String>,
{
    arguments
        .next()
        .ok_or_else(|| CliError::new(format!("missing value for {option}; {USAGE}")))
}

fn parse_arguments<I, S>(arguments: I) -> Result<CliArguments, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut arguments = arguments.into_iter().map(Into::into);
    let mut selected_case = None;
    let mut selected_mutant = None;

    while let Some(option) = arguments.next() {
        match option.as_str() {
            "--case" => {
                if selected_case.is_some() {
                    return Err(CliError::new(format!("duplicate --case option; {USAGE}")));
                }
                let value = next_value(&mut arguments, "--case")?;
                selected_case =
                    Some(BehavioralMutationCase::from_cli(&value).ok_or_else(|| {
                        CliError::new(format!("unsupported case {value:?}; {USAGE}"))
                    })?);
            }
            "--mutant" => {
                if selected_mutant.is_some() {
                    return Err(CliError::new(format!("duplicate --mutant option; {USAGE}")));
                }
                selected_mutant = Some(next_value(&mut arguments, "--mutant")?);
            }
            _ => {
                return Err(CliError::new(format!(
                    "unexpected argument {option:?}; {USAGE}"
                )));
            }
        }
    }

    let case =
        selected_case.ok_or_else(|| CliError::new(format!("missing --case option; {USAGE}")))?;
    let mutant = selected_mutant
        .ok_or_else(|| CliError::new(format!("missing --mutant option; {USAGE}")))?;

    let mutant_enabled = if mutant == "none" {
        false
    } else if mutant == case.as_str() || mutant == case.mutant_cli() {
        // A case accepts its own name or its declared typed counterexample
        // (the refused-topology case's counterexample is the refused change
        // being published anyway).
        true
    } else {
        return Err(CliError::new(format!(
            "mutant {mutant:?} does not match case {:?}; {USAGE}",
            case.as_str()
        )));
    };

    Ok(CliArguments {
        case,
        mutant_enabled,
    })
}

fn execute<I, S>(arguments: I) -> Result<i32, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let arguments = parse_arguments(arguments)?;
    let run = BehavioralMutationHarness::new().run(arguments.case, arguments.mutant_enabled);
    let observation = run
        .observation_json()
        .map_err(|error| CliError::new(format!("failed to serialize observation: {error}")))?;

    println!("{OBSERVATION_MARKER}{observation}");
    Ok(run.exit_code())
}

fn main() {
    match execute(std::env::args().skip(1)) {
        Ok(exit_code) => std::process::exit(exit_code),
        Err(error) => {
            eprintln!("crest-synth-witness: {error}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_arguments, BehavioralMutationCase, CliArguments};

    #[test]
    fn accepts_every_healthy_and_matching_mutant_case_in_either_option_order() {
        for case in BehavioralMutationCase::ALL {
            assert_eq!(
                parse_arguments(["--case", case.as_str(), "--mutant", "none"]),
                Ok(CliArguments {
                    case,
                    mutant_enabled: false,
                })
            );
            assert_eq!(
                parse_arguments(["--mutant", case.as_str(), "--case", case.as_str()]),
                Ok(CliArguments {
                    case,
                    mutant_enabled: true,
                })
            );
            assert_eq!(
                parse_arguments(["--case", case.as_str(), "--mutant", case.mutant_cli()]),
                Ok(CliArguments {
                    case,
                    mutant_enabled: true,
                })
            );
        }
        // The declared negative goal-witness command parses exactly.
        assert_eq!(
            parse_arguments([
                "--case",
                "refused-topology",
                "--mutant",
                "refused-topology-published",
            ]),
            Ok(CliArguments {
                case: BehavioralMutationCase::RefusedTopology,
                mutant_enabled: true,
            })
        );
        assert!(parse_arguments([
            "--case",
            "dropped-adjustment",
            "--mutant",
            "refused-topology-published",
        ])
        .is_err());
    }

    #[test]
    fn rejects_missing_duplicate_unknown_extra_and_mismatched_arguments() {
        assert!(parse_arguments(std::iter::empty::<&str>()).is_err());
        assert!(parse_arguments(["--case", "dropped-adjustment"]).is_err());
        assert!(parse_arguments(["--mutant", "none"]).is_err());
        assert!(parse_arguments([
            "--case",
            "dropped-adjustment",
            "--case",
            "dropped-adjustment",
            "--mutant",
            "none",
        ])
        .is_err());
        assert!(parse_arguments([
            "--case",
            "dropped-adjustment",
            "--mutant",
            "none",
            "--mutant",
            "none",
        ])
        .is_err());
        assert!(parse_arguments(["--case", "unknown", "--mutant", "none",]).is_err());
        assert!(parse_arguments(["--case", "dropped-adjustment", "--mutant", "unknown",]).is_err());
        assert!(
            parse_arguments(["--case", "dropped-adjustment", "--mutant", "zero-renderer",])
                .is_err()
        );
        assert!(parse_arguments([
            "--case",
            "dropped-adjustment",
            "--mutant",
            "none",
            "--extra",
        ])
        .is_err());
    }
}
