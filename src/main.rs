#![forbid(unsafe_code)]

use std::fmt;
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use es6::{Error as JsError, JsValue, Runtime};
use miette::{Diagnostic, LabeledSpan, NamedSource, Report, SourceCode};

/// A small ECMAScript engine with a QuickJS-style bytecode VM.
#[derive(Debug, Parser)]
#[command(name = "es6", version, about, long_about = None)]
struct Cli {
    /// Script file to execute. Reads stdin when omitted.
    #[arg(value_name = "FILE", conflicts_with = "eval")]
    file: Option<PathBuf>,

    /// Evaluate a source string instead of a file.
    #[arg(
        short = 'e',
        long = "eval",
        value_name = "CODE",
        conflicts_with = "file"
    )]
    eval: Option<String>,

    /// Do not print the result of a top-level expression.
    #[arg(short = 'q', long = "quiet")]
    quiet: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(report) => {
            eprintln!("{report:?}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> miette::Result<()> {
    let cli = Cli::parse();
    let (source, name) = match (cli.eval, cli.file) {
        (Some(source), _) => (source, "<eval>".to_string()),
        (None, Some(path)) => {
            let name = path.display().to_string();
            let source = std::fs::read_to_string(&path)
                .map_err(|error| miette::miette!("failed to read {name}: {error}"))?;
            (source, name)
        }
        (None, None) => {
            let mut source = String::new();
            std::io::stdin()
                .read_to_string(&mut source)
                .map_err(|error| miette::miette!("failed to read stdin: {error}"))?;
            (source, "<stdin>".to_string())
        }
    };

    let runtime = Runtime::new().map_err(|error| miette::miette!("{error}"))?;
    let context = runtime.context();
    let value = context
        .eval(&source)
        .map_err(|error| Report::new(JsDiagnostic::new(error, &name, source.clone())))?;

    if !cli.quiet && !matches!(value, JsValue::Undefined) && source_is_expression(&source) {
        let text = context
            .to_string(&value)
            .map_err(|error| Report::new(JsDiagnostic::new(error, &name, source.clone())))?;
        println!("{text}");
    }
    Ok(())
}

fn source_is_expression(source: &str) -> bool {
    let trimmed = source.trim();
    !trimmed.contains(';')
        && !trimmed.starts_with("let ")
        && !trimmed.starts_with("const ")
        && !trimmed.starts_with("var ")
        && !trimmed.starts_with("function ")
        && !trimmed.starts_with("class ")
}

#[derive(Debug)]
struct JsDiagnostic {
    message: String,
    source: NamedSource<String>,
    source_len: usize,
    offset: Option<usize>,
}

impl JsDiagnostic {
    fn new(error: JsError, name: &str, source: String) -> Self {
        let source_len = source.len();
        Self {
            message: error.to_string(),
            source: NamedSource::new(name, source),
            source_len,
            offset: error.offset(),
        }
    }
}

impl fmt::Display for JsDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for JsDiagnostic {}

impl Diagnostic for JsDiagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new("es6::eval"))
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(&self.source)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        let offset = self
            .offset
            .map(|offset| offset.min(self.source_len.saturating_sub(1)))?;
        Some(Box::new(std::iter::once(LabeledSpan::at_offset(
            offset,
            "error here",
        ))))
    }
}
