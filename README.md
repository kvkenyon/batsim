# batsim

An ERCOT-only residential battery fleet simulator, written in Rust.

batsim provides physics-faithful virtual residential battery systems —
Tesla Powerwall 2/3, Enphase IQ Battery, SolarEdge Home Battery, and
sonnen — behind an OpenAPI-first HTTP API, so dispatch strategies can be
developed and tested against realistic fleet behavior without touching
real hardware.

## Status

**Specification approved; implementation not started.**

## Specification

The authoritative build brief is the founding implementation specification:

[`docs/residential-battery-fleet-simulator-spec.md`](docs/residential-battery-fleet-simulator-spec.md)

All implementation work should follow that document.
