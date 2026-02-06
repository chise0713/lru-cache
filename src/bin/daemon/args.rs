use std::process::ExitCode;

use anyhow::Result;

#[derive(supershorty::Args, Debug)]
#[args(name = "lru-daemon")]
pub struct Args {
    #[arg(flag = 'c', help = "config path")]
    pub config: Option<Box<str>>,
}

const EXIT_INVALID_ARG: u8 = 2;

pub fn invalid_argument() -> Result<ExitCode> {
    Args::usage();
    Ok(ExitCode::from(EXIT_INVALID_ARG))
}
