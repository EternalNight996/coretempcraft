# CoreTempCraft

**A real-time CPU frequency and temperature monitor for Windows, built with [Craft Anything](https://github.com/storytold/craft-anything).**

PulseCraft shows what each core is doing right now: its current multiplier, the bus clock it was
measured at, the resulting frequency, and the package temperature, with a small history graph and a
tray-friendly window. It reads Windows performance counters and, where they exist, the CPU's own
frequency counters, so it needs no kernel driver and no administrator rights.

Free and open source (MIT). A community app of the [ArtCraft](https://getartcraft.com/) family — [www.mengbot.com.cn](https://www.mengbot.com.cn).

## What it shows

- **Per-core frequency**: multiplier × measured bus clock, updated on a fixed interval
- **Package temperature** from the platform's thermal zone when Windows exposes one
- **Base clock measurement**: when the system doesn't report a base clock, it measures one from the
  CPU timestamp counter over a five-second window
- **History**: a sparkline per core, and the current sample as a list

## Build and run

Requires Rust (edition 2021) and, on Windows, the MSVC toolchain.

```sh
cargo run --release
```

The window takes no arguments; it starts sampling immediately.

## How it works

- Windows exposes the current multiplier through the processor performance counters
  (`Pdh`/`CallNtPowerInformation` equivalents), and the bus clock through the same source where
  Windows reports it.
- Where it doesn't, the app measures a base clock from the CPU timestamp counter over five seconds
  and keeps that measurement for the session.
- Temperature comes from the thermal zone Windows reports for the package, and is shown as an
  estimate when the platform offers none.

## Status

It builds, starts and draws, and the checks made while rebuilding it pass. It is a study
reimplementation: it reproduces the *behaviour* the original app exposes, not its code, assets or
branding.

## Credits

Built with [Craft Anything](https://github.com/storytold/craft-anything), the clean-room engine that
shows how software works and then builds your own version of it.

This is an independent project. It is not affiliated with, endorsed by, or connected to the authors
of any commercial program it was studied from, and it ships none of their code, icons or other
assets.

## License

MIT — see [LICENSE](LICENSE).
