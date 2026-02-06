lru-cache

[ipc](./src/ipc.rs)

[daemon](./src/bin/daemon/) => do the lru filtering job

[client](./src/bin/client/) => ask daemon and output result

config example:
```toml
[[directory]]
path = "/home/user/.cargo/target"

[[directory]]
path = "/home/user/.cache"
```

client example:
```shell
lru -rs 1GB | tr '\0' '\n' | less
```
```shell
lru -rs 1GB | xargs -0 rm -vf
```

usages:

daemon:
```console
usage: lru-daemon [-h] [-c config]
Command Summary:
        -c              config path
        -h              prints this help message
```
client:
```console
usage: lru [-hr] [-s size]
Command Summary:
        -h              prints this help message
        -r              output raw data, NUL terminated string slice.
        -s              clean until below the given limit, e.g. 1MiB
```

```
Copyright (C) 2026 chise0713

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with this program.  If not, see <https://www.gnu.org/licenses/>.
```