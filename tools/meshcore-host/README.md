# meshcore-host

The radio worker for the backbone's MeshCore gateway. A Heltec V4 gateway is only a
LoRa radio; `meshcore-host` runs the MeshCore identities of the backbone's users
(one virtual node per person, using the official MeshCore code) and talks to the
radio over KISS through the server.

It is a TCP client. In service mode it connects to `127.0.0.1:5057` (the server's
`--meshcore-worker` socket) and shares the folder `meshcore-nodes` in the
server's data folder with `maarse-server`: the server drops small request files
(`provision`, `announce`, `scan`, `repeater-job.txt`, ...) there, and the worker
answers with files such as `public-key.txt` and `scan-results.txt`. It must run on
the same machine as the server.

## On a server (Debian or Ubuntu)

Nothing to do: `scripts/install-server.sh` (also with `--update`) builds it and installs
`/usr/local/bin/meshcore-host`, and `maarse-server` starts it when the MeshCore
gateway is on and keeps it running. `scripts/install-meshcore-worker.sh` builds
only the worker. See the "Connect a Heltec V4 radio" part of docs/GUIDE.md.

## Build

```sh
tools/meshcore-host/build.sh [output-file]      # Linux: git, cmake, a C++17 compiler
```

The script fetches the official MeshCore sources at the revision pinned in
`firmware/meshcore-gateway/MESHCORE_REVISION` (into `meshcore-src/`, not
committed) and compiles them unmodified. On Windows the original build is clang-cl
with Ninja: `cmake -S tools/meshcore-host -B build -G Ninja -DMESHCORE_DIR=<MeshCore checkout>
-DCMAKE_C_COMPILER=clang-cl -DCMAKE_CXX_COMPILER=clang-cl`.

## Run by hand

```sh
meshcore-host --service --tcp 127.0.0.1:5057 --data /var/lib/maarse-server/meshcore-nodes
```

Without `--service` it is an interactive test client (`--node NAME`, commands such
as `scan`, `advert`; type `help`). A serial port (`--port COM9`) is supported on
Windows only; on Linux use `--tcp`.

## Licenses

MeshCore (MIT, Scott Powell / rippleradios.com), the Crypto library in
`third_party/Crypto` (rweather/arduinolibs, MIT; the licence is in each file) and
`third_party/base64` (Densaugeo, MIT).

## Sending settings (path hash size and region scope)

In service mode each person folder may hold `radio.txt`: `<path hash size 1-3> <region>[,<region>...]`, for example `2 dk,dk-nordjylland` (up to 8 regions). The worker re-reads it every few hundred milliseconds. Every flood the person sends (channel messages, adverts, direct messages and repeater requests that flood) uses that many bytes per hop in the path, and, if regions are set, is sent once per region, each copy carrying that region's transport code (key = SHA-256 of `#region`, first 16 bytes, like a repeater's `region put`), so only repeaters that know that region pass it on (a packet carries one code, so several regions mean several copies; repeaters drop the later copies of a packet they already passed on). Without the file the defaults apply: 1 byte, no scope. The web client and the Android app write the file through the backbone (MeshCore, Chats, "Sending settings").
