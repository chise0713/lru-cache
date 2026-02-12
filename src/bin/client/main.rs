mod args;

use std::{
    env,
    io::{self, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
    str::FromStr as _,
};

use anyhow::{Result, bail};
use byte_unit::Byte;
use lru_cache::{Directory, Request, ipc::Client};

use crate::args::{Args, Parse as _};

fn main() -> Result<ExitCode> {
    let Args {
        size,
        raw,
        path,
        tag,
    } = match Args::parse() {
        Ok(v) => v,
        Err(e) => {
            return Ok(e);
        }
    };

    let Some(size) = size else {
        return args::invalid_argument();
    };

    let path = path
        .as_ref()
        .map(|s| Path::new(s.as_ref()))
        .unwrap_or(Path::new(""));

    let tag = tag.unwrap_or_default();

    let directory = match (path.as_os_str().is_empty(), tag.is_empty()) {
        (true, true) => Directory::Tag(""),
        (false, true) => Directory::Path(path),
        (false, false) => bail!("specify only one at once"),
        (true, false) => Directory::Tag(&tag),
    };

    let bytes = Byte::from_str(size.as_ref())?.as_u64();

    let socket_path = if let Some(d) = env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        d
    } else {
        PathBuf::from("/run/")
    }
    .join("lru-cache.sock");

    let cl = Client::send_request(socket_path, Request::new(bytes, directory)?)?;

    let resp = cl.read_response()?;
    if raw {
        match io::stdout().write_all(resp.as_bytes()) {
            Ok(()) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::BrokenPipe
                ) => {}
            Err(e) => return Err(e)?,
        };
    } else {
        let s: Box<[&Path]> = resp.evict().collect();
        println!("{}", serde_json::to_string(&s)?);
    }

    Ok(ExitCode::SUCCESS)
}
