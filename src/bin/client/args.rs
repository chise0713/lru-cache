use std::process::ExitCode;

use anyhow::Result;

#[derive(supershorty::Args, Debug)]
#[args(name = "lru")]
pub struct Args {
    #[arg(flag = 's', help = "clean until below the given limit, e.g. 1MiB")]
    pub size: Option<Box<str>>,
    #[arg(flag = 'r', help = "output raw data, NUL terminated string slice.")]
    pub raw: bool,
    #[arg(
        flag = 'd',
        help = "a directory to calculate evictation, no input equals all."
    )]
    pub directory: Option<Box<str>>,
}

const EXIT_INVALID_ARG: u8 = 2;

pub fn invalid_argument() -> Result<ExitCode> {
    Args::usage();
    Ok(ExitCode::from(EXIT_INVALID_ARG))
}
