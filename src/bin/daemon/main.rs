mod args;
mod config;
mod map;

use std::{
    collections::HashMap,
    env, fs,
    hash::BuildHasherDefault,
    io::ErrorKind,
    os::{
        fd::AsFd as _,
        unix::{ffi::OsStrExt as _, fs::MetadataExt as _},
    },
    path::{Path, PathBuf},
    process::ExitCode,
    str::FromStr as _,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, bail};
use inotify::{Inotify, WatchDescriptor, WatchMask, Watches};
use lru_cache::{Directory, Response, ipc::Daemon};
use nix::sys::{
    epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags, EpollTimeout},
    signal::{SigSet, Signal},
    signalfd::{SfdFlags, SignalFd},
};
use twox_hash::XxHash3_64;
use walkdir::WalkDir;

use crate::{
    args::{Args, Parse as _},
    config::Config,
    map::PathAtimeSizeMap,
};

type XxHashMap<K, V> = HashMap<K, V, BuildHasherDefault<XxHash3_64>>;

struct TagTable {
    entries: Box<[config::Directory]>,
}

impl TagTable {
    fn new(mut entries: Box<[config::Directory]>) -> Self {
        entries.sort_by(|a, b| a.tag.cmp(&b.tag));
        Self { entries }
    }

    fn get(&self, tag: &str) -> Option<&Path> {
        let idx = self
            .entries
            .binary_search_by(|e| e.tag.as_ref().cmp(tag))
            .ok()?;
        Some(self.entries[idx].as_ref())
    }
}

struct ActiveGuard<'a>(&'a AtomicBool);

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn main() -> Result<ExitCode> {
    let Args { config, socket } = match Args::parse() {
        Ok(v) => v,
        Err(e) => {
            return Ok(e);
        }
    };
    let Some(config) = config.as_deref().map(Path::new) else {
        return args::invalid_argument();
    };

    let config = Config::from_str(&fs::read_to_string(config)?)?;

    eprintln!("key-value map initializing.."); // init
    eprintln!("initializing inotify.."); // init

    let mut inotify = Inotify::init()?;
    let mut watches = inotify.watches();

    let mut wd_map = XxHashMap::default();

    let mut map = PathAtimeSizeMap::new();

    for dir in &config.directory {
        let walkdir = WalkDir::new(dir)
            .follow_root_links(false)
            .into_iter()
            .filter_map(Result::ok);
        for entry in walkdir {
            let path = entry.path();
            let meta = entry.metadata()?;
            if meta.is_dir() {
                add_watch(&mut wd_map, &mut watches, path);
            } else {
                map.insert(path, meta.atime(), meta.size());
            }
        }
    }
    eprintln!("inotify initialized");
    eprintln!("key-value map initialized");
    eprintln!(); // finish

    eprintln!("initializing tag-table.."); //init
    let tag_table = TagTable::new(config.directory);
    eprintln!("tag-table initialized"); //init

    eprintln!("starting daemon.."); // start

    let socket_path = if let Some(s) = socket {
        PathBuf::from(s.as_ref())
    } else {
        if let Some(d) = env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
            d
        } else {
            PathBuf::from("/run/")
        }
        .join("lru-cache.sock")
    };

    let ln = match Daemon::bind(socket_path) {
        Ok(d) => d,
        Err(e) => bail!("{e}"),
    };
    ln.set_nonblocking(true)?;
    eprintln!("daemon started");
    eprintln!(); // finish

    let epfd = Epoll::new(EpollCreateFlags::empty())?;

    const DAEMON_TAG: u64 = 1;
    epfd.add(ln.as_fd(), EpollEvent::new(EpollFlags::EPOLLIN, DAEMON_TAG))?;

    const SIGNAL_TAG: u64 = 2;
    let signal_fd = setup_signal();
    epfd.add(
        signal_fd.as_fd(),
        EpollEvent::new(EpollFlags::EPOLLIN, SIGNAL_TAG),
    )?;

    const INOTIFY_TAG: u64 = 3;
    epfd.add(
        inotify.as_fd(),
        EpollEvent::new(EpollFlags::EPOLLIN, INOTIFY_TAG),
    )?;

    eprintln!("enter event loop");
    eprintln!();

    let active = Arc::new(AtomicBool::new(false));

    let mut events = [EpollEvent::empty(); [DAEMON_TAG, SIGNAL_TAG, INOTIFY_TAG].len()];
    'outter: loop {
        match epfd.wait(events.as_mut(), EpollTimeout::NONE) {
            Ok(num) => {
                for ev in &events[..num] {
                    match ev.data() {
                        DAEMON_TAG => handle_daemon(&ln, &map, &tag_table, active.clone())?,
                        SIGNAL_TAG => {
                            while let Ok(Some(_)) = signal_fd.read_signal() {}
                            break 'outter;
                        }
                        INOTIFY_TAG => events_watch(&mut inotify, &mut wd_map, &mut map),
                        _ => unreachable!(),
                    }
                }
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }

    Ok(ExitCode::SUCCESS)
}

fn handle_daemon(
    ln: &Daemon,
    map: &PathAtimeSizeMap,
    tag_table: &TagTable,
    active: Arc<AtomicBool>,
) -> Result<()> {
    let (mut accepted, _guard) = loop {
        match ln.accept() {
            Ok(v) => {
                if active
                    .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    break (v, ActiveGuard(&active));
                } else {
                    return Ok(());
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::Interrupted) => continue,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock) => return Ok(()),
            Err(e) => return Err(e)?,
        }
    };

    eprintln!("\naccepted client\n");

    let req = accepted.read_request()?;
    let size = req.amount();
    eprintln!("client requested size: {size}\n");

    let directory = req.directory();
    let path = match directory {
        Directory::Tag("") => Path::new(""),
        Directory::Tag(tag) => {
            let Some(path) = tag_table.get(tag) else {
                eprintln!("tag: \"{tag}\" not found");
                return Ok(());
            };
            path
        }
        Directory::Path(path) => path,
    };

    let path_bytes_is_empty = path.as_os_str().as_bytes().is_empty();
    let prefix_filter = (!path_bytes_is_empty).then_some(path);

    let evict = map.plan_evict_until(size, prefix_filter);
    let resp = Response::new(evict)?;
    accepted.send_response(resp)?;

    eprintln!("responsed to client\n");

    Ok(())
}

fn setup_signal() -> SignalFd {
    let mut mask = SigSet::empty();
    mask.add(Signal::SIGINT);
    mask.add(Signal::SIGTERM);
    mask.thread_block().unwrap();

    SignalFd::with_flags(&mask, SfdFlags::SFD_NONBLOCK).unwrap()
}

fn add_watch(
    wd_map: &mut XxHashMap<WatchDescriptor, Box<Path>>,
    watches: &mut Watches,
    path: &Path,
) {
    let Ok(wd) = watches.add(
        path,
        WatchMask::CREATE
            | WatchMask::ACCESS
            | WatchMask::MODIFY
            | WatchMask::CLOSE_WRITE
            | WatchMask::MOVED_TO
            | WatchMask::DELETE
            | WatchMask::MOVED_FROM,
    ) else {
        return;
    };

    wd_map.insert(wd, Box::from(path));
}

fn events_watch(
    inotify: &mut Inotify,
    wd_map: &mut XxHashMap<WatchDescriptor, Box<Path>>,
    map: &mut PathAtimeSizeMap,
) {
    use inotify::EventMask as E;

    let mut buffer = [0u8; 4096];
    loop {
        let events = match inotify.read_events(&mut buffer) {
            Ok(e) => e,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock) => {
                break;
            }
            Err(e) if matches!(e.kind(), ErrorKind::Interrupted) => {
                continue;
            }
            Err(e) => panic!("failed to read inotify events: {}", e),
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        for event in events {
            let is_dir = event.mask.contains(E::ISDIR);
            let ignored = event.mask.contains(E::IGNORED);
            let create = event.mask.intersects(E::CREATE | E::MOVED_TO);
            let modify = event
                .mask
                .intersects(E::MODIFY | E::CLOSE_WRITE | E::ACCESS);
            let delete = event.mask.intersects(E::DELETE | E::MOVED_FROM);

            if ignored {
                wd_map.remove(&event.wd);
                continue;
            }

            let base = match wd_map.get(&event.wd) {
                Some(b) => b,
                None => continue,
            };

            let full = event
                .name
                .map(|n| base.join(n).into_boxed_path())
                .unwrap_or_else(|| base.clone());

            if is_dir && create {
                add_watch(wd_map, &mut inotify.watches(), &full);
                continue;
            }

            if create {
                eprintln!("\"{}\" created, updating key-value map", full.display());
            }

            if modify {
                eprintln!("\"{}\" modified, updating key-value map", full.display());
            }

            if (create || modify)
                && let Ok(meta) = full.metadata()
            {
                map.insert(&full, now, meta.size());
            }

            if delete {
                eprintln!("\"{}\" removed, updating key-value map", full.display());
                map.remove(&full);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tag_table_lookup() {
        let entries: Box<[config::Directory]> = [
            config::Directory {
                path: Box::from(Path::new("/a")),
                tag: "alpha".into(),
            },
            config::Directory {
                path: Box::from(Path::new("/b")),
                tag: "beta".into(),
            },
        ]
        .into();

        let table = TagTable::new(entries);

        assert_eq!(table.get("alpha"), Some(Path::new("/a")));
        assert_eq!(table.get("beta"), Some(Path::new("/b")));
        assert_eq!(table.get("not-exist"), None);
    }
}
