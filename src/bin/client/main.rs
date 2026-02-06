mod args;

use std::{
    env,
    io::{self, Write as _},
    path::PathBuf,
    process::ExitCode,
    str::FromStr as _,
};

use anyhow::Result;
use byte_unit::Byte;
use lru_cache::{Request, ipc::Client};

use crate::args::{Args, Parse as _};

fn main() -> Result<ExitCode> {
    let Args { size, raw } = match Args::parse() {
        Ok(v) => v,
        Err(e) => {
            return Ok(e);
        }
    };

    let Some(size) = size else {
        return args::invalid_argument();
    };

    let bytes = Byte::from_str(size.as_ref())?.as_u64();

    let runtime_dir = if let Some(d) = env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        d
    } else {
        PathBuf::from("/run/")
    };
    let socket_path = runtime_dir.join("lru-cache.sock");

    let cl = Client::request(socket_path, Request::new(bytes))?;

    if raw {
        match io::stdout().write_all(&cl.raw_data()?) {
            Ok(()) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::BrokenPipe
                ) => {}
            Err(e) => return Err(e)?,
        };
    } else {
        println!("{}", serde_json::to_string(&cl.evict()?)?);
    }

    Ok(ExitCode::SUCCESS)
}
